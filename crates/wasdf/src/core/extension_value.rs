//! ExtensionValue: the structural data carried by extension intents and Scheme
//! expressions. No downcasting — consumers read it structurally.

use std::collections::BTreeMap;
use std::path::PathBuf;

/// Reserved data key: filled by the Emit confirm action with a confirm shape.
pub const KEY_ITEM: &str = "item";
/// Reserved data key: marks an intent as a resolver plan executed by the kernel.
pub const KEY_RESOLVER: &str = "resolver";

#[derive(Debug, Clone, PartialEq)]
pub enum ExtensionValue {
    Nil,
    Bool(bool),
    Int(i64),
    String(String),
    Path(PathBuf),
    List(Vec<ExtensionValue>),
    Map(BTreeMap<String, ExtensionValue>),
}

impl Default for ExtensionValue {
    fn default() -> Self {
        ExtensionValue::Nil
    }
}

impl ExtensionValue {
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            ExtensionValue::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_int(&self) -> Option<i64> {
        match self {
            ExtensionValue::Int(i) => Some(*i),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            ExtensionValue::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_path(&self) -> Option<&PathBuf> {
        match self {
            ExtensionValue::Path(p) => Some(p),
            _ => None,
        }
    }

    pub fn as_list(&self) -> Option<&[ExtensionValue]> {
        match self {
            ExtensionValue::List(v) => Some(v),
            _ => None,
        }
    }

    /// Look up a key when this value is a Map.
    pub fn get(&self, key: &str) -> Option<&ExtensionValue> {
        match self {
            ExtensionValue::Map(m) => m.get(key),
            _ => None,
        }
    }

    /// Build a Map from key/value pairs.
    pub fn map(pairs: impl IntoIterator<Item = (String, ExtensionValue)>) -> Self {
        ExtensionValue::Map(pairs.into_iter().collect())
    }

    /// Return a copy with `key` set to `value` (Maps only; otherwise a new Map).
    pub fn with(&self, key: &str, value: ExtensionValue) -> Self {
        let mut m = match self {
            ExtensionValue::Map(m) => m.clone(),
            _ => BTreeMap::new(),
        };
        m.insert(key.to_string(), value);
        ExtensionValue::Map(m)
    }
}
