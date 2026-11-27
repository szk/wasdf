//! Option vocabulary: the `OptionKind` type system, the typed `OptionValue`, the
//! `OptionDef` record, and the `OptionStore` — the native projection of every
//! option's effective value (doc/SCHEME.md, The Settings Store). Placed in
//! `core` so `app`, `extension`, and `services` can all depend on it without
//! cycles.

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::RwLock;

// ── OptionKind ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum OptionKind {
    Bool,
    Text,
    Int { min: i64, max: i64 },
    Choice(Vec<String>),
    /// A list whose items are all of the inner kind.
    List(Box<OptionKind>),
    /// A fixed set of named fields, each of its own kind.
    Record(Vec<(String, OptionKind)>),
    /// Arbitrary string keys, every value of the inner kind.
    Map(Box<OptionKind>),
}

impl OptionKind {
    /// A list of text items — the kind the bare `list` symbol declares.
    pub fn text_list() -> Self {
        OptionKind::List(Box::new(OptionKind::Text))
    }

    /// Whether `val` is a valid value of this kind.
    pub fn accepts(&self, val: &OptionValue) -> bool {
        match (self, val) {
            (OptionKind::Bool, OptionValue::Bool(_)) => true,
            (OptionKind::Text, OptionValue::Text(_)) => true,
            (OptionKind::Int { min, max }, OptionValue::Int(n)) => n >= min && n <= max,
            (OptionKind::Choice(opts), OptionValue::Text(s)) => opts.iter().any(|o| o == s),
            (OptionKind::List(item), OptionValue::List(items)) => items.iter().all(|v| item.accepts(v)),
            (OptionKind::Record(fields), OptionValue::Map(m)) => {
                m.len() == fields.len()
                    && fields.iter().all(|(name, k)| m.get(name).is_some_and(|v| k.accepts(v)))
            }
            (OptionKind::Map(item), OptionValue::Map(m)) => m.values().all(|v| item.accepts(v)),
            _ => false,
        }
    }

    /// The kind as the Scheme datum the settings store validates against:
    /// `bool`, `text`, `(int 1 9)`, `(choice "a" "b")`, `(list text)`,
    /// `(record (fg text) (bold bool))`, `(map text)`.
    pub fn to_scheme(&self) -> String {
        match self {
            OptionKind::Bool => "bool".into(),
            OptionKind::Text => "text".into(),
            OptionKind::Int { min, max } => format!("(int {min} {max})"),
            OptionKind::Choice(opts) => {
                format!("(choice {})", opts.iter().map(|o| format!("{o:?}")).collect::<Vec<_>>().join(" "))
            }
            OptionKind::List(item) => format!("(list {})", item.to_scheme()),
            OptionKind::Record(fields) => format!(
                "(record {})",
                fields.iter().map(|(n, k)| format!("({n} {})", k.to_scheme())).collect::<Vec<_>>().join(" ")
            ),
            OptionKind::Map(item) => format!("(map {})", item.to_scheme()),
        }
    }

    /// User-visible summary of the accepted values (for UI placeholders).
    pub fn hint(&self) -> String {
        match self {
            OptionKind::Bool => "#t | #f".into(),
            OptionKind::Int { min, max } => format!("{min}..{max}"),
            OptionKind::Choice(opts) => opts.join(" | "),
            OptionKind::Text | OptionKind::List(_) | OptionKind::Record(_) | OptionKind::Map(_) => {
                String::new()
            }
        }
    }
}

// ── OptionValue ───────────────────────────────────────────────────────────────

/// A typed option value. Deliberately separate from `ExtensionValue` (intent
/// payloads): options have no `Nil` or `Path`, and their `OptionKind` decides
/// validation and rendering. A `record` and a `map` are both `Map`; a `choice`
/// is `Text`.
#[derive(Debug, Clone, PartialEq)]
pub enum OptionValue {
    Bool(bool),
    Int(i64),
    Text(String),
    List(Vec<OptionValue>),
    Map(BTreeMap<String, OptionValue>),
}

impl OptionValue {
    /// Parse the flat string form used by one-line inputs and legacy config
    /// (`"#t"`, `"42"`, `"dark"`, `"a,b"`) according to `kind`. Structured
    /// kinds have no flat form and return `None`.
    pub fn parse_flat(kind: &OptionKind, s: &str) -> Option<OptionValue> {
        let v = match kind {
            OptionKind::Bool => match s {
                "#t" => OptionValue::Bool(true),
                "#f" => OptionValue::Bool(false),
                _ => return None,
            },
            OptionKind::Int { .. } => OptionValue::Int(s.trim().parse().ok()?),
            OptionKind::Text | OptionKind::Choice(_) => OptionValue::Text(s.to_string()),
            OptionKind::List(item) => OptionValue::List(
                s.split(',')
                    .map(str::trim)
                    .filter(|p| !p.is_empty())
                    .map(|p| OptionValue::parse_flat(item, p))
                    .collect::<Option<Vec<_>>>()?,
            ),
            OptionKind::Record(_) | OptionKind::Map(_) => return None,
        };
        kind.accepts(&v).then_some(v)
    }

    /// The flat string form (the inverse of [`parse_flat`](Self::parse_flat)
    /// for scalar and list kinds): what one-line inputs show and what a
    /// `wasdf_sdk::Settings` snapshot carries. Structured values render as
    /// their Scheme literal.
    pub fn flat(&self) -> String {
        match self {
            OptionValue::Bool(b) => if *b { "#t" } else { "#f" }.into(),
            OptionValue::Int(n) => n.to_string(),
            OptionValue::Text(s) => s.clone(),
            OptionValue::List(items) => items.iter().map(OptionValue::flat).collect::<Vec<_>>().join(","),
            OptionValue::Map(_) => self.to_scheme(),
        }
    }

    /// The value as a Scheme literal: `#t`, `42`, `"text"`, `("a" "b")`, and
    /// a map as an association list `(("key" value) …)`.
    pub fn to_scheme(&self) -> String {
        match self {
            OptionValue::Bool(b) => if *b { "#t" } else { "#f" }.into(),
            OptionValue::Int(n) => n.to_string(),
            OptionValue::Text(s) => format!("{s:?}"),
            OptionValue::List(items) => {
                format!("({})", items.iter().map(OptionValue::to_scheme).collect::<Vec<_>>().join(" "))
            }
            OptionValue::Map(m) => format!(
                "({})",
                m.iter().map(|(k, v)| format!("({k:?} {})", v.to_scheme())).collect::<Vec<_>>().join(" ")
            ),
        }
    }
}

impl OptionValue {
    /// The value as a literal argument of a store call: lists and maps quoted.
    pub fn to_literal(&self) -> String {
        match self {
            OptionValue::List(_) | OptionValue::Map(_) => format!("'{}", self.to_scheme()),
            _ => self.to_scheme(),
        }
    }
}

// ── ConfigWrite ───────────────────────────────────────────────────────────────

/// A structured write to the settings store's `custom` layer — what the
/// settings UI and extension editors change (`Intent::Configure`). The kernel
/// runs it as a store call; the store validates and persists it.
#[derive(Debug, Clone, PartialEq)]
pub enum ConfigWrite {
    SetOption { key: String, value: OptionValue },
    ResetOption { key: String },
    /// Replace a named display order (`extensions`, `help`).
    SetOrder { name: String, items: Vec<String> },
}

impl ConfigWrite {
    /// The store call that performs the write.
    pub fn to_scheme(&self) -> String {
        match self {
            ConfigWrite::SetOption { key, value } => {
                format!("(customize! (opt-set! '{key} {}))", value.to_literal())
            }
            ConfigWrite::ResetOption { key } => format!("(reset! '{key})"),
            ConfigWrite::SetOrder { name, items } => {
                let items: Vec<String> = items.iter().map(|i| format!("{i:?}")).collect();
                format!("(customize! (order! '{name} '({})))", items.join(" "))
            }
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
    /// The default value; always accepted by `kind`.
    pub default: OptionValue,
    /// The id of the extension that declared this option, or `None` for a core
    /// option. Drives settings placement: core options group by `category` at the
    /// settings root; extension options group under `@/settings/Extensions/<owner>`.
    pub owner: Option<String>,
}

// ── OptionStore ───────────────────────────────────────────────────────────────

/// Option definitions and their current typed values. Thread-safe via
/// interior locking; shared as `Arc<OptionStore>`. Every accepted change bumps
/// the [`generation`](Self::generation), so readers can cache derived values.
///
/// Ordering is preserved: categories appear in the order they were first seen
/// across the def stream; defs within a category appear in insertion order.
/// This drives the stable SETTINGS tree.
pub struct OptionStore {
    defs: HashMap<String, OptionDef>,
    def_order: Vec<String>,
    category_order: Vec<String>,
    values: RwLock<HashMap<String, OptionValue>>,
    /// Flat values whose def was absent at `set_str` time; re-bound when
    /// `register` later brings the matching def.
    unowned: RwLock<HashMap<String, String>>,
    generation: AtomicU64,
}

impl OptionStore {
    /// Build from all collected defs; seed `values` from each def's default.
    /// First-wins on duplicate keys.
    pub fn new(defs: Vec<OptionDef>) -> Self {
        let mut def_map: HashMap<String, OptionDef> = HashMap::new();
        let mut def_order: Vec<String> = Vec::new();
        let mut category_order: Vec<String> = Vec::new();
        let mut values: HashMap<String, OptionValue> = HashMap::new();

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
            generation: AtomicU64::new(0),
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
            let restored = self
                .unowned
                .write()
                .unwrap()
                .remove(&def.key)
                .and_then(|flat| OptionValue::parse_flat(&def.kind, &flat));
            let initial = restored.unwrap_or_else(|| def.default.clone());
            self.values.write().unwrap().insert(def.key.clone(), initial);
            self.def_order.push(def.key.clone());
            self.defs.insert(def.key.clone(), def);
        }
        self.generation.fetch_add(1, Ordering::Relaxed);
    }

    /// Set a typed value. Returns `false` (value unchanged) for an unknown key
    /// or a value the def's kind rejects.
    pub fn set(&self, key: &str, val: OptionValue) -> bool {
        match self.defs.get(key) {
            Some(def) if def.kind.accepts(&val) => {
                self.values.write().unwrap().insert(key.into(), val);
                self.generation.fetch_add(1, Ordering::Relaxed);
                true
            }
            _ => false,
        }
    }

    /// Set from the flat string form (see [`OptionValue::parse_flat`]).
    ///
    /// Returns `true` on success. Returns `false` for an invalid value (the
    /// def exists but rejects the value) — the caller may warn; the prior
    /// value stays. Returns `false` for an unknown key — the value is stashed
    /// in `unowned` so a later `register` can bind it.
    pub fn set_str(&self, key: &str, val: &str) -> bool {
        match self.defs.get(key) {
            Some(def) => match OptionValue::parse_flat(&def.kind, val) {
                Some(v) => self.set(key, v),
                None => false,
            },
            None => {
                self.unowned.write().unwrap().insert(key.into(), val.into());
                false
            }
        }
    }

    /// Bumped by every accepted change.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Relaxed)
    }

    /// The current typed value, or `None` for an unknown key.
    pub fn value(&self, key: &str) -> Option<OptionValue> {
        self.values.read().unwrap().get(key).cloned()
    }

    /// The current value in flat string form, or `None` for an unknown key.
    pub fn get(&self, key: &str) -> Option<String> {
        self.value(key).map(|v| v.flat())
    }

    // ── Typed accessors ───────────────────────────────────────────────────────

    /// A `bool` option; false when absent or not a bool.
    pub fn get_bool(&self, key: &str) -> bool {
        matches!(self.value(key), Some(OptionValue::Bool(true)))
    }

    /// An `int` option as `u32`; `0` when absent, negative, or not an int.
    pub fn get_u32(&self, key: &str) -> u32 {
        u32::try_from(self.get_i64(key)).unwrap_or(0)
    }

    /// An `int` option; `0` when absent or not an int.
    pub fn get_i64(&self, key: &str) -> i64 {
        match self.value(key) {
            Some(OptionValue::Int(n)) => n,
            _ => 0,
        }
    }

    /// A `text` / `choice` option (other kinds: their flat form); `""` when absent.
    pub fn get_str(&self, key: &str) -> String {
        self.get(key).unwrap_or_default()
    }

    /// A list-of-text option's items; empty when absent or not a list.
    pub fn get_list(&self, key: &str) -> Vec<String> {
        match self.value(key) {
            Some(OptionValue::List(items)) => items.iter().map(OptionValue::flat).collect(),
            _ => Vec::new(),
        }
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
            if let Some(o) = &d.owner
                && !out.contains(o) {
                    out.push(o.clone());
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

    fn def(key: &str, kind: OptionKind, default: &str) -> OptionDef {
        let default = OptionValue::parse_flat(&kind, default).expect("valid test default");
        OptionDef { key: key.into(), category: "Test".into(), label: key.into(), kind, default, owner: None }
    }

    fn bool_def(key: &str, default: &str) -> OptionDef {
        def(key, OptionKind::Bool, default)
    }

    fn int_def(key: &str, default: &str, min: i64, max: i64) -> OptionDef {
        def(key, OptionKind::Int { min, max }, default)
    }

    fn choice_def(key: &str, default: &str, opts: &[&str]) -> OptionDef {
        def(key, OptionKind::Choice(opts.iter().map(|s| s.to_string()).collect()), default)
    }

    fn list_def(key: &str, default: &str) -> OptionDef {
        def(key, OptionKind::text_list(), default)
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
        assert!(store.set_str("x", "#t"));
        assert!(store.get_bool("x"));
    }

    #[test]
    fn set_invalid_bool_is_rejected() {
        let store = OptionStore::new(vec![bool_def("x", "#f")]);
        assert!(!store.set_str("x", "yes"));
        assert!(!store.get_bool("x"), "value unchanged");
    }

    #[test]
    fn set_int_within_range_accepted() {
        let store = OptionStore::new(vec![int_def("n", "5", 1, 99)]);
        assert!(store.set_str("n", "42"));
        assert_eq!(store.get_i64("n"), 42);
    }

    #[test]
    fn set_int_out_of_range_rejected() {
        let store = OptionStore::new(vec![int_def("n", "5", 1, 99)]);
        assert!(!store.set_str("n", "100"));
        assert_eq!(store.get_i64("n"), 5, "value unchanged");
    }

    #[test]
    fn set_choice_valid_accepted() {
        let store = OptionStore::new(vec![choice_def("c", "a", &["a", "b", "c"])]);
        assert!(store.set_str("c", "b"));
        assert_eq!(store.get_str("c"), "b");
    }

    #[test]
    fn set_choice_invalid_rejected() {
        let store = OptionStore::new(vec![choice_def("c", "a", &["a", "b"])]);
        assert!(!store.set_str("c", "z"));
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
        assert!(!store.set_str("unknown", "val"));
        // value is stashed but not visible via get
        assert!(store.get("unknown").is_none());
    }

    #[test]
    fn register_rebinds_unowned_value() {
        let mut store = OptionStore::new(vec![]);
        // Stash before the def exists.
        store.set_str("x", "#t");
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
                default: OptionValue::Bool(false),
                owner: None,
            },
            OptionDef {
                key: "b.y".into(),
                category: "B".into(),
                label: "y".into(),
                kind: OptionKind::Bool,
                default: OptionValue::Bool(false),
                owner: None,
            },
            OptionDef {
                key: "a.z".into(),
                category: "A".into(),
                label: "z".into(),
                kind: OptionKind::Bool,
                default: OptionValue::Bool(false),
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
            default: OptionValue::Int(def.parse().unwrap()),
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
        store.set_str("preview.tab-width", "6");
        let s = store.settings_for("preview");
        assert_eq!(s.get_u32("preview.tab-width"), 6);
        assert_eq!(s.get_u32("preview.pager"), 2);
        assert_eq!(s.get("core.n"), None, "only the owner's keys are snapshotted");
    }

    #[test]
    fn typed_set_validates_against_the_kind_and_bumps_the_generation() {
        let store = OptionStore::new(vec![int_def("n", "5", 1, 9), list_def("l", "a")]);
        let g0 = store.generation();
        assert!(store.set("n", OptionValue::Int(7)));
        assert!(!store.set("n", OptionValue::Text("7".into())), "wrong kind is rejected");
        assert!(!store.set("n", OptionValue::Int(10)), "out of range is rejected");
        assert_eq!(store.get_i64("n"), 7);
        assert!(store.generation() > g0);
        let g1 = store.generation();
        assert!(!store.set("l", OptionValue::List(vec![OptionValue::Int(1)])), "list items are checked");
        assert_eq!(store.generation(), g1, "a rejected write does not bump");
    }

    #[test]
    fn record_and_map_kinds_accept_only_well_formed_values() {
        let style = OptionKind::Record(vec![("fg".into(), OptionKind::Text), ("bold".into(), OptionKind::Bool)]);
        let themes = OptionKind::Map(Box::new(style.clone()));
        let rec = |fg: &str, bold: bool| {
            OptionValue::Map(
                [("fg".to_string(), OptionValue::Text(fg.into())), ("bold".to_string(), OptionValue::Bool(bold))]
                    .into_iter()
                    .collect(),
            )
        };
        assert!(style.accepts(&rec("#fff", true)));
        let missing = OptionValue::Map([("fg".to_string(), OptionValue::Text("#fff".into()))].into_iter().collect());
        assert!(!style.accepts(&missing), "a record needs every field");
        let map = OptionValue::Map([("dark".to_string(), rec("#000", false))].into_iter().collect());
        assert!(themes.accepts(&map));
        assert!(!themes.accepts(&OptionValue::Map([("x".to_string(), OptionValue::Int(1))].into_iter().collect())));
    }

    #[test]
    fn flat_form_round_trips_scalars_and_lists() {
        let list = OptionKind::text_list();
        let v = OptionValue::parse_flat(&list, " rs, md ,, toml").unwrap();
        assert_eq!(v.flat(), "rs,md,toml");
        assert_eq!(OptionValue::parse_flat(&list, &v.flat()), Some(v));
        assert_eq!(OptionValue::parse_flat(&OptionKind::Bool, "yes"), None);
        assert_eq!(OptionValue::parse_flat(&OptionKind::Int { min: 0, max: 3 }, "4"), None);
        assert_eq!(OptionValue::parse_flat(&OptionKind::Map(Box::new(OptionKind::Text)), "x"), None);
    }

    #[test]
    fn to_scheme_renders_literals() {
        assert_eq!(OptionValue::Bool(true).to_scheme(), "#t");
        assert_eq!(OptionValue::Text("a\"b".into()).to_scheme(), "\"a\\\"b\"");
        let v = OptionValue::List(vec![OptionValue::Text("rs".into()), OptionValue::Int(2)]);
        assert_eq!(v.to_scheme(), "(\"rs\" 2)");
        let m = OptionValue::Map([("k".to_string(), OptionValue::Bool(false))].into_iter().collect());
        assert_eq!(m.to_scheme(), "((\"k\" #f))");
    }
}
