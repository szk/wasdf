//! `ThemeRegistry`: the runtime store for named themes (presets + customs),
//! the active selection, and the `resolve_active → Theme` frame path.

use ratatui::style::Color;

use crate::core::{Role, Theme};

// ── NamedTheme ────────────────────────────────────────────────────────────────

/// A single named theme: a sparse set of role overrides on top of
/// `Theme::default()`. Presets are immutable; customs are editable.
#[derive(Clone)]
pub struct NamedTheme {
    pub name: String,
    /// true = built-in preset; immutable at runtime (edit offers duplicate).
    pub preset: bool,
    /// Sparse overrides: roles listed here differ from `Theme::default()`.
    pub overrides: Vec<(Role, Color)>,
}

// ── ThemeRegistry ─────────────────────────────────────────────────────────────

pub struct ThemeRegistry {
    themes: Vec<NamedTheme>,
    active: String,
}

impl ThemeRegistry {
    /// Build from preset `NamedTheme`s. A synthetic "default" (empty overrides,
    /// preset:true) is prepended if absent. `active` starts as "default".
    pub fn with_presets(mut presets: Vec<NamedTheme>) -> Self {
        if !presets.iter().any(|t| t.name == "default") {
            presets.insert(0, NamedTheme { name: "default".into(), preset: true, overrides: vec![] });
        }
        ThemeRegistry { themes: presets, active: "default".into() }
    }

    /// Append or replace custom themes (`preset:false`) parsed from config.scm.
    /// A custom whose name collides with a preset is silently dropped (preset wins).
    pub fn load_customs(&mut self, customs: Vec<NamedTheme>) {
        for custom in customs {
            if self.themes.iter().any(|t| t.preset && t.name == custom.name) {
                eprintln!("theme: custom '{}' collides with a preset; skipped", custom.name);
                continue;
            }
            if let Some(existing) = self.themes.iter_mut().find(|t| !t.preset && t.name == custom.name) {
                existing.overrides = custom.overrides;
            } else {
                self.themes.push(custom);
            }
        }
    }

    pub fn active(&self) -> &str {
        &self.active
    }

    /// Set the active theme by name. Returns false (keeps prior) if unknown.
    pub fn set_active(&mut self, name: &str) -> bool {
        if self.themes.iter().any(|t| t.name == name) {
            self.active = name.into();
            true
        } else {
            false
        }
    }

    /// Resolve `name` → `Theme` by overlaying its overrides on `Theme::default()`.
    /// Unknown name yields `Theme::default()`.
    pub fn resolve(&self, name: &str) -> Theme {
        let mut t = Theme::default();
        if let Some(nt) = self.themes.iter().find(|t| t.name == name) {
            for (role, color) in &nt.overrides {
                t.set(*role, *color);
            }
        }
        t
    }

    /// The frame palette: `resolve(active)`. Called once per frame by event_loop.
    pub fn resolve_active(&self) -> Theme {
        self.resolve(&self.active)
    }

    pub fn names(&self) -> Vec<String> {
        self.themes.iter().map(|t| t.name.clone()).collect()
    }

    pub fn get(&self, name: &str) -> Option<&NamedTheme> {
        self.themes.iter().find(|t| t.name == name)
    }

    /// Insert or replace a custom theme. Silently rejects `preset:true` entries.
    pub fn upsert_custom(&mut self, t: NamedTheme) {
        if t.preset {
            return;
        }
        if let Some(existing) = self.themes.iter_mut().find(|x| x.name == t.name) {
            if existing.preset {
                return;
            }
            existing.overrides = t.overrides;
        } else {
            self.themes.push(t);
        }
    }

    /// Remove a custom theme by name. Returns false if preset or unknown.
    pub fn remove_custom(&mut self, name: &str) -> bool {
        let pos = self.themes.iter().position(|t| t.name == name && !t.preset);
        if let Some(i) = pos {
            self.themes.remove(i);
            true
        } else {
            false
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::Role;

    fn nord_theme() -> NamedTheme {
        NamedTheme {
            name: "nord".into(),
            preset: true,
            overrides: vec![(Role::Border, ratatui::style::Color::Rgb(59, 66, 82))],
        }
    }

    fn registry() -> ThemeRegistry {
        ThemeRegistry::with_presets(vec![nord_theme()])
    }

    #[test]
    fn resolve_unknown_name_is_default() {
        let r = registry();
        let got = r.resolve("nope");
        let expected = Theme::default();
        assert_eq!(got.role(Role::Border), expected.role(Role::Border));
        assert_eq!(got.role(Role::Accent), expected.role(Role::Accent));
    }

    #[test]
    fn resolve_overlays_overrides_on_default() {
        let r = registry();
        let got = r.resolve("nord");
        assert_eq!(got.role(Role::Border), ratatui::style::Color::Rgb(59, 66, 82));
        assert_eq!(got.role(Role::Accent), Theme::default().role(Role::Accent));
    }

    #[test]
    fn set_active_unknown_keeps_prior() {
        let mut r = registry();
        let was = r.active().to_string();
        assert!(!r.set_active("nope"));
        assert_eq!(r.active(), was);
    }

    #[test]
    fn resolve_active_follows_selection() {
        let mut r = registry();
        assert!(r.set_active("nord"));
        let active = r.resolve_active();
        let direct = r.resolve("nord");
        assert_eq!(active.role(Role::Border), direct.role(Role::Border));
    }

    #[test]
    fn preset_is_immutable() {
        let mut r = registry();
        assert!(!r.remove_custom("nord"), "preset cannot be removed");
        assert!(!r.remove_custom("default"), "built-in default cannot be removed");
        let preset = NamedTheme { name: "nord".into(), preset: true, overrides: vec![] };
        r.upsert_custom(preset);
        assert_eq!(r.resolve("nord").role(Role::Border), ratatui::style::Color::Rgb(59, 66, 82),
            "upsert_custom should not overwrite a preset");
    }

    #[test]
    fn load_customs_skips_preset_collision() {
        let mut r = registry();
        r.load_customs(vec![NamedTheme { name: "nord".into(), preset: false, overrides: vec![] }]);
        assert_eq!(r.resolve("nord").role(Role::Border), ratatui::style::Color::Rgb(59, 66, 82),
            "custom with preset name should be dropped");
    }

    #[test]
    fn upsert_and_remove_custom() {
        let mut r = registry();
        r.upsert_custom(NamedTheme {
            name: "ocean".into(),
            preset: false,
            overrides: vec![(Role::Accent, ratatui::style::Color::Blue)],
        });
        assert!(r.names().contains(&"ocean".to_string()));
        assert_eq!(r.resolve("ocean").role(Role::Accent), ratatui::style::Color::Blue);
        assert!(r.remove_custom("ocean"));
        assert!(!r.names().contains(&"ocean".to_string()));
    }
}
