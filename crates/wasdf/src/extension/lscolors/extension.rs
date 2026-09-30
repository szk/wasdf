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
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock, RwLock, RwLockReadGuard};

use ratatui::style::Style;

use crate::core::{
    AppState, ConfigWrite, Entry, ExtensionIntent, ExtensionValue, InputFormSpec, Intent, Mode, OptionStore,
    OptionValue, KEY_ITEM,
};
use crate::extension::settings::SettingsNode;
use crate::extension::{About, Extension};
use crate::vfs::vfs_root;

use super::parse::{self, LsColorsTable};
use super::registry::{self, NamedPalette, PaletteRegistry};

const ID: &str = "lscolors";
const OPTIONS_SCHEME: &str = include_str!("options.scm");
const PRELUDE_SCHEME: &str = include_str!("prelude.scm");

/// The registry as a projection of the `lscolors.palette` / `lscolors.custom`
/// options, rebuilt whenever the OptionStore's generation moves (the same
/// shape as the theme extension's state).
struct PaletteState {
    registry: RwLock<PaletteRegistry>,
    options: OnceLock<Arc<OptionStore>>,
    /// The options generation the registry was last built from, plus one
    /// (0 = never built).
    synced: AtomicU64,
}

impl PaletteState {
    fn sync(&self) {
        let Some(options) = self.options.get() else { return };
        let generation = options.generation() + 1;
        if self.synced.load(Ordering::Acquire) == generation {
            return;
        }
        let customs = options.value("lscolors.custom").map(|v| registry::customs_from_value(&v)).unwrap_or_default();
        let active = options.get_str("lscolors.palette");
        if let Ok(mut reg) = self.registry.write() {
            reg.replace_customs(customs);
            if !reg.set_active(&active)
                && let Some(first) = reg.names().first().cloned()
            {
                reg.set_active(&first);
            }
        }
        self.synced.store(generation, Ordering::Release);
    }

    fn read(&self) -> Option<RwLockReadGuard<'_, PaletteRegistry>> {
        self.sync();
        self.registry.read().ok()
    }

    fn write_customs<'a>(palettes: impl IntoIterator<Item = &'a NamedPalette>) -> Intent {
        Intent::Configure(ConfigWrite::SetOption { key: "lscolors.custom".into(), value: registry::customs_to_value(palettes) })
    }

    fn write_active(name: &str) -> Intent {
        Intent::Configure(ConfigWrite::SetOption { key: "lscolors.palette".into(), value: OptionValue::Text(name.into()) })
    }
}

// ── LsColorsNode ──────────────────────────────────────────────────────────────

/// Serves the `@/settings/Extensions/lscolors/Themes` subtree via the
/// `SettingsNode` graft — the ls-colors analogue of `theme::ThemeNode`, minus
/// the per-role editor: a palette's `raw` LS_COLORS string edits as one text
/// field instead of per-key color pickers (the table has no fixed role set to
/// draw individual pickers for — just fixed slots plus an open `*.ext` map).
struct LsColorsNode {
    state: Arc<PaletteState>,
}

impl SettingsNode for LsColorsNode {
    fn anchor(&self) -> (&str, &str) {
        (ID, "Themes")
    }

    fn list(&self, path: &Path) -> io::Result<Vec<Entry>> {
        let reg = self.state.read().ok_or_else(|| io::Error::other("lscolors registry lock poisoned"))?;

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
    state: Arc<PaletteState>,
    /// Memo of the active palette's parsed table, keyed by the options
    /// generation it was built at — `entry_style` runs once per visible entry,
    /// so the raw string is parsed again only when the options changed.
    active_cache: RefCell<Option<(u64, LsColorsTable)>>,
}

impl LsColorsExtension {
    pub fn new() -> Self {
        let env_table = std::env::var("LS_COLORS").map(|raw| parse::parse_ls_colors(&raw)).unwrap_or_default();
        let state = PaletteState {
            registry: RwLock::new(PaletteRegistry::bundled()),
            options: OnceLock::new(),
            synced: AtomicU64::new(0),
        };
        LsColorsExtension { env_table, state: Arc::new(state), active_cache: RefCell::new(None) }
    }
}

impl Default for LsColorsExtension {
    fn default() -> Self {
        Self::new()
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

    fn prelude(&self) -> Option<String> {
        Some(PRELUDE_SCHEME.to_string())
    }

    fn bind_store(&self, store: &Arc<OptionStore>) {
        let _ = self.state.options.set(store.clone());
        self.state.sync();
    }

    fn settings_nodes(&self) -> Vec<Arc<dyn SettingsNode>> {
        vec![Arc::new(LsColorsNode { state: self.state.clone() })]
    }

    fn entry_style(&self, entry: &Entry) -> Option<Style> {
        let store = self.state.options.get()?;
        match store.get_str("lscolors.coloring").as_str() {
            "LS_COLORS" => parse::style_for(&self.env_table, entry),
            "Palette" => {
                let generation = store.generation();
                let stale = self.active_cache.borrow().as_ref().map(|(g, _)| *g != generation).unwrap_or(true);
                if stale {
                    let table = self.state.read()?.resolve_active();
                    *self.active_cache.borrow_mut() = Some((generation, table));
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
                let known = self.state.read().is_some_and(|r| r.get(name).is_some());
                if !known { return vec![]; }
                vec![PaletteState::write_active(name)]
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
                let Some(reg) = self.state.read() else { return vec![]; };
                if reg.get(&name).is_some() { return vec![]; }
                let raw = reg.get(reg.active()).map(|p| p.raw.clone()).unwrap_or_default();
                let new_palette = NamedPalette { name: name.clone(), preset: false, raw };
                vec![
                    PaletteState::write_customs(reg.customs().chain(std::iter::once(&new_palette))),
                    PaletteState::write_active(&name),
                ]
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
                let Some(reg) = self.state.read() else { return vec![]; };
                if reg.get(&new_name).is_some() { return vec![]; }
                let raw = reg.get(&source_name).map(|p| p.raw.clone()).unwrap_or_default();
                let new_palette = NamedPalette { name: new_name.clone(), preset: false, raw };
                vec![
                    PaletteState::write_customs(reg.customs().chain(std::iter::once(&new_palette))),
                    PaletteState::write_active(&new_name),
                ]
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
                let Some(reg) = self.state.read() else { return vec![]; };
                if reg.get(&new_name).is_some() { return vec![]; }
                let Some(old_np) = reg.get(&old_name) else { return vec![]; };
                if old_np.preset { return vec![]; }
                let renamed = NamedPalette { name: new_name.clone(), preset: false, raw: old_np.raw.clone() };
                let customs = reg.customs().map(|p| if p.name == old_name { &renamed } else { p });
                let mut out = vec![PaletteState::write_customs(customs)];
                if reg.active() == old_name {
                    out.push(PaletteState::write_active(&new_name));
                }
                out
            }

            // ── Edit raw string ────────────────────────────────────────────────
            "edit-raw" => {
                let Some(name) = intent.data.as_str() else { return vec![] };
                let Some(reg) = self.state.read() else { return vec![] };
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
                let Some(reg) = self.state.read() else { return vec![]; };
                let Some(np) = reg.get(&name) else { return vec![]; };
                if np.preset { return vec![]; }
                let updated = NamedPalette { name: np.name.clone(), preset: false, raw };
                vec![PaletteState::write_customs(reg.customs().map(|p| if p.name == name { &updated } else { p }))]
            }

            // ── Delete ─────────────────────────────────────────────────────────
            "delete" => {
                let Some(name) = intent.data.as_str() else { return vec![] };
                let is_preset = self.state.read()
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
                let Some(reg) = self.state.read() else { return vec![]; };
                let mut out = vec![PaletteState::write_customs(reg.customs().filter(|p| p.name != name))];
                if reg.active() == name
                    && let Some(first) = reg.names().first()
                {
                    out.push(PaletteState::write_active(first));
                }
                // The deleted palette's directory is gone — return to the Themes root.
                out.push(Intent::NavigateTo(vfs_root().join("settings").join("Extensions").join(ID).join("Themes")));
                out
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
        LsColorsExtension::new()
    }

    fn bound_ext() -> LsColorsExtension {
        let e = ext();
        let store = Arc::new(OptionStore::new(crate::app::defaults::extension_options(&e)));
        e.bind_store(&store);
        e
    }

    /// What the kernel does with a returned `Configure`.
    fn apply(e: &LsColorsExtension, intents: Vec<Intent>) {
        let store = e.state.options.get().unwrap();
        for i in intents {
            if let Intent::Configure(ConfigWrite::SetOption { key, value }) = i {
                assert!(store.set(&key, value), "the lscolors options accept {key}");
            }
        }
    }

    fn run(e: &LsColorsExtension, intent: &str, data: ExtensionValue) {
        let out = e.handle_intent(
            &ExtensionIntent { extension: ID.into(), intent: intent.into(), data },
            &AppState::new(std::env::temp_dir()),
        );
        apply(e, out);
    }

    #[test]
    fn builtin_dark_and_light_presets_parse_and_color_dirs_and_rs_files() {
        let reg = PaletteRegistry::bundled();
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
    fn set_active_writes_the_option_and_the_registry_follows() {
        let e = bound_ext();
        assert_eq!(e.state.read().unwrap().active(), "dark");
        run(&e, "set-active", ExtensionValue::String("light".into()));
        assert_eq!(e.state.read().unwrap().active(), "light");
        assert!(e.entry_style(&entry("x", true)).is_some());
    }

    #[test]
    fn editing_the_active_palette_recolors_without_a_rename() {
        let e = bound_ext();
        run(
            &e,
            "duplicate-apply",
            ExtensionValue::Nil
                .with("source", ExtensionValue::String("dark".into()))
                .with(KEY_ITEM, ExtensionValue::String("mine".into())),
        );
        assert_eq!(e.state.read().unwrap().active(), "mine");
        assert!(e.entry_style(&entry("main.rs", false)).is_some(), "the copy colors .rs like dark");
        run(
            &e,
            "edit-raw-apply",
            ExtensionValue::Nil
                .with("palette", ExtensionValue::String("mine".into()))
                .with(KEY_ITEM, ExtensionValue::String("di=01;34".into())),
        );
        assert!(e.entry_style(&entry("main.rs", false)).is_none(), "the cached table was rebuilt");
    }
}
