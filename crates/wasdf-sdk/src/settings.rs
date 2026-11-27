//! Settings: the shared vocabulary and read logic for configurable options,
//! agreed between the kernel and dynamically loaded extensions.
//!
//! Option values travel in their canonical **scheme-string** form (`"#t"`,
//! `"4"`, `"dark"`, `"a,b"`). The kernel's option store and a dynamic extension
//! (handed its values over the ABI) both parse them through the free functions
//! here, so a `bool` / `int` / `list` never means two different things on the two
//! sides. `Settings` is a small map wrapper with the same typed accessors.

use std::collections::HashMap;

/// A declared setting: how an extension advertises one configurable option (the
/// same five fields the kernel reads from an `(option …)` declaration).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingSpec {
    /// Dotted key, conventionally `<ext-id>.<name>` (e.g. `preview.tab-width`).
    pub key: String,
    /// Display grouping / category label.
    pub category: String,
    /// The kind tag (`bool` / `int` / `choice` / `text` / `list`).
    pub kind: String,
    /// Default value in scheme-string form.
    pub default: String,
    /// Human label shown in the settings UI.
    pub label: String,
}

/// A read-only view over option values in scheme-string form, with typed
/// accessors. Built from `(key, value)` pairs the kernel supplies.
#[derive(Debug, Clone, Default)]
pub struct Settings {
    values: HashMap<String, String>,
}

impl Settings {
    /// Build from `(key, value)` pairs (values in scheme-string form).
    pub fn from_pairs(pairs: impl IntoIterator<Item = (String, String)>) -> Self {
        Settings { values: pairs.into_iter().collect() }
    }

    /// Parse a Scheme association list of string pairs — `(("key" "value") …)` —
    /// into a `Settings`. This is the wire form the kernel hands a dynamically
    /// loaded extension over the ABI (the extension's own option snapshot): each
    /// value arrives in scheme-string form as a quoted string, so both sides read
    /// a bool / int / list identically through the typed accessors. Quoted strings
    /// may contain `\"` and `\\` escapes; anything malformed is skipped.
    pub fn from_scheme_alist(src: &str) -> Self {
        // Extract quoted strings left-to-right; they arrive as key, value, key, …
        // (every element of every pair is a quoted string in this wire form).
        let mut strings: Vec<String> = Vec::new();
        let mut chars = src.chars().peekable();
        while let Some(c) = chars.next() {
            if c != '"' {
                continue;
            }
            let mut s = String::new();
            while let Some(&d) = chars.peek() {
                chars.next();
                match d {
                    '\\' => {
                        if let Some(&e) = chars.peek() {
                            chars.next();
                            s.push(e);
                        }
                    }
                    '"' => break,
                    _ => s.push(d),
                }
            }
            strings.push(s);
        }
        Settings {
            values: strings.as_chunks::<2>().0.iter().map(|p| (p[0].clone(), p[1].clone())).collect(),
        }
    }

    /// Insert or replace one value.
    pub fn set(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.values.insert(key.into(), value.into());
    }

    /// The raw scheme-string value, or `None` if the key is absent.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.values.get(key).map(String::as_str)
    }

    /// `"#t"` → true; anything else (including absent) → false.
    pub fn get_bool(&self, key: &str) -> bool {
        parse_bool(self.get(key))
    }
    /// Parse as `i64`; `0` on absent / parse failure.
    pub fn get_i64(&self, key: &str) -> i64 {
        parse_i64(self.get(key))
    }
    /// Parse as `u32`; `0` on absent / parse failure.
    pub fn get_u32(&self, key: &str) -> u32 {
        parse_u32(self.get(key))
    }
    /// The value as a `String`; `""` when absent.
    pub fn get_str(&self, key: &str) -> String {
        self.get(key).unwrap_or("").to_string()
    }
    /// Split a `list` value on `,`, trimming each part and dropping empties.
    pub fn get_list(&self, key: &str) -> Vec<String> {
        parse_list(self.get(key))
    }
}

// ── Value-string parsing (the single source of truth) ──────────────────────────

/// `Some("#t")` → true; anything else → false.
pub fn parse_bool(v: Option<&str>) -> bool {
    v == Some("#t")
}

/// Parse as `i64`; `0` on absent / parse failure.
pub fn parse_i64(v: Option<&str>) -> i64 {
    v.and_then(|s| s.parse().ok()).unwrap_or(0)
}

/// Parse as `u32`; `0` on absent / parse failure.
pub fn parse_u32(v: Option<&str>) -> u32 {
    v.and_then(|s| s.parse().ok()).unwrap_or(0)
}

/// Split on `,`, trim each part, drop empties.
pub fn parse_list(v: Option<&str>) -> Vec<String> {
    v.unwrap_or("")
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}
