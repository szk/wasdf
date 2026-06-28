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

// ── SettingsNode ──────────────────────────────────────────────────────────────

/// A navigable subtree grafted under a `@/settings/<category>` path. An
/// extension implements this to serve a rich sub-panel (e.g. Themes) that the
/// `SettingsProvider` cannot enumerate from the `OptionStore` alone.
///
/// Activation leaves carry `ExtensionIntent { extension: "<owner>", … }` so
/// the normal dispatch routes them back to the extension that owns the node.
pub trait SettingsNode: Send + Sync {
    /// Where this node grafts: `(category, node-name)`, e.g. `("Display","Themes")`.
    fn anchor(&self) -> (&str, &str);
    /// List the children of `path`, which is at or below
    /// `@/settings/<category>/<node>`. The node owns all entries at this
    /// depth and deeper.
    fn list(&self, path: &Path) -> io::Result<Vec<Entry>>;
    /// Whether this node serves the given store option key itself (so the flat
    /// category listing must not also surface it as a leaf). Default: none — a
    /// node that does not reorganise store options leaves the flat listing alone.
    fn owns_key(&self, _key: &str) -> bool {
        false
    }
}

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
    nodes: Vec<Arc<dyn SettingsNode>>,
}

impl SettingsProvider {
    fn new(store: Arc<SettingsStore>, nodes: Vec<Arc<dyn SettingsNode>>) -> Self {
        SettingsProvider { store, nodes }
    }
}

/// The 4th path component of a virtual settings path:
/// `@/settings/<cat>/<node>[/…]` → `Some("node")`.
fn node_name_for(path: &Path) -> Option<&str> {
    use std::path::Component;
    path.components().nth(3).and_then(|c| {
        if let Component::Normal(s) = c { s.to_str() } else { None }
    })
}

impl VfsProvider for SettingsProvider {
    fn mount(&self) -> &str {
        "settings"
    }

    fn list(&self, path: &Path) -> io::Result<Vec<Entry>> {
        let os = &self.store.option_store;
        match (crate::vfs::tail_after_mount(path), node_name_for(path)) {
            // Root: list all settings categories.
            (None, _) => {
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
            // Category depth: list option leaves + one dir per grafted node.
            (Some(cat_name), None) => {
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
                let mut entries: Vec<Entry> = defs
                    .iter()
                    // A key reorganised by a grafted node (e.g. monitor.* under the
                    // Monitor node) must not also appear as a flat category leaf.
                    .filter(|def| !self.nodes.iter().any(|n| n.owns_key(&def.key)))
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
                    .collect();
                // Append a virtual dir for each node anchored to this category.
                for node in &self.nodes {
                    let (nc, nn) = node.anchor();
                    if nc.eq_ignore_ascii_case(cat_name) {
                        entries.push(Entry::virtual_dir(
                            vfs_root().join("settings").join(cat_name).join(nn),
                            nn.to_string(),
                        ));
                    }
                }
                Ok(entries)
            }
            // Deeper: delegate to the matching node.
            (Some(cat_name), Some(node_n)) => {
                let node = self.nodes.iter().find(|n| {
                    let (nc, nn) = n.anchor();
                    nc.eq_ignore_ascii_case(cat_name) && nn.eq_ignore_ascii_case(node_n)
                });
                match node {
                    Some(n) => n.list(path),
                    None => Err(io::Error::new(
                        io::ErrorKind::NotFound,
                        format!("no settings node at {cat_name}/{node_n}"),
                    )),
                }
            }
        }
    }
}

// ── MonitorNode ───────────────────────────────────────────────────────────────

/// Grafts `@/settings/Layout/Monitor`, organising the flat `monitor.<metric>.*`
/// store options into one dir per metric (CPU / Memory / Disk / Network), each
/// listing its settings as ordinary option-edit leaves. Backed by the same
/// `OptionStore`, so editing and persistence reuse the standard settings flow.
struct MonitorNode {
    store: Arc<OptionStore>,
}

impl MonitorNode {
    /// (display name, key segment) for each metric, in display order.
    const METRICS: &'static [(&'static str, &'static str)] =
        &[("CPU", "cpu"), ("Memory", "mem"), ("Disk", "dsk"), ("Network", "net")];
    /// The per-metric setting key suffixes, in display order.
    const SETTINGS: &'static [&'static str] =
        &["visible", "bar", "label", "label-max", "refresh", "graph-window", "bar-kind"];
}

impl SettingsNode for MonitorNode {
    fn anchor(&self) -> (&str, &str) {
        ("Layout", "Monitor")
    }

    fn owns_key(&self, key: &str) -> bool {
        key.starts_with("monitor.")
    }

    fn list(&self, path: &Path) -> io::Result<Vec<Entry>> {
        let base = vfs_root().join("settings").join("Layout").join("Monitor");
        let depth = path.components().count();
        // @/settings/Layout/Monitor → depth 4 (the metric dirs).
        if depth <= 4 {
            return Ok(Self::METRICS
                .iter()
                .map(|(disp, _)| Entry::virtual_dir(base.join(disp), disp.to_string()))
                .collect());
        }
        // @/settings/Layout/Monitor/<Metric> → the metric's setting leaves.
        let metric_disp = path
            .components()
            .nth(4)
            .and_then(|c| if let std::path::Component::Normal(s) = c { s.to_str() } else { None })
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "missing metric in path"))?;
        let metric_key = Self::METRICS
            .iter()
            .find(|(disp, _)| disp.eq_ignore_ascii_case(metric_disp))
            .map(|(_, key)| *key)
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, format!("unknown metric: {metric_disp}"))
            })?;

        let mbase = base.join(metric_disp);
        let mut entries = Vec::new();
        for setting in Self::SETTINGS {
            let key = format!("monitor.{metric_key}.{setting}");
            let Some(def) = self.store.def(&key) else { continue };
            let val = self.store.get_str(&key);
            let mut e = Entry::virtual_leaf(mbase.join(setting), format!("{}: {}", def.label, val));
            e.activation = Some(ExtensionIntent {
                extension: "settings".into(),
                intent: "edit".into(),
                data: ExtensionValue::String(key),
            });
            entries.push(e);
        }
        Ok(entries)
    }
}

// ── SettingsExtension ─────────────────────────────────────────────────────────

pub struct SettingsExtension {
    store: Arc<SettingsStore>,
    matcher_cfg: Arc<Mutex<MatcherConfig>>,
    nodes: Vec<Arc<dyn SettingsNode>>,
    writer: Option<ConfigWriter>,
}

impl SettingsExtension {
    pub fn new(
        option_store: Arc<OptionStore>,
        matcher_cfg: Arc<Mutex<MatcherConfig>>,
        nodes: Vec<Arc<dyn SettingsNode>>,
        config_scm: Option<PathBuf>,
    ) -> Self {
        let store = Arc::new(SettingsStore::new(option_store));
        let writer = config_scm.map(ConfigWriter::new);
        // The Monitor node is settings-owned (it reorganises this store's own
        // monitor.* options), so it is grafted here rather than collected in kernel.
        let mut nodes = nodes;
        nodes.push(Arc::new(MonitorNode { store: store.option_store.clone() }));
        SettingsExtension { store, matcher_cfg, nodes, writer }
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
        vec![Box::new(SettingsProvider::new(self.store.clone(), self.nodes.clone()))]
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
            Vec::new(),
            None,
        )
    }

    #[test]
    fn settings_provider_lists_categories_from_store() {
        let store = make_store();
        let p = SettingsProvider::new(store, Vec::new());
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
        let p = SettingsProvider::new(store, Vec::new());
        let entries = p.list(Path::new("@/settings/Display")).unwrap();
        assert!(entries.len() >= 1, "Display has fields");
        let leaves: Vec<_> = entries.iter().filter(|e| !e.is_dir).collect();
        for e in &leaves {
            let act = e.activation.as_ref().expect("leaf has activation");
            assert_eq!(act.extension, "settings");
            assert_eq!(act.intent, "edit");
            assert!(act.data.as_str().is_some(), "data is the key string");
        }
        let bs = leaves.iter().find(|e| e.name.contains("border-style")).unwrap();
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
            Vec::new(),
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
        let p = SettingsProvider::new(store, Vec::new());
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
        let ext = SettingsExtension::new(option_store.clone(), matcher_cfg.clone(), Vec::new(), None);
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
        let ext = SettingsExtension::new(option_store, matcher_cfg.clone(), Vec::new(), None);
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
            Vec::new(),
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

    // ── §13.4 SettingsNode graft tests ────────────────────────────────────────

    struct DummyNode { cat: &'static str, node: &'static str }
    impl SettingsNode for DummyNode {
        fn anchor(&self) -> (&str, &str) { (self.cat, self.node) }
        fn list(&self, _path: &Path) -> io::Result<Vec<Entry>> {
            Ok(vec![Entry::virtual_leaf(
                crate::vfs::vfs_root().join("settings").join(self.cat).join(self.node).join("child"),
                "child".into(),
            )])
        }
    }

    fn make_store_with_node(node: Arc<dyn SettingsNode>) -> SettingsProvider {
        let option_store = Arc::new(crate::core::OptionStore::new(core_option_defs()));
        let store = Arc::new(SettingsStore::new(option_store));
        SettingsProvider::new(store, vec![node])
    }

    #[test]
    fn display_category_lists_a_themes_node() {
        let node: Arc<dyn SettingsNode> = Arc::new(DummyNode { cat: "Display", node: "Themes" });
        let p = make_store_with_node(node);
        let entries = p.list(Path::new("@/settings/Display")).unwrap();
        let themes_entry = entries.iter().find(|e| e.name == "Themes");
        assert!(themes_entry.is_some(), "Themes dir entry present under Display");
        assert!(themes_entry.unwrap().is_dir);
    }

    #[test]
    fn themes_path_delegates_to_settings_node() {
        let node: Arc<dyn SettingsNode> = Arc::new(DummyNode { cat: "Display", node: "Themes" });
        let p = make_store_with_node(node);
        let entries = p.list(Path::new("@/settings/Display/Themes")).unwrap();
        assert_eq!(entries.len(), 1, "delegated to DummyNode");
        assert_eq!(entries[0].name, "child");
    }

    #[test]
    fn unrelated_category_has_no_node() {
        let node: Arc<dyn SettingsNode> = Arc::new(DummyNode { cat: "Display", node: "Themes" });
        let p = make_store_with_node(node);
        let entries = p.list(Path::new("@/settings/Matcher")).unwrap();
        assert!(entries.iter().all(|e| !e.is_dir), "Matcher has no node dirs");
    }

    // ── Monitor node ──────────────────────────────────────────────────────────

    fn monitor_provider() -> SettingsProvider {
        let option_store = Arc::new(crate::core::OptionStore::new(core_option_defs()));
        let store = Arc::new(SettingsStore::new(option_store.clone()));
        let node: Arc<dyn SettingsNode> = Arc::new(MonitorNode { store: option_store });
        SettingsProvider::new(store, vec![node])
    }

    #[test]
    fn layout_lists_monitor_dir_and_hides_monitor_options() {
        let p = monitor_provider();
        let entries = p.list(Path::new("@/settings/Layout")).unwrap();
        // The Monitor subtree dir is present...
        assert!(entries.iter().any(|e| e.is_dir && e.name == "Monitor"));
        // ...and the flat monitor.* option leaves are filtered out (owned by the node).
        assert!(
            entries.iter().all(|e| !e.name.contains("monitor.")),
            "monitor.* options must not list flat under Layout: {:?}",
            entries.iter().map(|e| e.name.as_str()).collect::<Vec<_>>()
        );
        // The non-monitor Layout options remain.
        assert!(entries.iter().any(|e| e.name.contains("nav-weight")));
    }

    #[test]
    fn monitor_lists_the_four_metric_dirs() {
        let p = monitor_provider();
        let entries = p.list(Path::new("@/settings/Layout/Monitor")).unwrap();
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["CPU", "Memory", "Disk", "Network"]);
        assert!(entries.iter().all(|e| e.is_dir));
    }

    #[test]
    fn metric_dir_lists_settings_leaves_with_edit_activation() {
        let p = monitor_provider();
        let entries = p.list(Path::new("@/settings/Layout/Monitor/CPU")).unwrap();
        assert_eq!(entries.len(), 7, "seven settings per metric");
        for e in &entries {
            let act = e.activation.as_ref().expect("leaf has activation");
            assert_eq!(act.extension, "settings");
            assert_eq!(act.intent, "edit");
            let key = act.data.as_str().unwrap();
            assert!(key.starts_with("monitor.cpu."), "edits a monitor.cpu.* key: {key}");
        }
        assert!(entries.iter().any(|e| e.name.contains("bar-kind")));
    }
}
