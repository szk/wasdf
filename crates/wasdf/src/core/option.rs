//! Option vocabulary: the `OptionKind` type system, `OptionDef` record, and the
//! `OptionStore` that replaces the typed `Options` struct. Placed in `core` so
//! `app`, `extension`, and `services` can all depend on it without cycles.

use std::collections::HashMap;
use std::sync::RwLock;

// ── OptionKind ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum OptionKind {
    Bool,
    Text,
    List, // comma-separated
    Int { min: i64, max: i64 },
    Choice(Vec<String>),
}

impl OptionKind {
    /// Whether a scheme-string value is valid for this kind.
    pub fn accepts(&self, val: &str) -> bool {
        match self {
            OptionKind::Bool => matches!(val, "#t" | "#f"),
            OptionKind::Text => true,
            OptionKind::List => true,
            OptionKind::Int { min, max } => {
                val.parse::<i64>().map(|n| n >= *min && n <= *max).unwrap_or(false)
            }
            OptionKind::Choice(opts) => opts.iter().any(|o| o == val),
        }
    }

    /// User-visible summary of the accepted values (for UI placeholders).
    pub fn hint(&self) -> String {
        match self {
            OptionKind::Bool => "#t | #f".into(),
            OptionKind::Text | OptionKind::List => String::new(),
            OptionKind::Int { min, max } => format!("{min}..{max}"),
            OptionKind::Choice(opts) => opts.join(" | "),
        }
    }
}

// ── OptionDef ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct OptionDef {
    pub key: String,
    pub category: String,
    pub label: String,
    pub kind: OptionKind,
    /// The value in scheme-string form (`"#t"`, `"4"`, `"dark"`, `"a,b"`). This
    /// is the canonical representation stored in `values`, persisted to
    /// `config.scm`, and returned by `get()`.
    pub default: String,
    /// The id of the extension that declared this option, or `None` for a core
    /// option. Drives settings placement: core options group by `category` at the
    /// settings root; extension options group under `@/settings/Extensions/<owner>`.
    pub owner: Option<String>,
}

// ── OptionStore ───────────────────────────────────────────────────────────────

/// A string-keyed store of option definitions and current values. Thread-safe
/// via interior locking; shared as `Arc<OptionStore>`.
///
/// Ordering is preserved: categories appear in the order they were first seen
/// across the def stream; defs within a category appear in insertion order.
/// This drives the stable SETTINGS tree.
pub struct OptionStore {
    defs: HashMap<String, OptionDef>,
    def_order: Vec<String>,
    category_order: Vec<String>,
    values: RwLock<HashMap<String, String>>,
    /// Values whose def was absent at `set` time; re-bound when `register` later
    /// brings the matching def (toggling an optional extension off/on).
    unowned: RwLock<HashMap<String, String>>,
}

impl OptionStore {
    /// Build from all collected defs; seed `values` from each def's default.
    /// First-wins on duplicate keys.
    pub fn new(defs: Vec<OptionDef>) -> Self {
        let mut def_map: HashMap<String, OptionDef> = HashMap::new();
        let mut def_order: Vec<String> = Vec::new();
        let mut category_order: Vec<String> = Vec::new();
        let mut values: HashMap<String, String> = HashMap::new();

        for def in defs {
            if def_map.contains_key(&def.key) {
                continue;
            }
            if !category_order.contains(&def.category) {
                category_order.push(def.category.clone());
            }
            values.insert(def.key.clone(), def.default.clone());
            def_order.push(def.key.clone());
            def_map.insert(def.key.clone(), def);
        }

        OptionStore {
            defs: def_map,
            def_order,
            category_order,
            values: RwLock::new(values),
            unowned: RwLock::new(HashMap::new()),
        }
    }

    /// Register late-arriving defs (e.g. optional extensions loaded after the
    /// initial build). Re-binds any matching `unowned` value into `values`.
    /// First-wins: a key already registered is ignored.
    pub fn register(&mut self, defs: Vec<OptionDef>) {
        for def in defs {
            if self.defs.contains_key(&def.key) {
                continue;
            }
            if !self.category_order.contains(&def.category) {
                self.category_order.push(def.category.clone());
            }
            let restored = self.unowned.write().unwrap().remove(&def.key);
            let initial = restored.unwrap_or_else(|| def.default.clone());
            self.values.write().unwrap().insert(def.key.clone(), initial);
            self.def_order.push(def.key.clone());
            self.defs.insert(def.key.clone(), def);
        }
    }

    /// Apply a `(key, val)` pair.
    ///
    /// Returns `true` on success. Returns `false` for an invalid value (the
    /// def exists but rejects the value) — caller may warn and keep the prior
    /// value. Returns `false` for an unknown key — the value is stashed in
    /// `unowned` so a later `register` can bind it.
    pub fn set(&self, key: &str, val: &str) -> bool {
        match self.defs.get(key) {
            Some(def) if def.kind.accepts(val) => {
                self.values.write().unwrap().insert(key.into(), val.into());
                true
            }
            Some(_) => false,
            None => {
                self.unowned.write().unwrap().insert(key.into(), val.into());
                false
            }
        }
    }

    /// Get the current value (or the def default) in scheme-string form. Returns
    /// `None` for an unknown key.
    pub fn get(&self, key: &str) -> Option<String> {
        self.values
            .read()
            .unwrap()
            .get(key)
            .cloned()
            .or_else(|| self.defs.get(key).map(|d| d.default.clone()))
    }

    // ── Typed accessors ───────────────────────────────────────────────────────

    // The value-string parsing is shared with dynamically loaded extensions via
    // `wasdf_sdk::settings`, so both sides read a bool / int / list identically.

    /// `"#t"` → true; anything else (including absent) → false.
    pub fn get_bool(&self, key: &str) -> bool {
        wasdf_sdk::settings::parse_bool(self.get(key).as_deref())
    }

    /// Parse the value as `u32`; returns `0` on absent or parse failure.
    pub fn get_u32(&self, key: &str) -> u32 {
        wasdf_sdk::settings::parse_u32(self.get(key).as_deref())
    }

    /// Parse the value as `i64`; returns `0` on absent or parse failure.
    pub fn get_i64(&self, key: &str) -> i64 {
        wasdf_sdk::settings::parse_i64(self.get(key).as_deref())
    }

    /// Get the value as a `String`; returns `""` for absent or unparseable.
    pub fn get_str(&self, key: &str) -> String {
        self.get(key).unwrap_or_default()
    }

    /// Split a `list` value on `,`, trim each part, drop empties.
    pub fn get_list(&self, key: &str) -> Vec<String> {
        wasdf_sdk::settings::parse_list(self.get(key).as_deref())
    }

    // ── Schema queries ────────────────────────────────────────────────────────

    /// All categories in insertion order, each with its defs in insertion order.
    pub fn categories(&self) -> Vec<(String, Vec<&OptionDef>)> {
        self.category_order
            .iter()
            .map(|cat| {
                let fields: Vec<&OptionDef> = self
                    .def_order
                    .iter()
                    .filter_map(|k| self.defs.get(k))
                    .filter(|d| &d.category == cat)
                    .collect();
                (cat.clone(), fields)
            })
            .collect()
    }

    /// Categories that contain at least one **core** (extension-unowned) option,
    /// each with only its core defs. Extension options are excluded — they group
    /// under `@/settings/Extensions/<owner>` instead of as a flat category.
    pub fn core_categories(&self) -> Vec<(String, Vec<&OptionDef>)> {
        self.category_order
            .iter()
            .filter_map(|cat| {
                let defs: Vec<&OptionDef> = self
                    .def_order
                    .iter()
                    .filter_map(|k| self.defs.get(k))
                    .filter(|d| &d.category == cat && d.owner.is_none())
                    .collect();
                (!defs.is_empty()).then(|| (cat.clone(), defs))
            })
            .collect()
    }

    /// The distinct extension owners that declared options, in first-seen order.
    pub fn owners(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for d in self.def_order.iter().filter_map(|k| self.defs.get(k)) {
            if let Some(o) = &d.owner {
                if !out.contains(o) {
                    out.push(o.clone());
                }
            }
        }
        out
    }

    /// The option defs declared by a given extension, in insertion order.
    pub fn options_for_owner(&self, owner: &str) -> Vec<&OptionDef> {
        self.def_order
            .iter()
            .filter_map(|k| self.defs.get(k))
            .filter(|d| d.owner.as_deref() == Some(owner))
            .collect()
    }

    /// A snapshot of an extension's own option values as a [`wasdf_sdk::Settings`],
    /// so the owning extension reads them through the shared SDK accessors. The
    /// same key→value snapshot is what a dynamically loaded extension is handed
    /// over the ABI, keeping bundled and dynamic reads identical.
    pub fn settings_for(&self, owner: &str) -> wasdf_sdk::Settings {
        wasdf_sdk::Settings::from_pairs(
            self.options_for_owner(owner)
                .into_iter()
                .map(|d| (d.key.clone(), self.get_str(&d.key))),
        )
    }

    /// Look up the def for a key.
    pub fn def(&self, key: &str) -> Option<&OptionDef> {
        self.defs.get(key)
    }

    /// All registered keys in insertion order.
    pub fn keys(&self) -> &[String] {
        &self.def_order
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn bool_def(key: &str, default: &str) -> OptionDef {
        OptionDef {
            key: key.into(),
            category: "Test".into(),
            label: key.into(),
            kind: OptionKind::Bool,
            default: default.into(),
            owner: None,
        }
    }

    fn int_def(key: &str, default: &str, min: i64, max: i64) -> OptionDef {
        OptionDef {
            key: key.into(),
            category: "Test".into(),
            label: key.into(),
            kind: OptionKind::Int { min, max },
            default: default.into(),
            owner: None,
        }
    }

    fn choice_def(key: &str, default: &str, opts: &[&str]) -> OptionDef {
        OptionDef {
            key: key.into(),
            category: "Test".into(),
            label: key.into(),
            kind: OptionKind::Choice(opts.iter().map(|s| s.to_string()).collect()),
            default: default.into(),
            owner: None,
        }
    }

    fn list_def(key: &str, default: &str) -> OptionDef {
        OptionDef {
            key: key.into(),
            category: "Test".into(),
            label: key.into(),
            kind: OptionKind::List,
            default: default.into(),
            owner: None,
        }
    }

    #[test]
    fn get_returns_default_when_not_set() {
        let store = OptionStore::new(vec![bool_def("x", "#f")]);
        assert_eq!(store.get("x"), Some("#f".into()));
        assert!(!store.get_bool("x"));
    }

    #[test]
    fn set_valid_bool_updates_value() {
        let store = OptionStore::new(vec![bool_def("x", "#f")]);
        assert!(store.set("x", "#t"));
        assert!(store.get_bool("x"));
    }

    #[test]
    fn set_invalid_bool_is_rejected() {
        let store = OptionStore::new(vec![bool_def("x", "#f")]);
        assert!(!store.set("x", "yes"));
        assert!(!store.get_bool("x"), "value unchanged");
    }

    #[test]
    fn set_int_within_range_accepted() {
        let store = OptionStore::new(vec![int_def("n", "5", 1, 99)]);
        assert!(store.set("n", "42"));
        assert_eq!(store.get_i64("n"), 42);
    }

    #[test]
    fn set_int_out_of_range_rejected() {
        let store = OptionStore::new(vec![int_def("n", "5", 1, 99)]);
        assert!(!store.set("n", "100"));
        assert_eq!(store.get_i64("n"), 5, "value unchanged");
    }

    #[test]
    fn set_choice_valid_accepted() {
        let store = OptionStore::new(vec![choice_def("c", "a", &["a", "b", "c"])]);
        assert!(store.set("c", "b"));
        assert_eq!(store.get_str("c"), "b");
    }

    #[test]
    fn set_choice_invalid_rejected() {
        let store = OptionStore::new(vec![choice_def("c", "a", &["a", "b"])]);
        assert!(!store.set("c", "z"));
        assert_eq!(store.get_str("c"), "a", "value unchanged");
    }

    #[test]
    fn get_list_splits_and_trims() {
        let store = OptionStore::new(vec![list_def("l", "rs, toml, md")]);
        assert_eq!(store.get_list("l"), vec!["rs", "toml", "md"]);
    }

    #[test]
    fn get_list_empty_string_returns_empty_vec() {
        let store = OptionStore::new(vec![list_def("l", "")]);
        assert_eq!(store.get_list("l"), Vec::<String>::new());
    }

    #[test]
    fn unknown_key_is_stashed_as_unowned() {
        let store = OptionStore::new(vec![]);
        assert!(!store.set("unknown", "val"));
        // value is stashed but not visible via get
        assert!(store.get("unknown").is_none());
    }

    #[test]
    fn register_rebinds_unowned_value() {
        let mut store = OptionStore::new(vec![]);
        // Stash before the def exists.
        store.set("x", "#t");
        // Now register the def — the stashed value should become live.
        store.register(vec![bool_def("x", "#f")]);
        assert!(store.get_bool("x"), "stashed value bound after register");
    }

    #[test]
    fn register_first_wins_does_not_overwrite() {
        let mut store = OptionStore::new(vec![bool_def("x", "#t")]);
        store.register(vec![bool_def("x", "#f")]);
        assert!(store.get_bool("x"), "first def wins");
    }

    #[test]
    fn categories_preserves_insertion_order() {
        let defs = vec![
            OptionDef {
                key: "a.x".into(),
                category: "A".into(),
                label: "x".into(),
                kind: OptionKind::Bool,
                default: "#f".into(),
                owner: None,
            },
            OptionDef {
                key: "b.y".into(),
                category: "B".into(),
                label: "y".into(),
                kind: OptionKind::Bool,
                default: "#f".into(),
                owner: None,
            },
            OptionDef {
                key: "a.z".into(),
                category: "A".into(),
                label: "z".into(),
                kind: OptionKind::Bool,
                default: "#f".into(),
                owner: None,
            },
        ];
        let store = OptionStore::new(defs);
        let cats = store.categories();
        assert_eq!(cats.len(), 2);
        assert_eq!(cats[0].0, "A");
        assert_eq!(cats[0].1.len(), 2, "A has two defs");
        assert_eq!(cats[1].0, "B");
    }

    #[test]
    fn def_lookup_returns_correct_kind() {
        let store = OptionStore::new(vec![int_def("n", "5", 1, 99)]);
        let d = store.def("n").expect("def exists");
        assert_eq!(d.kind, OptionKind::Int { min: 1, max: 99 });
    }

    #[test]
    fn owner_grouping_and_sdk_settings_snapshot() {
        let owned = |key: &str, cat: &str, def: &str, owner: &str| OptionDef {
            key: key.into(),
            category: cat.into(),
            label: key.into(),
            kind: OptionKind::Int { min: 0, max: 99 },
            default: def.into(),
            owner: Some(owner.into()),
        };
        let store = OptionStore::new(vec![
            int_def("core.n", "1", 0, 9),        // core (owner None)
            owned("preview.tab-width", "Preview", "4", "preview"),
            owned("preview.pager", "Preview", "2", "preview"),
        ]);
        assert_eq!(store.owners(), vec!["preview"]);
        assert_eq!(store.options_for_owner("preview").len(), 2);
        // core_categories excludes the extension-owned "Preview" category.
        let core = store.core_categories();
        assert!(core.iter().any(|(c, _)| c == "Test")); // int_def uses "Test"
        assert!(!core.iter().any(|(c, _)| c == "Preview"));
        // settings_for produces a wasdf_sdk::Settings the owner reads through.
        store.set("preview.tab-width", "6");
        let s = store.settings_for("preview");
        assert_eq!(s.get_u32("preview.tab-width"), 6);
        assert_eq!(s.get_u32("preview.pager"), 2);
        assert_eq!(s.get("core.n"), None, "only the owner's keys are snapshotted");
    }
}
