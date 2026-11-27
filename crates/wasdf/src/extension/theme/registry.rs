//! `ThemeRegistry`: the runtime store for named themes (presets + customs),
//! the active selection, and the `resolve_active → Theme` frame path.

use std::collections::BTreeMap;

use ratatui::style::Color;

use crate::core::{OptionValue, Role, Theme};
use crate::script::sexpr::{self, Datum};

/// Roles bold by default in the built-in look — the single, centralized
/// source of "what bold means" for a theme with no bold override of its own.
/// Lives here, not in
/// `core/theme.rs`: `Theme` itself stays a neutral palette primitive: this is
/// theme-extension-owned "default look" data.
pub const DEFAULT_BOLD_ROLES: &[Role] = &[
    Role::EntryDir,
    Role::PathActive,
    Role::NavCursor,
    Role::MonitorCpuLabel,
    Role::MonitorMemLabel,
    Role::MonitorDskLabel,
    Role::MonitorNetLabel,
    Role::HelpKey,
];

pub fn default_bold(role: Role) -> bool {
    DEFAULT_BOLD_ROLES.contains(&role)
}

/// `role`'s full look in the built-in theme, absent any override — what an
/// override compares against to decide whether it is redundant.
pub fn default_style(role: Role) -> RoleStyle {
    let t = Theme::default();
    RoleStyle { fg: t.role(role), bg: t.bg(role), bold: default_bold(role), italic: false, underline: false }
}

// ── RoleStyle ────────────────────────────────────────────────────────────────

/// One role's complete look: foreground, background (`Color::Reset` = nil /
/// transparent), and the three style modifiers. A theme override is always
/// this whole tuple at once, so editing one attribute of a role never leaves
/// the others ambiguous; written out, an attribute at its `Default` (nil, off)
/// is simply omitted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RoleStyle {
    pub fg: Color,
    pub bg: Color,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
}

impl Default for RoleStyle {
    fn default() -> Self {
        RoleStyle { fg: Color::Reset, bg: Color::Reset, bold: false, italic: false, underline: false }
    }
}

impl RoleStyle {
    /// Read `(fg C) (bg C) (bold B) (italic B) (underline B)` field pairs, any
    /// subset in any order. `None` for an unknown field or an ill-typed value.
    fn from_fields<'a>(fields: impl IntoIterator<Item = (&'a str, FieldValue<'a>)>) -> Option<RoleStyle> {
        let mut st = RoleStyle::default();
        for (name, v) in fields {
            match (name, v) {
                ("fg", FieldValue::Color(c)) => st.fg = color_from_text(c)?,
                ("bg", FieldValue::Color(c)) => st.bg = color_from_text(c)?,
                ("bold", FieldValue::Flag(b)) => st.bold = b,
                ("italic", FieldValue::Flag(b)) => st.italic = b,
                ("underline", FieldValue::Flag(b)) => st.underline = b,
                _ => return None,
            }
        }
        Some(st)
    }

    /// The fields that differ from `Default`, in declaration order.
    fn to_fields(self) -> Vec<(&'static str, OptionValue)> {
        let d = RoleStyle::default();
        let mut out = Vec::new();
        if self.fg != d.fg {
            out.push(("fg", OptionValue::Text(color_text(self.fg))));
        }
        if self.bg != d.bg {
            out.push(("bg", OptionValue::Text(color_text(self.bg))));
        }
        for (name, on) in [("bold", self.bold), ("italic", self.italic), ("underline", self.underline)] {
            if on {
                out.push((name, OptionValue::Bool(true)));
            }
        }
        out
    }
}

/// A field value as either source spells it: a color token or a flag.
enum FieldValue<'a> {
    Color(&'a str),
    Flag(bool),
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

/// Read `(quote ((theme "name" (role (fg C) (bg C) (bold B) …) …) …))` into
/// presets. Unknown role tokens and malformed role forms are skipped.
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

/// Each `(role (field value) …)` form as a role's whole style; a form with an
/// unknown role, field, or color is skipped.
fn role_styles(forms: &[Datum]) -> Vec<(Role, RoleStyle)> {
    forms
        .iter()
        .filter_map(|f| {
            let (role, fields) = f.as_list()?.split_first()?;
            let fields = fields
                .iter()
                .map(|field| {
                    let [name, value] = field.as_list()? else { return None };
                    let v = match value.as_bool() {
                        Some(b) => FieldValue::Flag(b),
                        None => FieldValue::Color(value.text()?),
                    };
                    Some((name.as_sym()?, v))
                })
                .collect::<Option<Vec<_>>>()?;
            Some((Role::from_token(role.as_sym()?)?, RoleStyle::from_fields(fields)?))
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
    let fields = m
        .iter()
        .map(|(k, v)| match v {
            OptionValue::Bool(b) => Some((k.as_str(), FieldValue::Flag(*b))),
            OptionValue::Text(s) => Some((k.as_str(), FieldValue::Color(s))),
            _ => None,
        })
        .collect::<Option<Vec<_>>>()?;
    RoleStyle::from_fields(fields)
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
                        let fields: BTreeMap<String, OptionValue> =
                            st.to_fields().into_iter().map(|(k, v)| (k.to_string(), v)).collect();
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

}
