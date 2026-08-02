//! `lscolors` (bundled): colors `PANEL_FILE` entries, via the generic
//! `Extension::entry_style` hook — an override the shared entry-grid renderer
//! consults before falling back to the theme's own dir/symlink/default
//! coloring. See `parse.rs` for the SGR/table parsing (kept separate and pure
//! so it's testable without an env var or the `Extension` trait).
//!
//! The `lscolors.coloring` option (`@/settings/Extensions/lscolors`) picks the
//! source table: `none` (off), `LS_COLORS` (the shell's env var — a no-op if
//! unset), or `Palette` — the `PaletteRegistry`'s active named palette, browsable
//! and editable under `@/settings/Extensions/lscolors/Themes` (see `LsColorsNode`
//! below), seeded from the bundled `dark`/`light` presets in `presets.scm` so
//! coloring works out of the box with no `LS_COLORS` in the environment.

use std::cell::RefCell;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock, RwLock};

use ratatui::style::Style;

use crate::core::{
    AppState, Entry, ExtensionIntent, ExtensionValue, InputFormSpec, Intent, Mode, OptionStore, KEY_ITEM,
};
use crate::extension::settings::SettingsNode;
use crate::extension::{About, Extension};
use crate::vfs::vfs_root;

use super::parse::{self, LsColorsTable};
use super::registry::{NamedPalette, PaletteRegistry};
use super::writer::LsColorsWriter;

const ID: &str = "lscolors";
const OPTIONS_SCHEME: &str = include_str!("options.scm");

// ── LsColorsNode ──────────────────────────────────────────────────────────────

/// Serves the `@/settings/Extensions/lscolors/Themes` subtree via the
/// `SettingsNode` graft — the ls-colors analogue of `theme::ThemeNode`, minus
/// the per-role editor: a palette's `raw` LS_COLORS string edits as one text
/// field instead of per-key color pickers (the table has no fixed role set to
/// draw individual pickers for — just fixed slots plus an open `*.ext` map).
struct LsColorsNode {
    registry: Arc<RwLock<PaletteRegistry>>,
}

impl SettingsNode for LsColorsNode {
    fn anchor(&self) -> (&str, &str) {
        (ID, "Themes")
    }

    fn list(&self, path: &Path) -> io::Result<Vec<Entry>> {
        let reg = self.registry.read().map_err(|e| {
            io::Error::new(io::ErrorKind::Other, format!("lscolors registry lock poisoned: {e}"))
        })?;

        let depth = path.components().count();
        // @/settings/Extensions/lscolors/Themes → depth 5
        // @/settings/Extensions/lscolors/Themes/<name> → depth 6
        let base = vfs_root().join("settings").join("Extensions").join(ID).join("Themes");

        if depth <= 5 {
            // Root of Themes: one dir entry per palette, then `+ New palette…`.
            let active = reg.active().to_string();
            let mut entries: Vec<Entry> = reg.names().into_iter().map(|name| {
                let np = reg.get(&name).unwrap();
                let kind = if np.preset { "preset" } else { "custom" };
                let badge = if name == active { format!("{name} ✓ ({kind})") } else { format!("{name} ({kind})") };
                Entry::virtual_dir(base.join(&name), badge)
            }).collect();
            let mut new_leaf = Entry::virtual_leaf(base.join("+ New palette…"), "+ New palette…".into());
            new_leaf.activation = Some(ExtensionIntent {
                extension: ID.into(),
                intent: "new".into(),
                data: ExtensionValue::Nil,
            });
            entries.push(new_leaf);
            Ok(entries)
        } else {
            // Palette dir: get the palette name from component 5.
            let palette_name = path.components().nth(5)
                .and_then(|c| if let std::path::Component::Normal(s) = c { s.to_str() } else { None })
                .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "missing palette name in path"))?;

            let np = reg.get(palette_name).ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, format!("palette '{palette_name}' not found"))
            })?;
            let is_preset = np.preset;
            let active = reg.active().to_string();
            let palette_base = base.join(palette_name);
            let mut entries: Vec<Entry> = Vec::new();

            // "Set active" leaf (omit if already active).
            if palette_name != active {
                let mut e = Entry::virtual_leaf(palette_base.join("set-active"), "Set active".into());
                e.activation = Some(ExtensionIntent {
                    extension: ID.into(),
                    intent: "set-active".into(),
                    data: ExtensionValue::String(palette_name.to_string()),
                });
                entries.push(e);
            }

            if is_preset {
                let mut header = Entry::virtual_leaf(palette_base.join("(preset)"), "preset — duplicate to edit".into());
                header.activation = Some(ExtensionIntent {
                    extension: ID.into(),
                    intent: "duplicate".into(),
                    data: ExtensionValue::String(palette_name.to_string()),
                });
                entries.push(header);
            }

            // The whole raw LS_COLORS string, editable as one text field.
            let mut raw_leaf = Entry::virtual_leaf(palette_base.join("raw"), format!("raw: {}", np.raw));
            raw_leaf.activation = Some(ExtensionIntent {
                extension: ID.into(),
                intent: if is_preset { "duplicate" } else { "edit-raw" }.to_string(),
                data: ExtensionValue::String(palette_name.to_string()),
            });
            entries.push(raw_leaf);

            // Duplicate, Rename (custom only), Delete (custom only).
            let mut dup = Entry::virtual_leaf(palette_base.join("duplicate"), "Duplicate…".into());
            dup.activation = Some(ExtensionIntent {
                extension: ID.into(),
                intent: "duplicate".into(),
                data: ExtensionValue::String(palette_name.to_string()),
            });
            entries.push(dup);

            if !is_preset {
                let mut rename = Entry::virtual_leaf(palette_base.join("rename"), "Rename…".into());
                rename.activation = Some(ExtensionIntent {
                    extension: ID.into(),
                    intent: "rename".into(),
                    data: ExtensionValue::String(palette_name.to_string()),
                });
                entries.push(rename);

                let mut del = Entry::virtual_leaf(palette_base.join("delete"), "Delete".into());
                del.activation = Some(ExtensionIntent {
                    extension: ID.into(),
                    intent: "delete".into(),
                    data: ExtensionValue::String(palette_name.to_string()),
                });
                entries.push(del);
            }

            Ok(entries)
        }
    }
}

// ── LsColorsExtension ─────────────────────────────────────────────────────────

pub struct LsColorsExtension {
    /// Parsed once at boot: the shell's `LS_COLORS` (empty table if unset).
    env_table: LsColorsTable,
    pub registry: Arc<RwLock<PaletteRegistry>>,
    writer: Option<LsColorsWriter>,
    /// Memo of the active palette's parsed table, keyed by its name — refreshed
    /// only when the active name changes, since `entry_style` is called once
    /// per visible entry (reparsing the raw string on every call would be
    /// quadratic in listing size).
    active_cache: RefCell<Option<(String, LsColorsTable)>>,
    /// Set once from boot via `bind_store`; read in `entry_style` for the
    /// `lscolors.coloring` option value.
    store: OnceLock<Arc<OptionStore>>,
}

impl LsColorsExtension {
    /// `config_dir` is the bare app config directory (e.g. `~/.config/wasdf`),
    /// matching the convention other bundled extensions take (see
    /// `bookmarks::BookmarksExtension::new`) — this extension owns both its
    /// file name inside it (`config.scm`) and its own preset registry, so
    /// nothing about it needs to live in the kernel.
    pub fn new(config_dir: Option<PathBuf>) -> Self {
        let presets = parse_presets();
        let registry = Arc::new(RwLock::new(PaletteRegistry::with_presets(presets)));

        let config_scm = config_dir.map(|d| d.join("config.scm"));
        if let Some(ref path) = config_scm {
            if let Ok(text) = std::fs::read_to_string(path) {
                boot_load_config(&registry, &text);
            }
        }
        let env_table = std::env::var("LS_COLORS")
            .map(|raw| parse::parse_ls_colors(&raw))
            .unwrap_or_default();
        let writer = config_scm.map(LsColorsWriter::new);
        LsColorsExtension {
            env_table,
            registry,
            writer,
            active_cache: RefCell::new(None),
            store: OnceLock::new(),
        }
    }
}

/// Parse the bundled `presets.scm` into `NamedPalette` presets.
fn parse_presets() -> Vec<NamedPalette> {
    let datum = crate::script::sexpr::parse(super::PRESETS_SCHEME)
        .unwrap_or(crate::script::sexpr::Datum::List(vec![]));
    let inner = datum.as_list()
        .and_then(|l| l.get(1))
        .cloned()
        .unwrap_or(crate::script::sexpr::Datum::List(vec![]));
    crate::script::config::parse_lscolors_presets_config(&inner)
}

/// Parse the managed block in `text` for `(define-lscolors-palette …)` and
/// `(set-lscolors-palette …)` forms, loading customs first so a custom active
/// name is recognized by `set_active`.
fn boot_load_config(registry: &Arc<RwLock<PaletteRegistry>>, text: &str) {
    let lines: Vec<&str> = text.lines().collect();
    let begin = lines.iter().position(|l| l.trim() == super::writer::BLOCK_BEGIN);
    let end = lines.iter().position(|l| l.trim() == super::writer::BLOCK_END);
    let (Some(b), Some(e)) = (begin, end) else { return };
    if b >= e { return; }
    let block = &lines[b + 1..e];

    let mut customs = Vec::new();
    let mut active_name: Option<String> = None;
    let mut i = 0;

    while i < block.len() {
        let t = block[i].trim();
        if t.starts_with("(define-lscolors-palette ") {
            // Single- or multi-line form: count parens to find the closing line.
            let start = i;
            let mut depth = 0i32;
            let mut end_i = i;
            for j in i..block.len() {
                for ch in block[j].chars() {
                    match ch { '(' => depth += 1, ')' => depth -= 1, _ => {} }
                }
                end_i = j;
                if depth <= 0 { break; }
            }
            let form_str = block[start..=end_i].join("\n");
            if let Ok(datum) = crate::script::sexpr::parse(&form_str) {
                if let Some(np) = crate::script::config::parse_define_lscolors_palette(&datum) {
                    customs.push(np);
                }
            }
            i = end_i + 1;
        } else if t.starts_with("(set-lscolors-palette ") {
            if let Ok(datum) = crate::script::sexpr::parse(t) {
                if let Some(name) = crate::script::config::parse_set_lscolors_palette(&datum) {
                    active_name = Some(name);
                }
            }
            i += 1;
        } else {
            i += 1;
        }
    }

    if !customs.is_empty() {
        let _ = registry.write().map(|mut r| r.load_customs(customs));
    }
    if let Some(name) = active_name {
        let _ = registry.write().map(|mut r| r.set_active(&name));
    }
}

impl Extension for LsColorsExtension {
    fn id(&self) -> &str {
        ID
    }

    fn about(&self) -> About {
        About::new(
            "LS Colors",
            "Color file-list entries from LS_COLORS or a bundled/custom named palette",
        )
    }

    fn option_scheme(&self) -> Option<String> {
        Some(OPTIONS_SCHEME.to_string())
    }

    fn bind_store(&self, store: &Arc<OptionStore>) {
        let _ = self.store.set(store.clone());
    }

    fn settings_nodes(&self) -> Vec<Arc<dyn SettingsNode>> {
        vec![Arc::new(LsColorsNode { registry: self.registry.clone() })]
    }

    fn entry_style(&self, entry: &Entry) -> Option<Style> {
        let store = self.store.get()?;
        match store.get_str("lscolors.coloring").as_str() {
            "LS_COLORS" => parse::style_for(&self.env_table, entry),
            "Palette" => {
                let active = self.registry.read().ok()?.active().to_string();
                let stale = self.active_cache.borrow().as_ref().map(|(n, _)| *n != active).unwrap_or(true);
                if stale {
                    let table = self.registry.read().ok()?.resolve(&active);
                    *self.active_cache.borrow_mut() = Some((active, table));
                }
                let cache = self.active_cache.borrow();
                parse::style_for(&cache.as_ref()?.1, entry)
            }
            // "none" (or any unrecognized value) — no opinion.
            _ => None,
        }
    }

    fn handle_intent(&self, intent: &ExtensionIntent, _state: &AppState) -> Vec<Intent> {
        match intent.intent.as_str() {
            // ── Selection ──────────────────────────────────────────────────────
            "set-active" => {
                let Some(name) = intent.data.as_str() else { return vec![] };
                let changed = self.registry.write()
                    .map(|mut r| r.set_active(name))
                    .unwrap_or(false);
                if !changed { return vec![]; }
                if let Some(w) = &self.writer { let _ = w.set_active(name); }
                vec![Intent::Refresh]
            }

            // ── New palette ────────────────────────────────────────────────────
            "new" => {
                let template = ExtensionIntent {
                    extension: ID.into(),
                    intent: "new-apply".into(),
                    data: ExtensionValue::Nil,
                };
                let spec = InputFormSpec::emit_input_form("New palette name", template, None);
                vec![Intent::PushMode(Box::new(Mode::InputForm(spec)))]
            }
            "new-apply" => {
                let Some(name_raw) = intent.data.get(KEY_ITEM).and_then(|v| v.as_str()) else {
                    return vec![];
                };
                let name = name_raw.trim().to_string();
                if name.is_empty() { return vec![]; }
                let new_palette = {
                    let Ok(reg) = self.registry.read() else { return vec![]; };
                    if reg.get(&name).is_some() { return vec![]; }
                    let active = reg.active().to_string();
                    let seed_raw = reg.get(&active).map(|p| p.raw.clone()).unwrap_or_default();
                    NamedPalette { name: name.clone(), preset: false, raw: seed_raw }
                };
                {
                    let Ok(mut reg) = self.registry.write() else { return vec![]; };
                    reg.upsert_custom(new_palette.clone());
                    reg.set_active(&name);
                }
                if let Some(w) = &self.writer {
                    let _ = w.upsert_palette(&new_palette);
                    let _ = w.set_active(&name);
                }
                vec![Intent::Refresh]
            }

            // ── Duplicate ──────────────────────────────────────────────────────
            "duplicate" => {
                let Some(source) = intent.data.as_str().map(str::to_string) else { return vec![]; };
                let template = ExtensionIntent {
                    extension: ID.into(),
                    intent: "duplicate-apply".into(),
                    data: ExtensionValue::Nil
                        .with("source", ExtensionValue::String(source)),
                };
                let spec = InputFormSpec::emit_input_form("Duplicate as", template, None);
                vec![Intent::PushMode(Box::new(Mode::InputForm(spec)))]
            }
            "duplicate-apply" => {
                let Some(source_name) = intent.data.get("source").and_then(|v| v.as_str()) else {
                    return vec![];
                };
                let Some(new_name_raw) = intent.data.get(KEY_ITEM).and_then(|v| v.as_str()) else {
                    return vec![];
                };
                let source_name = source_name.to_string();
                let new_name = new_name_raw.trim().to_string();
                if new_name.is_empty() { return vec![]; }
                let new_palette = {
                    let Ok(reg) = self.registry.read() else { return vec![]; };
                    if reg.get(&new_name).is_some() { return vec![]; }
                    let raw = reg.get(&source_name).map(|p| p.raw.clone()).unwrap_or_default();
                    NamedPalette { name: new_name.clone(), preset: false, raw }
                };
                {
                    let Ok(mut reg) = self.registry.write() else { return vec![]; };
                    reg.upsert_custom(new_palette.clone());
                    reg.set_active(&new_name);
                }
                if let Some(w) = &self.writer {
                    let _ = w.upsert_palette(&new_palette);
                    let _ = w.set_active(&new_name);
                }
                vec![Intent::Refresh]
            }

            // ── Rename ─────────────────────────────────────────────────────────
            "rename" => {
                let Some(old_name) = intent.data.as_str() else { return vec![] };
                let template = ExtensionIntent {
                    extension: ID.into(),
                    intent: "rename-apply".into(),
                    data: ExtensionValue::Nil
                        .with("source", ExtensionValue::String(old_name.to_string())),
                };
                let spec = InputFormSpec::emit_input_form("Rename to", template, Some(old_name.to_string()));
                vec![Intent::PushMode(Box::new(Mode::InputForm(spec)))]
            }
            "rename-apply" => {
                let Some(old_name) = intent.data.get("source").and_then(|v| v.as_str()) else {
                    return vec![];
                };
                let Some(new_name_raw) = intent.data.get(KEY_ITEM).and_then(|v| v.as_str()) else {
                    return vec![];
                };
                let old_name = old_name.to_string();
                let new_name = new_name_raw.trim().to_string();
                if new_name.is_empty() { return vec![]; }
                let (was_active, renamed) = {
                    let Ok(reg) = self.registry.read() else { return vec![]; };
                    if reg.get(&new_name).is_some() { return vec![]; }
                    let Some(old_np) = reg.get(&old_name) else { return vec![]; };
                    if old_np.preset { return vec![]; }
                    let was_active = reg.active() == old_name;
                    let renamed = NamedPalette {
                        name: new_name.clone(),
                        preset: false,
                        raw: old_np.raw.clone(),
                    };
                    (was_active, renamed)
                };
                {
                    let Ok(mut reg) = self.registry.write() else { return vec![]; };
                    reg.remove_custom(&old_name);
                    reg.upsert_custom(renamed.clone());
                    if was_active { reg.set_active(&new_name); }
                }
                if let Some(w) = &self.writer {
                    let _ = w.remove_palette(&old_name);
                    let _ = w.upsert_palette(&renamed);
                    if was_active { let _ = w.set_active(&new_name); }
                }
                vec![Intent::Refresh]
            }

            // ── Edit raw string ────────────────────────────────────────────────
            "edit-raw" => {
                let Some(name) = intent.data.as_str() else { return vec![] };
                let Ok(reg) = self.registry.read() else { return vec![] };
                let Some(np) = reg.get(name) else { return vec![] };
                if np.preset { return vec![]; }
                let template = ExtensionIntent {
                    extension: ID.into(),
                    intent: "edit-raw-apply".into(),
                    data: ExtensionValue::Nil.with("palette", ExtensionValue::String(name.to_string())),
                };
                let spec = InputFormSpec::emit_input_form(
                    format!("{name} — raw LS_COLORS"),
                    template,
                    Some(np.raw.clone()),
                );
                vec![Intent::PushMode(Box::new(Mode::InputForm(spec)))]
            }
            "edit-raw-apply" => {
                let Some(name) = intent.data.get("palette").and_then(|v| v.as_str()) else {
                    return vec![];
                };
                let Some(raw) = intent.data.get(KEY_ITEM).and_then(|v| v.as_str()) else {
                    return vec![];
                };
                let name = name.to_string();
                let raw = raw.to_string();
                let updated = {
                    let Ok(reg) = self.registry.read() else { return vec![]; };
                    let Some(np) = reg.get(&name) else { return vec![]; };
                    if np.preset { return vec![]; }
                    NamedPalette { name: np.name.clone(), preset: false, raw }
                };
                { let Ok(mut reg) = self.registry.write() else { return vec![]; }; reg.upsert_custom(updated.clone()); }
                if let Some(w) = &self.writer { let _ = w.upsert_palette(&updated); }
                vec![Intent::Refresh]
            }

            // ── Delete ─────────────────────────────────────────────────────────
            "delete" => {
                let Some(name) = intent.data.as_str() else { return vec![] };
                let is_preset = self.registry.read().ok()
                    .and_then(|r| r.get(name).map(|p| p.preset))
                    .unwrap_or(true);
                if is_preset { return vec![]; }
                let confirm_intent = Intent::Extension(ExtensionIntent {
                    extension: ID.into(),
                    intent: "delete-confirm".into(),
                    data: ExtensionValue::String(name.to_string()),
                });
                vec![Intent::PushMode(Box::new(Mode::Policy(Box::new(confirm_intent))))]
            }
            "delete-confirm" => {
                let Some(name) = intent.data.as_str() else { return vec![] };
                let was_active = self.registry.read().ok()
                    .map(|r| r.active() == name)
                    .unwrap_or(false);
                let _ = self.registry.write().map(|mut r| r.remove_custom(name));
                if let Some(w) = &self.writer { let _ = w.remove_palette(name); }
                if was_active {
                    let fallback = self.registry.read().ok().and_then(|r| r.names().first().cloned());
                    if let Some(fallback) = fallback {
                        let _ = self.registry.write().map(|mut r| r.set_active(&fallback));
                        if let Some(w) = &self.writer { let _ = w.set_active(&fallback); }
                    }
                }
                // The deleted palette's directory is gone — return to the Themes root.
                let themes_root = vfs_root().join("settings").join("Extensions").join(ID).join("Themes");
                vec![Intent::NavigateTo(themes_root)]
            }

            _ => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn entry(name: &str, is_dir: bool) -> Entry {
        Entry {
            path: PathBuf::from(name),
            name: name.to_string(),
            is_dir,
            is_symlink: false,
            size: 0,
            mode: 0o644,
            uid: 0,
            gid: 0,
            modified: None,
            created: None,
            accessed: None,
            symlink_target: None,
            activation: None,
        }
    }

    fn ext() -> LsColorsExtension {
        LsColorsExtension::new(None)
    }

    #[test]
    fn builtin_dark_and_light_presets_parse_and_color_dirs_and_rs_files() {
        let reg = PaletteRegistry::with_presets(parse_presets());
        let dark = reg.resolve("dark");
        let light = reg.resolve("light");
        assert!(parse::style_for(&dark, &entry("x", true)).is_some(), "dark: dir has a color");
        assert!(parse::style_for(&light, &entry("x", true)).is_some(), "light: dir has a color");
        assert!(parse::style_for(&dark, &entry("main.rs", false)).is_some(), "dark: .rs has a color");
        assert!(parse::style_for(&light, &entry("main.rs", false)).is_some(), "light: .rs has a color");
    }

    #[test]
    fn no_bound_store_yields_no_opinion() {
        let e = ext();
        assert!(e.entry_style(&entry("main.rs", false)).is_none());
    }

    #[test]
    fn set_active_switches_the_resolved_table() {
        let e = ext();
        let store = Arc::new(OptionStore::new(vec![crate::core::OptionDef {
            key: "lscolors.coloring".into(),
            category: "LsColors".into(),
            label: "coloring".into(),
            kind: crate::core::OptionKind::Choice(vec!["none".into(), "LS_COLORS".into(), "Palette".into()]),
            default: "Palette".into(),
            owner: Some(ID.into()),
        }]));
        e.bind_store(&store);
        assert_eq!(e.registry.read().unwrap().active(), "dark");
        assert!(e.registry.write().unwrap().set_active("light"));
        assert_eq!(e.registry.read().unwrap().active(), "light");
        assert!(e.entry_style(&entry("x", true)).is_some());
    }
}
