//! `PaletteRegistry`: the runtime store for named ls-colors palettes (presets +
//! customs), the active selection, and the `resolve_active → LsColorsTable`
//! path — the ls-colors analogue of `theme::registry::ThemeRegistry`.

use super::parse::{self, LsColorsTable};

// ── NamedPalette ──────────────────────────────────────────────────────────────

/// A single named ls-colors palette: a raw `LS_COLORS`-format string
/// (`key=SGR:key=SGR:…`, see `parse::parse_ls_colors`). Presets are immutable;
/// customs are editable.
#[derive(Clone)]
pub struct NamedPalette {
    pub name: String,
    /// true = built-in preset; immutable at runtime (edit offers duplicate).
    pub preset: bool,
    pub raw: String,
}

// ── PaletteRegistry ───────────────────────────────────────────────────────────

pub struct PaletteRegistry {
    palettes: Vec<NamedPalette>,
    active: String,
}

impl PaletteRegistry {
    /// Build from preset `NamedPalette`s. `active` starts as the first preset's
    /// name (empty if there are no presets at all).
    pub fn with_presets(presets: Vec<NamedPalette>) -> Self {
        let active = presets.first().map(|p| p.name.clone()).unwrap_or_default();
        PaletteRegistry { palettes: presets, active }
    }

    /// Append or replace custom palettes (`preset:false`) parsed from config.scm.
    /// A custom whose name collides with a preset is silently dropped (preset wins).
    pub fn load_customs(&mut self, customs: Vec<NamedPalette>) {
        for custom in customs {
            if self.palettes.iter().any(|p| p.preset && p.name == custom.name) {
                crate::services::messages::log(format!("lscolors: custom '{}' collides with a preset; skipped", custom.name), true);
                continue;
            }
            if let Some(existing) = self.palettes.iter_mut().find(|p| !p.preset && p.name == custom.name) {
                existing.raw = custom.raw;
            } else {
                self.palettes.push(custom);
            }
        }
    }

    pub fn active(&self) -> &str {
        &self.active
    }

    /// Set the active palette by name. Returns false (keeps prior) if unknown.
    pub fn set_active(&mut self, name: &str) -> bool {
        if self.palettes.iter().any(|p| p.name == name) {
            self.active = name.into();
            true
        } else {
            false
        }
    }

    /// Resolve `name` → a parsed `LsColorsTable`. Unknown name yields an empty table.
    pub fn resolve(&self, name: &str) -> LsColorsTable {
        self.palettes
            .iter()
            .find(|p| p.name == name)
            .map(|p| parse::parse_ls_colors(&p.raw))
            .unwrap_or_default()
    }

    /// The active palette, parsed. Callers should cache this — parsing runs
    /// fresh on every call.
    pub fn resolve_active(&self) -> LsColorsTable {
        self.resolve(&self.active)
    }

    pub fn names(&self) -> Vec<String> {
        self.palettes.iter().map(|p| p.name.clone()).collect()
    }

    pub fn get(&self, name: &str) -> Option<&NamedPalette> {
        self.palettes.iter().find(|p| p.name == name)
    }

    /// Insert or replace a custom palette. Silently rejects `preset:true` entries.
    pub fn upsert_custom(&mut self, p: NamedPalette) {
        if p.preset {
            return;
        }
        if let Some(existing) = self.palettes.iter_mut().find(|x| x.name == p.name) {
            if existing.preset {
                return;
            }
            existing.raw = p.raw;
        } else {
            self.palettes.push(p);
        }
    }

    /// Remove a custom palette by name. Returns false if preset or unknown.
    pub fn remove_custom(&mut self, name: &str) -> bool {
        let pos = self.palettes.iter().position(|p| p.name == name && !p.preset);
        if let Some(i) = pos {
            self.palettes.remove(i);
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

    #[test]
    fn with_presets_defaults_active_to_the_first_preset() {
        let reg = PaletteRegistry::with_presets(vec![
            NamedPalette { name: "dark".into(), preset: true, raw: "di=01;34".into() },
            NamedPalette { name: "light".into(), preset: true, raw: "di=01;34".into() },
        ]);
        assert_eq!(reg.active(), "dark");
    }

    #[test]
    fn upsert_custom_replaces_raw_on_an_existing_entry() {
        let mut reg = PaletteRegistry::with_presets(vec![]);
        reg.upsert_custom(NamedPalette { name: "x".into(), preset: false, raw: "di=01;34".into() });
        assert_eq!(reg.get("x").unwrap().raw, "di=01;34");

        reg.upsert_custom(NamedPalette { name: "x".into(), preset: false, raw: "di=01;36".into() });
        assert_eq!(reg.get("x").unwrap().raw, "di=01;36", "raw must be replaced on an upsert onto an existing palette");
    }

    #[test]
    fn upsert_custom_rejects_preset_collision() {
        let mut reg = PaletteRegistry::with_presets(vec![
            NamedPalette { name: "dark".into(), preset: true, raw: "di=01;34".into() },
        ]);
        reg.upsert_custom(NamedPalette { name: "dark".into(), preset: false, raw: "di=01;36".into() });
        assert!(reg.get("dark").unwrap().preset, "preset must not be overwritten by a custom");
    }

    #[test]
    fn remove_custom_rejects_preset() {
        let mut reg = PaletteRegistry::with_presets(vec![
            NamedPalette { name: "dark".into(), preset: true, raw: "di=01;34".into() },
        ]);
        assert!(!reg.remove_custom("dark"));
        assert!(reg.get("dark").is_some());
    }
}
