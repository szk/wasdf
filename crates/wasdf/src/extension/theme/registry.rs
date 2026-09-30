//! `ThemeRegistry`: the runtime store for named themes (presets + customs),
//! the active selection, and the `resolve_active → Theme` frame path.

use std::collections::BTreeMap;

use ratatui::style::Color;

use crate::core::{OptionValue, Role, Theme};
use crate::script::sexpr::{self, Datum};

/// Roles bold by default in the built-in look — the single, centralized
/// source of "what bold means" for a theme with no bold override of its own
/// (reproduces the exact spots that were once hard-coded `Modifier::BOLD` in
/// `ui/*.rs`, before themes could carry bold at all). Lives here, not in
/// `core/theme.rs`: `Theme` itself stays a neutral palette primitive: this is
/// theme-extension-owned "default look" data.
pub const DEFAULT_BOLD_ROLES: &[Role] = &[
    Role::EntryDir,
    Role::PathActive,
    Role::NavCursorFg,
    Role::MonitorCpuLabel,
    Role::MonitorMemLabel,
    Role::MonitorDskLabel,
    Role::MonitorNetLabel,
    Role::HelpKeyFg,
];

pub fn default_bold(role: Role) -> bool {
    DEFAULT_BOLD_ROLES.contains(&role)
}

/// `role`'s full look in the built-in theme, absent any override — what an
/// override compares against to decide whether it is redundant. `bg` is
/// always `Color::Reset` ("nil"): `Theme::default()` carries no per-role
/// background opinion of its own.
pub fn default_style(role: Role) -> RoleStyle {
    RoleStyle { fg: Theme::default().role(role), bg: Color::Reset, bold: default_bold(role), italic: false, underline: false }
}

// ── RoleStyle ────────────────────────────────────────────────────────────────

/// One role's complete look: foreground, background (`Color::Reset` = nil /
/// transparent), and the three style modifiers. A theme override is always
/// this whole tuple at once — there is no partial per-attribute override —
/// so editing one attribute of a role never leaves the others ambiguous.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RoleStyle {
    pub fg: Color,
    pub bg: Color,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
}

// ── NamedTheme ────────────────────────────────────────────────────────────────

/// A single named theme: a sparse set of role overrides on top of
/// `Theme::default()`. Presets are immutable; customs are editable.
#[derive(Clone, Default)]
pub struct NamedTheme {
    pub name: String,
    /// true = built-in preset; immutable at runtime (edit offers duplicate).
    pub preset: bool,
    /// Sparse overrides: roles listed here differ (in fg, bg, or any
    /// modifier) from `default_style(role)`.
    pub overrides: Vec<(Role, RoleStyle)>,
}

impl NamedTheme {
    /// A new custom theme named `name`, seeding its overrides from `source`
    /// (or empty if `None`) — the common shape behind "new theme" (seeds from
    /// the active theme) and "duplicate" (seeds from the chosen one), so both
    /// start as an exact copy of their source's look.
    pub fn seeded(name: String, source: Option<&NamedTheme>) -> NamedTheme {
        NamedTheme { name, preset: false, overrides: source.map(|nt| nt.overrides.clone()).unwrap_or_default() }
    }
}

// ── ThemeRegistry ─────────────────────────────────────────────────────────────

pub struct ThemeRegistry {
    themes: Vec<NamedTheme>,
    active: String,
}

impl ThemeRegistry {
    /// Build from preset `NamedTheme`s. A synthetic "default" (preset:true,
    /// one override per `DEFAULT_BOLD_ROLES` role — its `default_style`,
    /// which is already just "the built-in fg, bold") is prepended if absent.
    /// `default_style` is only ever consulted to *decide* whether an edit is
    /// redundant; resolving a theme still only ever applies what's in its own
    /// `overrides`, so "default" needs these written out explicitly to have
    /// any bold roles at all. `active` starts as "default".
    pub fn with_presets(mut presets: Vec<NamedTheme>) -> Self {
        if !presets.iter().any(|t| t.name == "default") {
            let overrides = DEFAULT_BOLD_ROLES.iter().map(|r| (*r, default_style(*r))).collect();
            presets.insert(0, NamedTheme { name: "default".into(), preset: true, overrides });
        }
        ThemeRegistry { themes: presets, active: "default".into() }
    }

    /// The bundled presets (`presets.scm`), plus the synthetic "default".
    pub fn bundled() -> Self {
        ThemeRegistry::with_presets(parse_presets(super::PRESETS_SCHEME))
    }

    /// Replace every custom theme with `customs` (the `theme.custom` option's
    /// current value); presets are untouched.
    pub fn replace_customs(&mut self, customs: Vec<NamedTheme>) {
        self.themes.retain(|t| t.preset);
        self.load_customs(customs);
    }

    /// The custom themes, in order.
    pub fn customs(&self) -> impl Iterator<Item = &NamedTheme> {
        self.themes.iter().filter(|t| !t.preset)
    }

    /// Append or replace custom themes (`preset:false`).
    /// A custom whose name collides with a preset is silently dropped (preset wins).
    pub fn load_customs(&mut self, customs: Vec<NamedTheme>) {
        for custom in customs {
            if self.themes.iter().any(|t| t.preset && t.name == custom.name) {
                crate::services::messages::log(format!("theme: custom '{}' collides with a preset; skipped", custom.name), true);
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
            for (role, style) in &nt.overrides {
                t.set_fg(*role, style.fg);
                t.set_bg(*role, style.bg);
                t.set_bold(*role, style.bold);
                t.set_italic(*role, style.italic);
                t.set_underline(*role, style.underline);
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

// ── presets.scm ──────────────────────────────────────────────────────────────

/// Read `(quote ((theme "name" (role fg bg bold italic underline) …) …))`
/// into presets. Unknown role tokens and malformed role forms are skipped.
pub fn parse_presets(src: &str) -> Vec<NamedTheme> {
    let Ok(datum) = sexpr::parse_quoted(src) else { return Vec::new() };
    let Some(forms) = datum.as_list() else { return Vec::new() };
    forms
        .iter()
        .filter_map(|form| {
            let parts = form.as_list()?;
            if parts.first()?.as_sym()? != "theme" {
                return None;
            }
            let name = parts.get(1)?.as_str()?.to_string();
            Some(NamedTheme { name, preset: true, overrides: role_styles(&parts[2..]) })
        })
        .collect()
}

/// Each `(role fg bg bold italic underline)` form as a role's whole style; a
/// form missing a position, or with an unknown role / color, is skipped.
fn role_styles(forms: &[Datum]) -> Vec<(Role, RoleStyle)> {
    forms
        .iter()
        .filter_map(|f| {
            let p = f.as_list()?;
            Some((
                Role::from_token(p.first()?.as_sym()?)?,
                RoleStyle {
                    fg: color_from_text(p.get(1)?.text()?)?,
                    bg: color_from_text(p.get(2)?.text()?)?,
                    bold: p.get(3)?.as_bool()?,
                    italic: p.get(4)?.as_bool()?,
                    underline: p.get(5)?.as_bool()?,
                },
            ))
        })
        .collect()
}

// ── The theme.custom option ──────────────────────────────────────────────────

/// A color as the option stores it: `"nil"` for no color (`Color::Reset`),
/// otherwise its token (`#rrggbb`, an ANSI name, …).
pub fn color_text(c: Color) -> String {
    if c == Color::Reset { "nil".into() } else { crate::core::color_token(c) }
}

pub fn color_from_text(s: &str) -> Option<Color> {
    if s == "nil" { Some(Color::Reset) } else { crate::core::parse_color(s) }
}

/// The custom themes held in `theme.custom` (theme name → role token → style).
/// Entries that do not decode are skipped.
pub fn customs_from_value(v: &OptionValue) -> Vec<NamedTheme> {
    let OptionValue::Map(themes) = v else { return Vec::new() };
    themes
        .iter()
        .map(|(name, roles)| {
            let overrides = match roles {
                OptionValue::Map(roles) => roles.iter().filter_map(|(tok, style)| Some((Role::from_token(tok)?, style_from_value(style)?))).collect(),
                _ => Vec::new(),
            };
            NamedTheme { name: name.clone(), preset: false, overrides }
        })
        .collect()
}

fn style_from_value(v: &OptionValue) -> Option<RoleStyle> {
    let OptionValue::Map(m) = v else { return None };
    let text = |k: &str| match m.get(k) {
        Some(OptionValue::Text(s)) => color_from_text(s),
        _ => None,
    };
    let flag = |k: &str| match m.get(k) {
        Some(OptionValue::Bool(b)) => Some(*b),
        _ => None,
    };
    Some(RoleStyle { fg: text("fg")?, bg: text("bg")?, bold: flag("bold")?, italic: flag("italic")?, underline: flag("underline")? })
}

/// The value for `theme.custom` holding `themes`.
pub fn customs_to_value<'a>(themes: impl IntoIterator<Item = &'a NamedTheme>) -> OptionValue {
    OptionValue::Map(
        themes
            .into_iter()
            .map(|t| {
                let roles = t
                    .overrides
                    .iter()
                    .map(|(role, st)| {
                        let fields: BTreeMap<String, OptionValue> = [
                            ("fg", OptionValue::Text(color_text(st.fg))),
                            ("bg", OptionValue::Text(color_text(st.bg))),
                            ("bold", OptionValue::Bool(st.bold)),
                            ("italic", OptionValue::Bool(st.italic)),
                            ("underline", OptionValue::Bool(st.underline)),
                        ]
                        .into_iter()
                        .map(|(k, v)| (k.to_string(), v))
                        .collect();
                        (role.token().to_string(), OptionValue::Map(fields))
                    })
                    .collect();
                (t.name.clone(), OptionValue::Map(roles))
            })
            .collect(),
    )
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn style(fg: Color, bold: bool) -> RoleStyle {
        RoleStyle { fg, bg: Color::Reset, bold, italic: false, underline: false }
    }

    #[test]
    fn upsert_custom_replaces_overrides_on_an_existing_entry() {
        let mut reg = ThemeRegistry::with_presets(vec![]);
        reg.upsert_custom(NamedTheme { name: "x".into(), preset: false, overrides: vec![] });
        assert!(!reg.resolve("x").bold(Role::Title));

        reg.upsert_custom(NamedTheme {
            name: "x".into(),
            preset: false,
            overrides: vec![(Role::Title, style(Color::Reset, true))],
        });
        assert!(reg.resolve("x").bold(Role::Title), "override must survive an upsert onto an existing theme");
    }

    #[test]
    fn resolve_applies_fg_bg_and_every_modifier_from_one_overrides_entry() {
        let mut reg = ThemeRegistry::with_presets(vec![]);
        reg.upsert_custom(NamedTheme {
            name: "x".into(),
            preset: false,
            overrides: vec![(Role::Title, RoleStyle {
                fg: Color::Rgb(1, 2, 3),
                bg: Color::Rgb(4, 5, 6),
                bold: true,
                italic: true,
                underline: true,
            })],
        });
        let resolved = reg.resolve("x");
        assert_eq!(resolved.role(Role::Title), Color::Rgb(1, 2, 3));
        assert_eq!(resolved.bg(Role::Title), Color::Rgb(4, 5, 6));
        assert!(resolved.bold(Role::Title));
        assert!(resolved.italic(Role::Title));
        assert!(resolved.underline(Role::Title));
        // Untouched role stays at Theme::default()'s own look.
        assert_eq!(resolved.bg(Role::Border), Color::Reset);
    }

    #[test]
    fn default_style_reflects_default_bold_roles_and_theme_default_colors() {
        let s = default_style(Role::EntryDir);
        assert_eq!(s.fg, Theme::default().role(Role::EntryDir));
        assert_eq!(s.bg, Color::Reset);
        assert!(s.bold, "EntryDir is bold in the built-in look");
        assert!(!default_style(Role::Title).bold);
    }

    #[test]
    fn customs_round_trip_through_the_option_value() {
        let themes = vec![NamedTheme {
            name: "mine".into(),
            preset: false,
            overrides: vec![(Role::Title, RoleStyle { fg: Color::Rgb(1, 2, 3), bg: Color::Reset, bold: true, italic: false, underline: true })],
        }];
        let v = customs_to_value(&themes);
        let back = customs_from_value(&v);
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].name, "mine");
        assert_eq!(back[0].overrides, themes[0].overrides);
    }

    #[test]
    fn replace_customs_keeps_presets_and_drops_old_customs() {
        let mut reg = ThemeRegistry::bundled();
        reg.upsert_custom(NamedTheme { name: "a".into(), preset: false, overrides: vec![] });
        reg.replace_customs(vec![NamedTheme { name: "b".into(), preset: false, overrides: vec![] }]);
        assert!(reg.get("a").is_none() && reg.get("b").is_some());
        assert!(reg.get("default").is_some_and(|t| t.preset));
    }
}
