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
            values: strings.chunks_exact(2).map(|p| (p[0].clone(), p[1].clone())).collect(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_reads_match_the_scheme_string_forms() {
        let s = Settings::from_pairs([
            ("flag".into(), "#t".into()),
            ("count".into(), "4".into()),
            ("name".into(), "dark".into()),
            ("exts".into(), "rs, toml , ,md".into()),
        ]);
        assert!(s.get_bool("flag"));
        assert!(!s.get_bool("missing"));
        assert_eq!(s.get_u32("count"), 4);
        assert_eq!(s.get_i64("count"), 4);
        assert_eq!(s.get_str("name"), "dark");
        assert_eq!(s.get_list("exts"), vec!["rs", "toml", "md"]);
        assert_eq!(s.get_i64("missing"), 0);
        assert_eq!(s.get_str("missing"), "");
    }

    #[test]
    fn scheme_alist_round_trips_string_pairs() {
        let s = Settings::from_scheme_alist(
            r##"(("exampleext.greeting" "hi there") ("exampleext.verbose" "#f"))"##,
        );
        assert_eq!(s.get_str("exampleext.greeting"), "hi there");
        assert!(!s.get_bool("exampleext.verbose"));
        assert_eq!(s.get("missing"), None);
    }

    #[test]
    fn scheme_alist_handles_escapes_and_empty() {
        let s = Settings::from_scheme_alist(r#"(("k" "a\"b\\c"))"#);
        assert_eq!(s.get_str("k"), "a\"b\\c");
        assert_eq!(Settings::from_scheme_alist("()").get("k"), None);
    }

    #[test]
    fn free_functions_are_the_shared_parsers() {
        assert!(parse_bool(Some("#t")));
        assert!(!parse_bool(Some("#f")));
        assert_eq!(parse_u32(Some("7")), 7);
        assert_eq!(parse_list(Some("a, b,c")), vec!["a", "b", "c"]);
    }
}
