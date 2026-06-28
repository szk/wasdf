//! The `theme` bundled extension: registry, presets, selection, and authoring.

pub mod registry;
pub mod writer;

pub use registry::{NamedTheme, ThemeRegistry};

use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use crate::core::{
    AppState, Candidate, Entry, ExtensionIntent, ExtensionValue, Intent, Mode, Role, SelectSpec,
    KEY_ITEM,
};
use crate::extension::settings::SettingsNode;
use crate::extension::Extension;
use crate::vfs::vfs_root;

use writer::ThemeWriter;

/// The raw preset scheme source (include_str! at compile time).
pub const PRESETS_SCHEME: &str = include_str!("theme_presets.scm");

/// The 16 standard ANSI color names offered in the role color picker.
const ANSI_COLORS: &[&str] = &[
    "black", "red", "green", "yellow", "blue", "magenta", "cyan", "gray",
    "darkgray", "lightred", "lightgreen", "lightyellow", "lightblue",
    "lightmagenta", "lightcyan", "white",
];

// ── ThemeNode ─────────────────────────────────────────────────────────────────

/// Serves the `@/settings/Display/Themes` subtree via the `SettingsNode` graft.
struct ThemeNode {
    registry: Arc<RwLock<ThemeRegistry>>,
}

impl SettingsNode for ThemeNode {
    fn anchor(&self) -> (&str, &str) {
        ("Display", "Themes")
    }

    fn list(&self, path: &Path) -> io::Result<Vec<Entry>> {
        let reg = self.registry.read().map_err(|e| {
            io::Error::new(io::ErrorKind::Other, format!("theme registry lock poisoned: {e}"))
        })?;

        let depth = path.components().count();
        // @/settings/Display/Themes → depth 4
        // @/settings/Display/Themes/<name> → depth 5
        let base = vfs_root().join("settings").join("Display").join("Themes");

        if depth <= 4 {
            // Root of Themes: one dir entry per theme, then `+ New theme…`.
            let active = reg.active().to_string();
            let mut entries: Vec<Entry> = reg.names().into_iter().map(|name| {
                let nt = reg.get(&name).unwrap();
                let kind = if nt.preset { "preset" } else { "custom" };
                let badge = if name == active { format!("{name} ✓ ({kind})") } else { format!("{name} ({kind})") };
                Entry::virtual_dir(base.join(&name), badge)
            }).collect();
            let mut new_leaf = Entry::virtual_leaf(base.join("+ New theme…"), "+ New theme…".into());
            new_leaf.activation = Some(ExtensionIntent {
                extension: "theme".into(),
                intent: "new".into(),
                data: ExtensionValue::Nil,
            });
            entries.push(new_leaf);
            Ok(entries)
        } else {
            // Theme dir: get the theme name from component 4.
            let theme_name = path.components().nth(4)
                .and_then(|c| if let std::path::Component::Normal(s) = c { s.to_str() } else { None })
                .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "missing theme name in path"))?;

            let nt = reg.get(theme_name).ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, format!("theme '{theme_name}' not found"))
            })?;
            let is_preset = nt.preset;
            let active = reg.active().to_string();
            let resolved = reg.resolve(theme_name);
            let theme_base = base.join(theme_name);
            let mut entries: Vec<Entry> = Vec::new();

            // "Set active" leaf (omit if already active).
            if theme_name != active {
                let mut e = Entry::virtual_leaf(theme_base.join("set-active"), "Set active".into());
                e.activation = Some(ExtensionIntent {
                    extension: "theme".into(),
                    intent: "set-active".into(),
                    data: ExtensionValue::String(theme_name.to_string()),
                });
                entries.push(e);
            }

            // One leaf per role showing the current color.
            if is_preset {
                let mut header = Entry::virtual_leaf(theme_base.join("(preset)"), "preset — duplicate to edit".into());
                header.activation = Some(ExtensionIntent {
                    extension: "theme".into(),
                    intent: "duplicate".into(),
                    data: ExtensionValue::String(theme_name.to_string()),
                });
                entries.push(header);
            }
            for role in Role::ALL {
                let tok = role.token();
                let color = crate::core::color_token(resolved.role(*role));
                let label = format!("{tok}: {color}");
                let mut e = Entry::virtual_leaf(theme_base.join(tok), label);
                e.activation = Some(ExtensionIntent {
                    extension: "theme".into(),
                    intent: if is_preset { "duplicate" } else { "edit-role" }.to_string(),
                    data: ExtensionValue::Nil
                        .with("theme", ExtensionValue::String(theme_name.to_string()))
                        .with("role", ExtensionValue::String(tok.to_string())),
                });
                entries.push(e);
            }

            // Duplicate, Rename (custom only), Delete (custom only).
            let mut dup = Entry::virtual_leaf(theme_base.join("duplicate"), "Duplicate…".into());
            dup.activation = Some(ExtensionIntent {
                extension: "theme".into(),
                intent: "duplicate".into(),
                data: ExtensionValue::String(theme_name.to_string()),
            });
            entries.push(dup);

            if !is_preset {
                let mut rename = Entry::virtual_leaf(theme_base.join("rename"), "Rename…".into());
                rename.activation = Some(ExtensionIntent {
                    extension: "theme".into(),
                    intent: "rename".into(),
                    data: ExtensionValue::String(theme_name.to_string()),
                });
                entries.push(rename);

                let mut del = Entry::virtual_leaf(theme_base.join("delete"), "Delete".into());
                del.activation = Some(ExtensionIntent {
                    extension: "theme".into(),
                    intent: "delete".into(),
                    data: ExtensionValue::String(theme_name.to_string()),
                });
                entries.push(del);
            }

            Ok(entries)
        }
    }
}

// ── ThemeExtension ────────────────────────────────────────────────────────────

pub struct ThemeExtension {
    pub registry: Arc<RwLock<ThemeRegistry>>,
    writer: Option<ThemeWriter>,
}

impl ThemeExtension {
    pub fn new(registry: Arc<RwLock<ThemeRegistry>>, config_scm: Option<PathBuf>) -> Self {
        if let Some(ref path) = config_scm {
            if let Ok(text) = std::fs::read_to_string(path) {
                boot_load_config(&registry, &text);
            }
        }
        let writer = config_scm.map(ThemeWriter::new);
        ThemeExtension { registry, writer }
    }
}

/// Parse the managed block in `text` for `(define-theme …)` and `(set-theme …)` forms,
/// loading customs first so a custom active name is recognized by `set_active`.
fn boot_load_config(registry: &Arc<RwLock<ThemeRegistry>>, text: &str) {
    let lines: Vec<&str> = text.lines().collect();
    let begin = lines.iter().position(|l| l.trim() == writer::BLOCK_BEGIN);
    let end = lines.iter().position(|l| l.trim() == writer::BLOCK_END);
    let (Some(b), Some(e)) = (begin, end) else { return };
    if b >= e { return; }
    let block = &lines[b + 1..e];

    let mut customs = Vec::new();
    let mut active_name: Option<String> = None;
    let mut i = 0;

    while i < block.len() {
        let t = block[i].trim();
        if t.starts_with("(define-theme ") {
            // Multi-line form: count parens to find the closing line.
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
                if let Some(nt) = crate::script::config::parse_define_theme(&datum) {
                    customs.push(nt);
                }
            }
            i = end_i + 1;
        } else if t.starts_with("(set-theme ") {
            if let Ok(datum) = crate::script::sexpr::parse(t) {
                if let Some(name) = crate::script::config::parse_set_theme(&datum) {
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

impl Extension for ThemeExtension {
    fn id(&self) -> &str {
        "theme"
    }

    fn settings_nodes(&self) -> Vec<Arc<dyn SettingsNode>> {
        vec![Arc::new(ThemeNode { registry: self.registry.clone() })]
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

            // ── New theme ──────────────────────────────────────────────────────
            "new" => {
                let template = ExtensionIntent {
                    extension: "theme".into(),
                    intent: "new-apply".into(),
                    data: ExtensionValue::Nil,
                };
                let spec = SelectSpec::emit_path_input("theme-new", template, None, None);
                vec![Intent::PushMode(Box::new(Mode::Select(spec)))]
            }
            "new-apply" => {
                let Some(name_raw) = intent.data.get(KEY_ITEM).and_then(|v| v.as_str()) else {
                    return vec![];
                };
                let name = name_raw.trim().to_string();
                if name.is_empty() { return vec![]; }
                let new_theme = {
                    let Ok(reg) = self.registry.read() else { return vec![]; };
                    if reg.get(&name).is_some() { return vec![]; }
                    let active = reg.active().to_string();
                    let seed = reg.get(&active).map(|nt| nt.overrides.clone()).unwrap_or_default();
                    NamedTheme { name: name.clone(), preset: false, overrides: seed }
                };
                {
                    let Ok(mut reg) = self.registry.write() else { return vec![]; };
                    reg.upsert_custom(new_theme.clone());
                    reg.set_active(&name);
                }
                if let Some(w) = &self.writer {
                    let _ = w.upsert_theme(&new_theme);
                    let _ = w.set_active(&name);
                }
                vec![Intent::Refresh]
            }

            // ── Duplicate ──────────────────────────────────────────────────────
            "duplicate" => {
                let source = match intent.data.as_str().map(str::to_string)
                    .or_else(|| intent.data.get("theme").and_then(|v| v.as_str()).map(str::to_string))
                {
                    Some(s) => s,
                    None => return vec![],
                };
                let template = ExtensionIntent {
                    extension: "theme".into(),
                    intent: "duplicate-apply".into(),
                    data: ExtensionValue::Nil
                        .with("source", ExtensionValue::String(source)),
                };
                let spec = SelectSpec::emit_path_input("theme-duplicate", template, None, None);
                vec![Intent::PushMode(Box::new(Mode::Select(spec)))]
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
                let new_theme = {
                    let Ok(reg) = self.registry.read() else { return vec![]; };
                    if reg.get(&new_name).is_some() { return vec![]; }
                    let seed = reg.get(&source_name)
                        .map(|nt| nt.overrides.clone())
                        .unwrap_or_default();
                    NamedTheme { name: new_name.clone(), preset: false, overrides: seed }
                };
                {
                    let Ok(mut reg) = self.registry.write() else { return vec![]; };
                    reg.upsert_custom(new_theme.clone());
                    reg.set_active(&new_name);
                }
                if let Some(w) = &self.writer {
                    let _ = w.upsert_theme(&new_theme);
                    let _ = w.set_active(&new_name);
                }
                vec![Intent::Refresh]
            }

            // ── Rename ─────────────────────────────────────────────────────────
            "rename" => {
                let Some(old_name) = intent.data.as_str() else { return vec![] };
                let template = ExtensionIntent {
                    extension: "theme".into(),
                    intent: "rename-apply".into(),
                    data: ExtensionValue::Nil
                        .with("source", ExtensionValue::String(old_name.to_string())),
                };
                let spec = SelectSpec::emit_path_input("theme-rename", template, None, None);
                vec![Intent::PushMode(Box::new(Mode::Select(spec)))]
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
                    let Some(old_nt) = reg.get(&old_name) else { return vec![]; };
                    if old_nt.preset { return vec![]; }
                    let was_active = reg.active() == old_name;
                    let renamed = NamedTheme {
                        name: new_name.clone(),
                        preset: false,
                        overrides: old_nt.overrides.clone(),
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
                    let _ = w.remove_theme(&old_name);
                    let _ = w.upsert_theme(&renamed);
                    if was_active { let _ = w.set_active(&new_name); }
                }
                vec![Intent::Refresh]
            }

            // ── Edit role color ────────────────────────────────────────────────
            "edit-role" => {
                let Some(theme_name) = intent.data.get("theme").and_then(|v| v.as_str()) else {
                    return vec![];
                };
                let Some(role_tok) = intent.data.get("role").and_then(|v| v.as_str()) else {
                    return vec![];
                };
                let is_preset = self.registry.read().ok()
                    .and_then(|r| r.get(theme_name).map(|nt| nt.preset))
                    .unwrap_or(true);
                if is_preset { return vec![]; }
                let template = ExtensionIntent {
                    extension: "theme".into(),
                    intent: "edit-role-apply".into(),
                    data: ExtensionValue::Nil
                        .with("theme", ExtensionValue::String(theme_name.to_string()))
                        .with("role", ExtensionValue::String(role_tok.to_string())),
                };
                let candidates: Vec<Candidate> =
                    std::iter::once(Candidate::new(
                        "default",
                        ExtensionValue::String("default".to_string()),
                    ))
                    .chain(ANSI_COLORS.iter()
                        .map(|&n| Candidate::new(n, ExtensionValue::String(n.to_string()))))
                    .chain(std::iter::once(Candidate::new(
                        "custom #hex…",
                        ExtensionValue::String("custom".to_string()),
                    )))
                    .collect();
                let spec = SelectSpec::emit_static("theme-edit-role", candidates, template, None);
                vec![Intent::PushMode(Box::new(Mode::Select(spec)))]
            }
            "edit-role-apply" => {
                let Some(theme_name) = intent.data.get("theme").and_then(|v| v.as_str()) else {
                    return vec![];
                };
                let Some(role_tok) = intent.data.get("role").and_then(|v| v.as_str()) else {
                    return vec![];
                };
                let Some(color_str) = intent.data.get(KEY_ITEM).and_then(|v| v.as_str()) else {
                    return vec![];
                };
                if color_str == "custom" {
                    let hex_template = ExtensionIntent {
                        extension: "theme".into(),
                        intent: "edit-role-hex-apply".into(),
                        data: ExtensionValue::Nil
                            .with("theme", ExtensionValue::String(theme_name.to_string()))
                            .with("role", ExtensionValue::String(role_tok.to_string())),
                    };
                    let spec = SelectSpec::emit_path_input(
                        "theme-edit-role-hex", hex_template, None, None,
                    );
                    return vec![Intent::PushMode(Box::new(Mode::Select(spec)))];
                }
                self.apply_role_color(theme_name, role_tok, color_str)
            }
            "edit-role-hex-apply" => {
                let Some(theme_name) = intent.data.get("theme").and_then(|v| v.as_str()) else {
                    return vec![];
                };
                let Some(role_tok) = intent.data.get("role").and_then(|v| v.as_str()) else {
                    return vec![];
                };
                let Some(hex_str) = intent.data.get(KEY_ITEM).and_then(|v| v.as_str()) else {
                    return vec![];
                };
                self.apply_role_color(theme_name, role_tok, hex_str)
            }

            // ── Delete ─────────────────────────────────────────────────────────
            "delete" => {
                let Some(name) = intent.data.as_str() else { return vec![] };
                let is_preset = self.registry.read().ok()
                    .and_then(|r| r.get(name).map(|nt| nt.preset))
                    .unwrap_or(true);
                if is_preset { return vec![]; }
                let confirm_intent = Intent::Extension(ExtensionIntent {
                    extension: "theme".into(),
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
                if let Some(w) = &self.writer { let _ = w.remove_theme(name); }
                if was_active {
                    let _ = self.registry.write().map(|mut r| r.set_active("default"));
                    if let Some(w) = &self.writer { let _ = w.set_active("default"); }
                }
                // The deleted theme's directory is gone — return to the Themes root.
                let themes_root = vfs_root().join("settings").join("Display").join("Themes");
                vec![Intent::NavigateTo(themes_root)]
            }

            _ => Vec::new(),
        }
    }
}

impl ThemeExtension {
    /// Read `color_str` via `parse_color`, set or remove the role override in
    /// the named custom theme, then persist.
    fn apply_role_color(&self, theme_name: &str, role_tok: &str, color_str: &str) -> Vec<Intent> {
        let Some(role) = Role::from_token(role_tok) else { return vec![]; };
        let Some(color) = crate::core::parse_color(color_str) else { return vec![]; };
        let default_color = crate::core::Theme::default().role(role);

        let updated = {
            let Ok(reg) = self.registry.read() else { return vec![]; };
            let Some(nt) = reg.get(theme_name) else { return vec![]; };
            if nt.preset { return vec![]; }
            let mut new_overrides = nt.overrides.clone();
            if color == default_color {
                new_overrides.retain(|(r, _)| *r != role);
            } else if let Some(pos) = new_overrides.iter().position(|(r, _)| *r == role) {
                new_overrides[pos].1 = color;
            } else {
                new_overrides.push((role, color));
            }
            NamedTheme { name: nt.name.clone(), preset: false, overrides: new_overrides }
        };
        { let Ok(mut reg) = self.registry.write() else { return vec![]; }; reg.upsert_custom(updated.clone()); }
        if let Some(w) = &self.writer { let _ = w.upsert_theme(&updated); }
        vec![Intent::Refresh]
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{AppState, Intent, KEY_ITEM};
    use std::sync::atomic::{AtomicU32, Ordering};

    fn temp_path() -> PathBuf {
        static SEQ: AtomicU32 = AtomicU32::new(0);
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir()
            .join(format!("wasdf-theme-ext-{}-{}", std::process::id(), n));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("config.scm")
    }

    fn presets_registry() -> Arc<RwLock<ThemeRegistry>> {
        let datum = crate::script::sexpr::parse(PRESETS_SCHEME).unwrap();
        let inner = datum.as_list().and_then(|l| l.get(1)).cloned()
            .unwrap_or(crate::script::sexpr::Datum::List(vec![]));
        let presets = crate::script::config::parse_theme_config(&inner);
        Arc::new(RwLock::new(ThemeRegistry::with_presets(presets)))
    }

    #[test]
    fn themes_node_lists_presets_and_new_entry() {
        let registry = presets_registry();
        let node = ThemeNode { registry };
        let entries = node.list(Path::new("@/settings/Display/Themes")).unwrap();
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert!(names.iter().any(|n| n.contains("default")), "default present: {names:?}");
        assert!(names.iter().any(|n| n.contains("nord")));
        assert!(names.iter().any(|n| n.contains("gruvbox")));
        assert!(names.iter().any(|n| n.contains("light")));
        assert!(names.contains(&"+ New theme…"), "new-theme entry present");
    }

    #[test]
    fn set_active_intent_updates_registry() {
        let registry = presets_registry();
        let ext = ThemeExtension::new(registry.clone(), None);
        let state = AppState::new(PathBuf::from("/tmp"));
        let intent = ExtensionIntent {
            extension: "theme".into(),
            intent: "set-active".into(),
            data: ExtensionValue::String("nord".into()),
        };
        let result = ext.handle_intent(&intent, &state);
        assert_eq!(result, vec![Intent::Refresh]);
        assert_eq!(registry.read().unwrap().active(), "nord");
    }

    #[test]
    fn set_active_unknown_name_returns_nothing() {
        let registry = presets_registry();
        let ext = ThemeExtension::new(registry.clone(), None);
        let state = AppState::new(PathBuf::from("/tmp"));
        let intent = ExtensionIntent {
            extension: "theme".into(),
            intent: "set-active".into(),
            data: ExtensionValue::String("nope".into()),
        };
        let result = ext.handle_intent(&intent, &state);
        assert!(result.is_empty());
        assert_eq!(registry.read().unwrap().active(), "default");
    }

    #[test]
    fn set_active_persists_to_config() {
        let path = temp_path();
        let registry = presets_registry();
        let ext = ThemeExtension::new(registry.clone(), Some(path.clone()));
        let state = AppState::new(PathBuf::from("/tmp"));
        let intent = ExtensionIntent {
            extension: "theme".into(),
            intent: "set-active".into(),
            data: ExtensionValue::String("gruvbox".into()),
        };
        ext.handle_intent(&intent, &state);
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains(r#"(set-theme "gruvbox")"#));
    }

    #[test]
    fn boot_load_reads_set_theme_from_config() {
        let path = temp_path();
        std::fs::write(
            &path,
            format!(";; other\n{}\n(set-theme \"nord\")\n{}\n",
                writer::BLOCK_BEGIN, writer::BLOCK_END),
        ).unwrap();
        let registry = presets_registry();
        let _ext = ThemeExtension::new(registry.clone(), Some(path));
        assert_eq!(registry.read().unwrap().active(), "nord");
    }

    #[test]
    fn new_theme_creates_custom_and_selects_it() {
        let path = temp_path();
        let registry = presets_registry();
        let ext = ThemeExtension::new(registry.clone(), Some(path.clone()));
        let state = AppState::new(PathBuf::from("/tmp"));
        // Simulate confirm from emit_path_input: KEY_ITEM = "ocean"
        let intent = ExtensionIntent {
            extension: "theme".into(),
            intent: "new-apply".into(),
            data: ExtensionValue::Nil.with(KEY_ITEM, ExtensionValue::String("ocean".into())),
        };
        let result = ext.handle_intent(&intent, &state);
        assert_eq!(result, vec![Intent::Refresh]);
        assert_eq!(registry.read().unwrap().active(), "ocean");
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("define-theme"), "define-theme persisted");
        assert!(content.contains("ocean"), "theme name in config");
        assert!(content.contains(r#"(set-theme "ocean")"#), "active persisted");
    }

    #[test]
    fn edit_role_updates_override_and_persists() {
        let path = temp_path();
        let registry = presets_registry();
        let ext = ThemeExtension::new(registry.clone(), Some(path.clone()));
        let state = AppState::new(PathBuf::from("/tmp"));
        // First create a custom theme.
        ext.handle_intent(&ExtensionIntent {
            extension: "theme".into(),
            intent: "new-apply".into(),
            data: ExtensionValue::Nil.with(KEY_ITEM, ExtensionValue::String("ocean".into())),
        }, &state);
        // Then edit border role to "blue".
        let intent = ExtensionIntent {
            extension: "theme".into(),
            intent: "edit-role-apply".into(),
            data: ExtensionValue::Nil
                .with("theme", ExtensionValue::String("ocean".into()))
                .with("role", ExtensionValue::String("border".into()))
                .with(KEY_ITEM, ExtensionValue::String("blue".into())),
        };
        let result = ext.handle_intent(&intent, &state);
        assert_eq!(result, vec![Intent::Refresh]);
        // Check override in registry.
        let resolved = registry.read().unwrap().resolve("ocean");
        assert_eq!(resolved.role(crate::core::Role::Border), ratatui::style::Color::Blue);
        // Check config.scm.
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("(border blue)"), "role override persisted: {content}");
    }

    #[test]
    fn delete_custom_resets_active_to_default() {
        let path = temp_path();
        let registry = presets_registry();
        let ext = ThemeExtension::new(registry.clone(), Some(path.clone()));
        let state = AppState::new(PathBuf::from("/tmp"));
        // Create a custom and make it active.
        ext.handle_intent(&ExtensionIntent {
            extension: "theme".into(),
            intent: "new-apply".into(),
            data: ExtensionValue::Nil.with(KEY_ITEM, ExtensionValue::String("ocean".into())),
        }, &state);
        assert_eq!(registry.read().unwrap().active(), "ocean");
        // Delete-confirm fires the wrapped intent on confirm.
        let intent = ExtensionIntent {
            extension: "theme".into(),
            intent: "delete-confirm".into(),
            data: ExtensionValue::String("ocean".into()),
        };
        let result = ext.handle_intent(&intent, &state);
        let themes_root = vfs_root().join("settings").join("Display").join("Themes");
        assert_eq!(result, vec![Intent::NavigateTo(themes_root)]);
        assert_eq!(registry.read().unwrap().active(), "default");
        assert!(!registry.read().unwrap().names().contains(&"ocean".to_string()));
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(!content.contains("(define-theme \"ocean\""), "form removed from config");
    }
}
