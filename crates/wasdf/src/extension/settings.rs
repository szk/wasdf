//! Settings extension: exposes app configuration as a navigable VFS tree under
//! `@/settings`. Generated dynamically from the shared `OptionStore`: **core**
//! options group by `category` at the root, while **extension-owned** options
//! group under `@/settings/Extensions/<ext-id>` — one per-extension settings
//! screen per extension that declared options (Preview's live there, not as a
//! flat root category). Each leaf shows the current value and opens an inline
//! editor on Enter. A confirmed edit is a `Configure` intent: the settings
//! store validates it, the projections pick it up, and the Persister writes it
//! to `custom.scm` (doc/SCHEME.md).

use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use crate::core::{
    AppState, ConfigWrite, Entry, ExtensionIntent, ExtensionValue, InputFormSpec, InputKind, Intent,
    Mode, OptionKind, OptionStore, OptionValue, KEY_ITEM,
};
use crate::extension::{About, Extension};
use crate::vfs::{vfs_root, VfsProvider};


// ── SettingsNode ──────────────────────────────────────────────────────────────

/// A navigable subtree grafted under a `@/settings/<category>` path — a core
/// category (e.g. `Display`) or an extension's own screen (`Extensions/<id>`,
/// anchored with the extension's id as the "category"). An extension
/// implements this to serve a rich sub-panel (e.g. Themes) that the
/// `SettingsProvider` cannot enumerate from the `OptionStore` alone.
///
/// Activation leaves carry `ExtensionIntent { extension: "<owner>", … }` so
/// the normal dispatch routes them back to the extension that owns the node.
pub trait SettingsNode: Send + Sync {
    /// Where this node grafts: `(category, node-name)`, e.g. `("Display","Themes")`
    /// for a core category, or `("lscolors","Themes")` to graft onto that
    /// extension's own `@/settings/Extensions/lscolors` screen.
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

/// One registered extension's id + About card, for the `@/settings/Extensions`
/// listing, in priority order. The kernel fills it once every extension is
/// registered and rewrites it whenever the `extensions` order changes.
#[derive(Debug, Clone)]
pub struct ExtInfo {
    pub id: String,
    pub about: About,
}

/// The registered extensions, highest priority first.
pub type ExtInfos = Arc<RwLock<Vec<ExtInfo>>>;

pub struct SettingsProvider {
    store: Arc<SettingsStore>,
    nodes: Vec<Arc<dyn SettingsNode>>,
    ext_infos: ExtInfos,
}

impl SettingsProvider {
    fn new(store: Arc<SettingsStore>, nodes: Vec<Arc<dyn SettingsNode>>, ext_infos: ExtInfos) -> Self {
        SettingsProvider { store, nodes, ext_infos }
    }

    /// The registered extensions (or empty until boot fills them).
    fn extensions(&self) -> Vec<ExtInfo> {
        self.ext_infos.read().map(|v| v.clone()).unwrap_or_default()
    }

    /// The ids to list under `@/settings/Extensions`: all registered extensions,
    /// falling back to the option-declaring owners before boot fills the list.
    fn extension_ids(&self) -> Vec<String> {
        let infos = self.extensions();
        if infos.is_empty() {
            self.store.option_store.owners()
        } else {
            infos.iter().map(|i| i.id.clone()).collect()
        }
    }

    /// The About card for an extension id, if the list has been filled.
    fn about_for(&self, id: &str) -> Option<About> {
        self.extensions().into_iter().find(|i| i.id == id).map(|i| i.about)
    }

    /// The extension's priority rank as `(1-based position, total)`: lower is
    /// higher priority wherever one extension is chosen over another (a key
    /// collision, the content provider, `entry_style` — doc/UI.md).
    fn rank_of(&self, id: &str) -> Option<(usize, usize)> {
        let ids = self.extension_ids();
        ids.iter().position(|i| i == id).map(|p| (p + 1, ids.len()))
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

/// The 5th path component of a virtual settings path:
/// `@/settings/Extensions/<id>/<node>[/…]` → `Some("node")`. Used to detect a
/// path reaching below an extension's own screen into a node it grafted there
/// (e.g. lscolors' Themes), the `Extensions/<id>` analogue of a core
/// category's node dirs.
fn ext_subnode_name_for(path: &Path) -> Option<&str> {
    use std::path::Component;
    path.components().nth(4).and_then(|c| {
        if let Component::Normal(s) = c { s.to_str() } else { None }
    })
}

/// The root dir grouping every extension's own settings.
const EXTENSIONS_DIR: &str = "Extensions";

/// Capitalize the first character (for the display name of an extension id).
fn display_name(id: &str) -> String {
    let mut cs = id.chars();
    match cs.next() {
        Some(c) => c.to_uppercase().collect::<String>() + cs.as_str(),
        None => String::new(),
    }
}

/// Build one editable option leaf: `label: value`, activating the `settings`
/// extension's inline editor for the key.
fn option_leaf(path: PathBuf, key: &str, label: &str, value: &str) -> Entry {
    let mut e = Entry::virtual_leaf(path, format!("{label}: {value}"));
    e.activation = Some(ExtensionIntent {
        extension: "settings".into(),
        intent: "edit".into(),
        data: ExtensionValue::String(key.to_string()),
    });
    e
}

impl VfsProvider for SettingsProvider {
    fn mount(&self) -> &str {
        "settings"
    }

    fn list(&self, path: &Path) -> io::Result<Vec<Entry>> {
        let os = &self.store.option_store;
        let base = || vfs_root().join("settings");
        match (crate::vfs::tail_after_mount(path), node_name_for(path)) {
            // Root: the core categories, plus one "Extensions" dir grouping every
            // registered extension's own settings + About (dynamic).
            (None, _) => {
                let mut entries: Vec<Entry> = os
                    .core_categories()
                    .into_iter()
                    .map(|(cat, _)| Entry::virtual_dir(base().join(&cat), cat.clone()))
                    .collect();
                if !self.extension_ids().is_empty() {
                    entries.push(Entry::virtual_dir(
                        base().join(EXTENSIONS_DIR),
                        EXTENSIONS_DIR.to_string(),
                    ));
                }
                Ok(entries)
            }
            // @/settings/Extensions → one dir per registered extension, prefixed
            // with its precedence rank `[N]` in registration/consultation order
            // (rank 1 wins key collisions; the highest rank wins the panel). The
            // list is already in that order, so the ranks read top-to-bottom.
            (Some(cat), None) if cat.eq_ignore_ascii_case(EXTENSIONS_DIR) => Ok(self
                .extension_ids()
                .into_iter()
                .enumerate()
                .map(|(i, id)| {
                    let name = self.about_for(&id).map(|a| a.name.clone()).unwrap_or_else(|| display_name(&id));
                    Entry::virtual_dir(base().join(EXTENSIONS_DIR).join(&id), format!("[{}] {name}", i + 1))
                })
                .collect()),
            // @/settings/Extensions/<id> → the extension's option leaves + About,
            // plus one dir per node the extension grafted onto its own screen
            // (e.g. lscolors' Themes). Any path reaching below such a node
            // delegates to it instead, mirroring how a core category delegates
            // to its own anchored nodes.
            (Some(cat), Some(id)) if cat.eq_ignore_ascii_case(EXTENSIONS_DIR) => {
                if let Some(node_n) = ext_subnode_name_for(path) {
                    let node = self.nodes.iter().find(|n| {
                        let (nc, nn) = n.anchor();
                        nc.eq_ignore_ascii_case(id) && nn.eq_ignore_ascii_case(node_n)
                    });
                    return match node {
                        Some(n) => n.list(path),
                        None => Err(io::Error::new(
                            io::ErrorKind::NotFound,
                            format!("no settings node at {EXTENSIONS_DIR}/{id}/{node_n}"),
                        )),
                    };
                }

                let mut entries: Vec<Entry> = os
                    .options_for_owner(id)
                    .into_iter()
                    .map(|def| {
                        let val = os.get_str(&def.key);
                        option_leaf(
                            base().join(EXTENSIONS_DIR).join(id).join(&def.key),
                            &def.key,
                            &def.label,
                            &val,
                        )
                    })
                    .collect();
                // One virtual dir per node this extension grafted onto its own screen.
                for node in &self.nodes {
                    let (nc, nn) = node.anchor();
                    if nc.eq_ignore_ascii_case(id) {
                        entries.push(Entry::virtual_dir(base().join(EXTENSIONS_DIR).join(id).join(nn), nn.to_string()));
                    }
                }
                // The Priority leaf: the extension's rank, editable (it rewrites
                // the store's `extensions` order).
                if let Some((rank, total)) = self.rank_of(id) {
                    let mut e = Entry::virtual_leaf(
                        base().join(EXTENSIONS_DIR).join(id).join("Priority"),
                        format!("Priority: {rank} of {total} (1 = highest)"),
                    );
                    e.activation = Some(ExtensionIntent {
                        extension: "settings".into(),
                        intent: "edit-priority".into(),
                        data: ExtensionValue::String(id.to_string()),
                    });
                    entries.push(e);
                }
                // A read-only About leaf carrying the version / description.
                if let Some(about) = self.about_for(id) {
                    let mut label = format!("About: {} v{}", about.name, about.version);
                    if !about.description.is_empty() {
                        label.push_str(" — ");
                        label.push_str(&about.description);
                    }
                    entries.push(Entry::virtual_leaf(base().join(EXTENSIONS_DIR).join(id).join("About"), label));
                } else if entries.is_empty() {
                    return Err(io::Error::new(
                        io::ErrorKind::NotFound,
                        format!("no settings for extension: {id}"),
                    ));
                }
                Ok(entries)
            }
            // Core category: its (unowned) option leaves + one dir per grafted node.
            (Some(cat_name), None) => {
                let cats = os.core_categories();
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
                        option_leaf(base().join(cat_name).join(&def.key), &def.key, &def.label, &val)
                    })
                    .collect();
                // Append a virtual dir for each node anchored to this category.
                for node in &self.nodes {
                    let (nc, nn) = node.anchor();
                    if nc.eq_ignore_ascii_case(cat_name) {
                        entries.push(Entry::virtual_dir(base().join(cat_name).join(nn), nn.to_string()));
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
    nodes: Vec<Arc<dyn SettingsNode>>,
    /// The registered extensions (id + About), filled by the kernel once all
    /// extensions are known; shared with the provider so it can list them under
    /// `@/settings/Extensions`.
    ext_infos: ExtInfos,
}

impl SettingsExtension {
    fn ranked_ids(&self) -> Vec<String> {
        self.ext_infos.read().map(|v| v.iter().map(|i| i.id.clone()).collect()).unwrap_or_default()
    }

    pub fn new(
        option_store: Arc<OptionStore>,
        nodes: Vec<Arc<dyn SettingsNode>>,
        ext_infos: ExtInfos,
    ) -> Self {
        let store = Arc::new(SettingsStore::new(option_store));
        // The Monitor node is settings-owned (it reorganises this store's own
        // monitor.* options), so it is grafted here rather than collected in kernel.
        let mut nodes = nodes;
        nodes.push(Arc::new(MonitorNode { store: store.option_store.clone() }));
        SettingsExtension { store, nodes, ext_infos }
    }
}

impl Extension for SettingsExtension {
    fn id(&self) -> &str {
        "settings"
    }

    fn about(&self) -> crate::extension::About {
        crate::extension::About::new("Settings", "Browse and edit app + extension configuration under @/settings")
    }

    fn vfs_providers(&self) -> Vec<Box<dyn VfsProvider>> {
        vec![Box::new(SettingsProvider::new(self.store.clone(), self.nodes.clone(), self.ext_infos.clone()))]
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

                // Every option kind gets its own typed widget; invalid typed
                // values (Int out of range, a stale Choice option after a
                // config edit) are simply rejected by `OptionStore::set`
                // (below), same as an invalid value always was.
                let kind = match &def.kind {
                    OptionKind::Bool => InputKind::Bool,
                    OptionKind::Text => InputKind::Text,
                    OptionKind::List(_) => InputKind::List,
                    // Structured values have no one-line form; their editors are
                    // settings nodes of the owning extension.
                    OptionKind::Record(_) | OptionKind::Map(_) => return Vec::new(),
                    OptionKind::Int { min, max } => InputKind::Int { min: *min, max: *max },
                    OptionKind::Choice(opts) => InputKind::Choice(opts.clone()),
                };
                let hint = def.kind.hint();
                let title = if hint.is_empty() { key.to_string() } else { format!("{key} ({hint})") };
                let current = os.get_str(key);
                let spec = InputFormSpec::emit_input_widget(title, template, kind, current);

                vec![Intent::PushMode(Box::new(Mode::InputForm(spec)))]
            }
            "edit-priority" => {
                let Some(id) = intent.data.as_str() else { return Vec::new() };
                let ids = self.ranked_ids();
                let Some(pos) = ids.iter().position(|i| i == id) else { return Vec::new() };
                let template = ExtensionIntent {
                    extension: "settings".into(),
                    intent: "set-priority".into(),
                    data: ExtensionValue::Nil.with("field", ExtensionValue::String(id.into())),
                };
                let kind = InputKind::Int { min: 1, max: ids.len() as i64 };
                let spec = InputFormSpec::emit_input_widget(format!("{id} priority (1 = highest)"), template, kind, (pos + 1).to_string());
                vec![Intent::PushMode(Box::new(Mode::InputForm(spec)))]
            }
            "set-priority" => {
                let Some(id) = intent.data.get("field").and_then(|v| v.as_str()) else { return Vec::new() };
                let Some(rank) = intent.data.get(KEY_ITEM).and_then(|v| v.as_str()).and_then(|v| v.trim().parse::<usize>().ok())
                else {
                    return Vec::new();
                };
                let mut items: Vec<String> = self.ranked_ids().into_iter().filter(|i| i != id).collect();
                items.insert((rank.max(1) - 1).min(items.len()), id.to_string());
                vec![Intent::Configure(ConfigWrite::SetOrder { name: "extensions".into(), items })]
            }
            "set" => {
                let Some(key) = intent.data.get("field").and_then(|v| v.as_str()) else {
                    return Vec::new();
                };
                let Some(val) = intent.data.get(KEY_ITEM).and_then(|v| v.as_str()) else {
                    return Vec::new();
                };

                let value = self
                    .store
                    .option_store
                    .def(key)
                    .and_then(|def| OptionValue::parse_flat(&def.kind, val));
                match value {
                    Some(value) => vec![Intent::Configure(ConfigWrite::SetOption { key: key.to_string(), value })],
                    None => vec![Intent::Notify(format!("{key}: invalid value {val:?}"))],
                }
            }
            _ => Vec::new(),
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{OptionDef, OptionKind};

    /// A provider whose extension list is filled (as the kernel does at boot):
    /// `preview` (with an option) and `archive` (no options, About only).
    fn provider() -> SettingsProvider {
        provider_with_nodes(vec![])
    }

    /// A stub node anchored onto an extension's own screen, e.g. `("preview","Sub")`
    /// for `@/settings/Extensions/preview/Sub` — stands in for `lscolors::LsColorsNode`
    /// without pulling in the whole extension.
    struct StubExtNode {
        anchor: (&'static str, &'static str),
    }

    impl SettingsNode for StubExtNode {
        fn anchor(&self) -> (&str, &str) {
            self.anchor
        }

        fn list(&self, _path: &Path) -> io::Result<Vec<Entry>> {
            Ok(vec![Entry::virtual_leaf(vfs_root().join("settings").join("stub-leaf"), "stub-leaf".into())])
        }
    }

    fn provider_with_nodes(nodes: Vec<Arc<dyn SettingsNode>>) -> SettingsProvider {
        let os = OptionStore::new(vec![
            OptionDef {
                key: "border-style".into(),
                category: "Display".into(),
                label: "border-style".into(),
                kind: OptionKind::Choice(vec!["single".into()]),
                default: crate::core::OptionValue::Text("single".into()),
                owner: None,
            },
            OptionDef {
                key: "preview.tab-width".into(),
                category: "Preview".into(),
                label: "tab-width".into(),
                kind: OptionKind::Int { min: 1, max: 8 },
                default: crate::core::OptionValue::Int(4),
                owner: Some("preview".into()),
            },
        ]);
        let infos: ExtInfos = Arc::new(RwLock::new(vec![
            ExtInfo { id: "preview".into(), about: About::new("Preview", "file preview") },
            ExtInfo { id: "archive".into(), about: About::new("Archive", "pack/unpack") },
        ]));
        SettingsProvider {
            store: Arc::new(SettingsStore::new(Arc::new(os))),
            nodes,
            ext_infos: infos,
        }
    }

    fn names(entries: &[Entry]) -> Vec<String> {
        entries.iter().map(|e| e.name.clone()).collect()
    }

    #[test]
    fn root_lists_core_categories_plus_the_extensions_dir() {
        let ns = names(&provider().list(&vfs_root().join("settings")).unwrap());
        assert!(ns.contains(&"Display".to_string()), "core category listed: {ns:?}");
        assert!(ns.contains(&EXTENSIONS_DIR.to_string()), "Extensions grouping listed: {ns:?}");
        // The extension's own category no longer appears as a flat root category.
        assert!(!ns.contains(&"Preview".to_string()), "extension category moved out of the root: {ns:?}");
    }

    #[test]
    fn extensions_dir_lists_one_dir_per_registered_extension_ranked() {
        let list = provider().list(&vfs_root().join("settings").join(EXTENSIONS_DIR)).unwrap();
        // Every registered extension appears (About-only ones included), prefixed
        // with its 1-based precedence rank in registration/consultation order.
        assert_eq!(names(&list), vec!["[1] Preview", "[2] Archive"]);
        // Navigation still keys off the id, not the ranked display name.
        assert!(list[0].path.ends_with("preview"), "path uses the id: {:?}", list[0].path);
    }

    #[test]
    fn extension_dir_lists_its_editable_options_and_an_about_leaf() {
        let path = vfs_root().join("settings").join(EXTENSIONS_DIR).join("preview");
        let list = provider().list(&path).unwrap();
        // The editable option leaf.
        let opt = list.iter().find(|e| e.name.contains("tab-width")).expect("option leaf");
        let act = opt.activation.as_ref().expect("leaf edits via the settings extension");
        assert_eq!(act.extension, "settings");
        assert_eq!(act.intent, "edit");
        // The read-only About leaf carrying version + description.
        let about = list.iter().find(|e| e.name.starts_with("About:")).expect("About leaf");
        assert!(about.name.contains("Preview"), "About shows the name: {}", about.name);
        assert!(about.name.contains("file preview"), "About shows the description: {}", about.name);
        assert!(about.activation.is_none(), "About is read-only");
    }

    #[test]
    fn extension_dir_lists_a_node_grafted_onto_its_own_screen() {
        // A node anchored ("preview","Sub") appears as a virtual dir alongside
        // preview's own option leaves — the Extensions/<id> analogue of a core
        // category listing its own anchored nodes.
        let node: Arc<dyn SettingsNode> = Arc::new(StubExtNode { anchor: ("preview", "Sub") });
        let list = provider_with_nodes(vec![node])
            .list(&vfs_root().join("settings").join(EXTENSIONS_DIR).join("preview"))
            .unwrap();
        let dir = list.iter().find(|e| e.name == "Sub").expect("grafted node dir listed");
        assert!(dir.path.ends_with("preview/Sub"));
    }

    #[test]
    fn a_path_below_an_extension_owned_node_delegates_to_it() {
        let node: Arc<dyn SettingsNode> = Arc::new(StubExtNode { anchor: ("preview", "Sub") });
        let list = provider_with_nodes(vec![node])
            .list(&vfs_root().join("settings").join(EXTENSIONS_DIR).join("preview").join("Sub"))
            .unwrap();
        assert_eq!(names(&list), vec!["stub-leaf"], "delegated to the node instead of the flat option listing");
    }

    #[test]
    fn about_only_extension_lists_its_priority_and_about() {
        // `archive` declared no options → its screen is the Priority + About leaves.
        let path = vfs_root().join("settings").join(EXTENSIONS_DIR).join("archive");
        let list = provider().list(&path).unwrap();
        assert_eq!(list.len(), 2, "Priority + About: {:?}", names(&list));
        let about = list.iter().find(|e| e.name.starts_with("About:")).expect("About leaf");
        assert!(about.name.contains("Archive"));
    }

    #[test]
    fn extension_screen_shows_its_priority_and_editing_it_writes_the_order() {
        // `archive` is listed second → priority 2 of 2.
        let path = vfs_root().join("settings").join(EXTENSIONS_DIR).join("archive");
        let p = provider();
        let list = p.list(&path).unwrap();
        let prio = list.iter().find(|e| e.name.starts_with("Priority:")).expect("Priority leaf");
        assert!(prio.name.contains("2 of 2"), "rank shown: {}", prio.name);
        let ext = SettingsExtension::new(p.store.option_store.clone(), vec![], p.ext_infos.clone());
        let set = ExtensionIntent {
            extension: "settings".into(),
            intent: "set-priority".into(),
            data: ExtensionValue::Nil
                .with("field", ExtensionValue::String("archive".into()))
                .with(KEY_ITEM, ExtensionValue::String("1".into())),
        };
        assert_eq!(
            ext.handle_intent(&set, &AppState::new(std::env::temp_dir())),
            vec![Intent::Configure(ConfigWrite::SetOrder {
                name: "extensions".into(),
                items: vec!["archive".into(), "preview".into()],
            })]
        );
    }

    // A **user** extension dropped into the extensions dir (the same path the
    // kernel scans: `$WASDF_EXTENSIONS_DIR` / `~/.config/wasdf/extensions`) is
    // registered like a bundled one and appears under @/settings/Extensions with
    // an About card carrying the app version — equivalent display to bundled.
    #[test]
    fn a_user_extension_appears_under_extensions_like_a_bundled_one() {
        use std::io::Write;
        let dir = std::env::temp_dir().join(format!("wasdf-userext-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut f = std::fs::File::create(dir.join("userext.scm")).unwrap();
        writeln!(f, "(extension \"userext\")").unwrap();

        // Loads via the real optional-extension loader.
        let loaded = crate::extension::loader::load_optional(&dir);
        assert_eq!(loaded.len(), 1, "the user manifest loaded");
        let ue = &loaded[0];
        assert_eq!(ue.id(), "userext");
        let about = ue.about();
        assert_eq!(about.name, "userext");
        assert_eq!(about.version, env!("CARGO_PKG_VERSION"), "default About carries the app version");

        // Fed into the settings provider exactly as the kernel does at boot.
        let infos: ExtInfos = Arc::new(RwLock::new(vec![ExtInfo { id: ue.id().to_string(), about }]));
        let p = SettingsProvider {
            store: Arc::new(SettingsStore::new(Arc::new(OptionStore::new(vec![])))),
            nodes: vec![],
            ext_infos: infos,
        };

        let listed = names(&p.list(&vfs_root().join("settings").join(EXTENSIONS_DIR)).unwrap());
        assert!(
            listed.iter().any(|n| n.contains("userext")),
            "user extension listed under Extensions: {listed:?}",
        );

        let screen = p.list(&vfs_root().join("settings").join(EXTENSIONS_DIR).join("userext")).unwrap();
        assert!(
            screen.iter().any(|e| e.name.starts_with("About:") && e.name.contains("userext")),
            "user extension has an About leaf: {:?}",
            names(&screen),
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_confirmed_edit_is_a_typed_configure_and_a_bad_value_a_notice() {
        let store = Arc::new(OptionStore::new(vec![OptionDef {
            key: "n".into(),
            category: "Display".into(),
            label: "n".into(),
            kind: OptionKind::Int { min: 1, max: 9 },
            default: OptionValue::Int(1),
            owner: None,
        }]));
        let ext = SettingsExtension::new(store.clone(), vec![], Arc::default());
        let set = |val: &str| {
            let data = ExtensionValue::Nil
                .with("field", ExtensionValue::String("n".into()))
                .with(KEY_ITEM, ExtensionValue::String(val.into()));
            let intent = ExtensionIntent { extension: "settings".into(), intent: "set".into(), data };
            ext.handle_intent(&intent, &AppState::new(std::env::temp_dir()))
        };
        assert_eq!(
            set("7"),
            vec![Intent::Configure(ConfigWrite::SetOption { key: "n".into(), value: OptionValue::Int(7) })]
        );
        assert!(matches!(set("70").as_slice(), [Intent::Notify(_)]), "out of range");
        assert_eq!(store.get_i64("n"), 1, "the extension never writes the projection itself");
    }
}
