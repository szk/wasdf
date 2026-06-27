//! Settings extension: exposes app configuration as a navigable VFS tree under
//! `@/settings`. Categories and leaves are generated dynamically from the shared
//! `OptionStore`. Each leaf shows the current value and opens an inline editor on
//! Enter. Confirmed edits apply live (via `OptionStore::set`) and persist to
//! `~/.config/wasdf/config.scm` inside the managed block (unchanged writer).
//!
//! After the `Options` struct was deleted, this extension is the only writer to
//! the store for live edits. Matcher settings additionally refresh the shared
//! `Arc<Mutex<MatcherConfig>>` so the search worker picks up new weights without a
//! restart.

mod writer;

use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::core::{
    AppState, Candidate, Entry, ExtensionIntent, ExtensionValue, FunctionHint, Intent, Mode,
    OptionKind, OptionStore, SelectSpec, KEY_ITEM,
};
use crate::extension::Extension;
use crate::services::MatcherConfig;
use crate::vfs::{vfs_root, VfsProvider};

use writer::ConfigWriter;

// ── SettingsStore ─────────────────────────────────────────────────────────────

/// Shared between `SettingsProvider` and `SettingsExtension`.
pub struct SettingsStore {
    pub option_store: Arc<OptionStore>,
}

impl SettingsStore {
    pub fn new(option_store: Arc<OptionStore>) -> Self {
        SettingsStore { option_store }
    }
}

// ── SettingsProvider ──────────────────────────────────────────────────────────

pub struct SettingsProvider {
    store: Arc<SettingsStore>,
}

impl SettingsProvider {
    fn new(store: Arc<SettingsStore>) -> Self {
        SettingsProvider { store }
    }
}

impl VfsProvider for SettingsProvider {
    fn mount(&self) -> &str {
        "settings"
    }

    fn list(&self, path: &Path) -> io::Result<Vec<Entry>> {
        let os = &self.store.option_store;
        match crate::vfs::tail_after_mount(path) {
            None => {
                Ok(os
                    .categories()
                    .into_iter()
                    .map(|(cat, _)| {
                        Entry::virtual_dir(
                            vfs_root().join("settings").join(&cat),
                            cat.clone(),
                        )
                    })
                    .collect())
            }
            Some(cat_name) => {
                let cats = os.categories();
                let (_, defs) = cats
                    .iter()
                    .find(|(cat, _)| cat.eq_ignore_ascii_case(cat_name))
                    .ok_or_else(|| {
                        io::Error::new(
                            io::ErrorKind::NotFound,
                            format!("unknown settings category: {cat_name}"),
                        )
                    })?;
                Ok(defs
                    .iter()
                    .map(|def| {
                        let val = os.get_str(&def.key);
                        let mut e = Entry::virtual_leaf(
                            vfs_root().join("settings").join(cat_name).join(&def.key),
                            format!("{}: {}", def.label, val),
                        );
                        e.activation = Some(ExtensionIntent {
                            extension: "settings".into(),
                            intent: "edit".into(),
                            data: ExtensionValue::String(def.key.clone()),
                        });
                        e
                    })
                    .collect())
            }
        }
    }
}

// ── SettingsExtension ─────────────────────────────────────────────────────────

pub struct SettingsExtension {
    store: Arc<SettingsStore>,
    matcher_cfg: Arc<Mutex<MatcherConfig>>,
    writer: Option<ConfigWriter>,
}

impl SettingsExtension {
    pub fn new(
        option_store: Arc<OptionStore>,
        matcher_cfg: Arc<Mutex<MatcherConfig>>,
        config_scm: Option<PathBuf>,
    ) -> Self {
        let store = Arc::new(SettingsStore::new(option_store));
        let writer = config_scm.map(ConfigWriter::new);
        SettingsExtension { store, matcher_cfg, writer }
    }
}

/// Rebuild the typed `MatcherConfig` from current store values. Called after any
/// `matcher.*` key is updated so the search worker picks up new weights.
fn project_matcher(os: &OptionStore, cfg: &Mutex<MatcherConfig>) {
    let mut mc = cfg.lock().unwrap();
    mc.tail_bonus = os.get_i64("matcher.tail-bonus");
    mc.depth_penalty = os.get_i64("matcher.depth-penalty");
    mc.length_penalty = os.get_i64("matcher.length-penalty");
    mc.exec_bonus = os.get_i64("matcher.exec-bonus");
    mc.preferred_ext = os.get_list("matcher.preferred-ext");
    mc.noise_ext = os.get_list("matcher.noise-ext");
    mc.noise_dirs = os.get_list("matcher.noise-dirs");
}

impl Extension for SettingsExtension {
    fn id(&self) -> &str {
        "settings"
    }

    fn vfs_providers(&self) -> Vec<Box<dyn VfsProvider>> {
        vec![Box::new(SettingsProvider::new(self.store.clone()))]
    }

    fn handle_intent(&self, intent: &ExtensionIntent, _state: &AppState) -> Vec<Intent> {
        match intent.intent.as_str() {
            "edit" => {
                let Some(key) = intent.data.as_str() else { return Vec::new() };
                let os = &self.store.option_store;
                let Some(def) = os.def(key) else { return Vec::new() };

                let template = ExtensionIntent {
                    extension: "settings".into(),
                    intent: "set".into(),
                    data: ExtensionValue::Nil
                        .with("field", ExtensionValue::String(key.into())),
                };

                let spec = match &def.kind {
                    OptionKind::Bool => {
                        let candidates = vec![
                            Candidate::new("#t", ExtensionValue::String("#t".into())),
                            Candidate::new("#f", ExtensionValue::String("#f".into())),
                        ];
                        SelectSpec::emit_static(
                            "settings-edit",
                            candidates,
                            template,
                            Some(FunctionHint::Extension("settings".into())),
                        )
                    }
                    OptionKind::Choice(opts) => {
                        let candidates: Vec<Candidate> = opts
                            .iter()
                            .map(|o| Candidate::new(o, ExtensionValue::String(o.clone())))
                            .collect();
                        SelectSpec::emit_static(
                            "settings-edit",
                            candidates,
                            template,
                            Some(FunctionHint::Extension("settings".into())),
                        )
                    }
                    OptionKind::Text | OptionKind::List | OptionKind::Int { .. } => {
                        let current = os.get_str(key);
                        SelectSpec::emit_path_input(
                            "settings-edit",
                            template,
                            Some(current),
                            Some(FunctionHint::Extension("settings".into())),
                        )
                    }
                };

                vec![Intent::PushMode(Box::new(Mode::Select(spec)))]
            }
            "set" => {
                let Some(key) = intent.data.get("field").and_then(|v| v.as_str()) else {
                    return Vec::new();
                };
                let Some(val) = intent.data.get(KEY_ITEM).and_then(|v| v.as_str()) else {
                    return Vec::new();
                };

                if !self.store.option_store.set(key, val) {
                    return Vec::new();
                }

                // Refresh the typed matcher config when a matcher.* key changes.
                if key.starts_with("matcher.") {
                    project_matcher(&self.store.option_store, &self.matcher_cfg);
                }

                if let Some(w) = &self.writer {
                    let _ = w.upsert_option(key, val);
                }

                vec![Intent::Refresh]
            }
            _ => Vec::new(),
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn core_option_defs() -> Vec<crate::core::OptionDef> {
        let src = include_str!("../app/options.scm");
        let outer = crate::script::sexpr::parse(src).expect("parse options.scm");
        let inner = outer.as_list().expect("outer is list")[1].clone();
        crate::script::config::parse_option_config(&inner).expect("parse defs")
    }

    fn make_store() -> Arc<SettingsStore> {
        let option_store = Arc::new(crate::core::OptionStore::new(core_option_defs()));
        Arc::new(SettingsStore::new(option_store))
    }

    fn make_ext() -> SettingsExtension {
        let option_store = Arc::new(crate::core::OptionStore::new(core_option_defs()));
        SettingsExtension::new(
            option_store,
            Arc::new(Mutex::new(MatcherConfig::default())),
            None,
        )
    }

    #[test]
    fn settings_provider_lists_categories_from_store() {
        let store = make_store();
        let p = SettingsProvider::new(store);
        let entries = p.list(Path::new("@/settings")).unwrap();
        assert!(entries.len() >= 3, "at least 3 categories");
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"Display"));
        assert!(names.contains(&"Matcher"));
        assert!(names.contains(&"Layout"));
        assert!(entries.iter().all(|e| e.is_dir));
    }

    #[test]
    fn settings_provider_display_category_has_leaves_with_activation() {
        let store = make_store();
        let p = SettingsProvider::new(store);
        let entries = p.list(Path::new("@/settings/Display")).unwrap();
        assert!(entries.len() >= 1, "Display has fields");
        for e in &entries {
            assert!(!e.is_dir, "fields are leaves");
            let act = e.activation.as_ref().expect("leaf has activation");
            assert_eq!(act.extension, "settings");
            assert_eq!(act.intent, "edit");
            assert!(act.data.as_str().is_some(), "data is the key string");
        }
        let bs = entries.iter().find(|e| e.name.contains("border-style")).unwrap();
        assert!(bs.name.contains("single"), "default border-style is single: {:?}", bs.name);
    }

    #[test]
    fn handle_edit_border_style_pushes_select_mode() {
        let ext = make_ext();
        let state = AppState::new(PathBuf::from("/tmp"));
        let intent = ExtensionIntent {
            extension: "settings".into(),
            intent: "edit".into(),
            data: ExtensionValue::String("border-style".into()),
        };
        let result = ext.handle_intent(&intent, &state);
        assert_eq!(result.len(), 1);
        assert!(matches!(&result[0], Intent::PushMode(m) if matches!(**m, Mode::Select(_))));
    }

    #[test]
    fn handle_set_applies_live_and_returns_refresh() {
        let option_store = Arc::new(crate::core::OptionStore::new(core_option_defs()));
        let ext = SettingsExtension::new(
            option_store.clone(),
            Arc::new(Mutex::new(MatcherConfig::default())),
            None,
        );
        let state = AppState::new(PathBuf::from("/tmp"));
        let intent = ExtensionIntent {
            extension: "settings".into(),
            intent: "set".into(),
            data: ExtensionValue::Nil
                .with("field", ExtensionValue::String("border-style".into()))
                .with(KEY_ITEM, ExtensionValue::String("rounded".into())),
        };
        let result = ext.handle_intent(&intent, &state);
        assert_eq!(result, vec![Intent::Refresh]);
        assert_eq!(option_store.get_str("border-style"), "rounded", "store updated");
    }

    #[test]
    fn matcher_category_has_fields_with_activation() {
        let store = make_store();
        let p = SettingsProvider::new(store);
        let entries = p.list(Path::new("@/settings/Matcher")).unwrap();
        assert!(entries.len() >= 7, "Matcher has at least 7 fields");
        for e in &entries {
            assert!(!e.is_dir, "fields are leaves");
            let act = e.activation.as_ref().expect("leaf has activation");
            assert_eq!(act.extension, "settings");
            assert_eq!(act.intent, "edit");
        }
        let tb = entries.iter().find(|e| e.name.contains("tail-bonus")).unwrap();
        assert!(tb.name.contains("16"), "default tail-bonus is 16: {:?}", tb.name);
    }

    #[test]
    fn handle_set_matcher_numeric_field_applies_live() {
        let option_store = Arc::new(crate::core::OptionStore::new(core_option_defs()));
        let matcher_cfg = Arc::new(Mutex::new(MatcherConfig::default()));
        let ext = SettingsExtension::new(option_store.clone(), matcher_cfg.clone(), None);
        let state = AppState::new(PathBuf::from("/tmp"));
        let intent = ExtensionIntent {
            extension: "settings".into(),
            intent: "set".into(),
            data: ExtensionValue::Nil
                .with("field", ExtensionValue::String("matcher.tail-bonus".into()))
                .with(KEY_ITEM, ExtensionValue::String("32".into())),
        };
        let result = ext.handle_intent(&intent, &state);
        assert_eq!(result, vec![Intent::Refresh]);
        assert_eq!(option_store.get_i64("matcher.tail-bonus"), 32);
        assert_eq!(matcher_cfg.lock().unwrap().tail_bonus, 32, "matcher cfg refreshed");
    }

    #[test]
    fn handle_set_matcher_numeric_invalid_value_is_ignored() {
        let option_store = Arc::new(crate::core::OptionStore::new(core_option_defs()));
        let matcher_cfg = Arc::new(Mutex::new(MatcherConfig::default()));
        let ext = SettingsExtension::new(option_store, matcher_cfg.clone(), None);
        let state = AppState::new(PathBuf::from("/tmp"));
        let intent = ExtensionIntent {
            extension: "settings".into(),
            intent: "set".into(),
            data: ExtensionValue::Nil
                .with("field", ExtensionValue::String("matcher.tail-bonus".into()))
                .with(KEY_ITEM, ExtensionValue::String("not-a-number".into())),
        };
        let result = ext.handle_intent(&intent, &state);
        assert!(result.is_empty(), "invalid numeric value returns nothing");
        assert_eq!(matcher_cfg.lock().unwrap().tail_bonus, 16, "value unchanged");
    }

    #[test]
    fn handle_set_invalid_choice_is_ignored() {
        let option_store = Arc::new(crate::core::OptionStore::new(core_option_defs()));
        let ext = SettingsExtension::new(
            option_store.clone(),
            Arc::new(Mutex::new(MatcherConfig::default())),
            None,
        );
        let state = AppState::new(PathBuf::from("/tmp"));
        let intent = ExtensionIntent {
            extension: "settings".into(),
            intent: "set".into(),
            data: ExtensionValue::Nil
                .with("field", ExtensionValue::String("border-style".into()))
                .with(KEY_ITEM, ExtensionValue::String("neon-pink".into())),
        };
        let result = ext.handle_intent(&intent, &state);
        assert!(result.is_empty(), "invalid choice returns nothing");
        assert_eq!(option_store.get_str("border-style"), "single", "option unchanged");
    }

    #[test]
    fn handle_edit_unknown_key_returns_nothing() {
        let ext = make_ext();
        let state = AppState::new(PathBuf::from("/tmp"));
        let intent = ExtensionIntent {
            extension: "settings".into(),
            intent: "edit".into(),
            data: ExtensionValue::String("no-such-key".into()),
        };
        assert!(ext.handle_intent(&intent, &state).is_empty());
    }
}
