//! Decoding parsed config data (s-expression datums) into kernel registry
//! types: resolver entries, palette commands, and keymap bindings. Shared by
//! the embedded-default loader (kernel boot) and the optional-extension loader.

use crate::core::{OptionDef, OptionKind};
use crate::script::codec;
use crate::script::condition::Cond;
use crate::script::keymap::{Binding, Layer};
use crate::script::sexpr::Datum;
use crate::services::command::CommandDef;
use crate::services::resolver::{ArgEl, Candidate, Kind, OptDef, ResolverEntry, Slot};

/// Map a resolver-config datum (a list of `(key destructive (candidate…))`)
/// into resolver entries.
pub fn parse_resolver_config(datum: &Datum) -> Result<Vec<ResolverEntry>, String> {
    let entries = datum.as_list().ok_or("resolver config is not a list")?;
    let mut out = Vec::new();
    for entry in entries {
        let parts = entry.as_list().ok_or("resolver entry is not a list")?;
        let key = parts
            .first()
            .and_then(|d| d.as_sym())
            .ok_or("resolver entry missing key")?
            .to_string();
        let destructive = parts.get(1).and_then(|d| d.as_bool()).unwrap_or(false);
        let cand_data = parts.get(2).and_then(|d| d.as_list()).ok_or("missing candidates")?;
        // Optional 4th element: selectable options, a list of (token label) pairs.
        let options = match parts.get(3).and_then(|d| d.as_list()) {
            Some(opts) => opts
                .iter()
                .filter_map(|o| {
                    let ol = o.as_list()?;
                    Some(OptDef {
                        token: ol.first()?.as_str()?.to_string(),
                        label: ol.get(1).and_then(|d| d.as_str()).unwrap_or("").to_string(),
                    })
                })
                .collect(),
            None => Vec::new(),
        };
        let mut candidates = Vec::new();
        for c in cand_data {
            let cl = c.as_list().ok_or("candidate is not a list")?;
            let kind = match cl.first().and_then(|d| d.as_sym()) {
                Some("native") => Kind::Native,
                Some("uutils") => Kind::Uutils,
                Some("native-macos") => Kind::NativeMacos,
                Some("native-linux") => Kind::NativeLinux,
                other => return Err(format!("unknown candidate kind: {other:?}")),
            };
            let mut argv = Vec::new();
            for a in &cl[1..] {
                argv.push(match a {
                    Datum::Str(s) => ArgEl::Lit(s.clone()),
                    Datum::Sym(s) => match s.as_str() {
                        "src" => ArgEl::Ph(Slot::Src),
                        "dst" => ArgEl::Ph(Slot::Dst),
                        "path" => ArgEl::Ph(Slot::Path),
                        "paths" => ArgEl::Ph(Slot::Paths),
                        "opts" => ArgEl::Ph(Slot::Opts),
                        _ => ArgEl::Lit(s.clone()),
                    },
                    _ => return Err("invalid argv element".into()),
                });
            }
            candidates.push(Candidate { kind, argv });
        }
        out.push(ResolverEntry { key, destructive, options, candidates });
    }
    Ok(out)
}

/// Map a command-config datum (a list of `(name description intent)`) into
/// palette commands.
pub fn parse_command_config(datum: &Datum) -> Result<Vec<CommandDef>, String> {
    let list = datum.as_list().ok_or("command config is not a list")?;
    let mut out = Vec::new();
    for entry in list {
        let parts = entry.as_list().ok_or("command entry is not a list")?;
        let name = parts.first().and_then(|d| d.as_sym()).ok_or("command missing name")?;
        let description =
            parts.get(1).and_then(|d| d.as_str()).ok_or("command missing description")?;
        let intent = parts
            .get(2)
            .and_then(codec::intent_from_datum)
            .ok_or_else(|| format!("command '{name}' has an unknown intent"))?;
        out.push(CommandDef {
            name: name.to_string(),
            description: description.to_string(),
            intent,
        });
    }
    Ok(out)
}

/// Map an option-config datum (the inner list of a `(quote (…))` form) into
/// `OptionDef` records. Each element must be an `(option key category kind
/// default label)` form as documented in `plan/scm_setting.md §4`.
pub fn parse_option_config(datum: &Datum) -> Result<Vec<OptionDef>, String> {
    let forms = datum.as_list().ok_or("option config is not a list")?;
    let mut out = Vec::new();
    for form in forms {
        let parts = form.as_list().ok_or("option form is not a list")?;
        let head =
            parts.first().and_then(|d| d.as_sym()).ok_or("option form missing head symbol")?;
        if head != "option" {
            return Err(format!("expected 'option' form, got '{head}'"));
        }
        let key =
            parts.get(1).and_then(|d| d.as_str()).ok_or("option missing key")?.to_string();
        let category = parts
            .get(2)
            .and_then(|d| d.as_str())
            .ok_or("option missing category")?
            .to_string();
        let kind_datum = parts.get(3).ok_or("option missing kind")?;
        let kind = parse_option_kind(kind_datum)?;
        let default =
            parts.get(4).and_then(|d| d.as_str()).ok_or("option missing default")?.to_string();
        let label =
            parts.get(5).and_then(|d| d.as_str()).ok_or("option missing label")?.to_string();
        // Parsed defs are unowned; the kernel tags an extension's defs with its id.
        out.push(OptionDef { key, category, label, kind, default, owner: None });
    }
    Ok(out)
}

fn parse_option_kind(datum: &Datum) -> Result<OptionKind, String> {
    match datum {
        Datum::Sym(s) => match s.as_str() {
            "bool" => Ok(OptionKind::Bool),
            "text" => Ok(OptionKind::Text),
            "list" => Ok(OptionKind::List),
            other => Err(format!("unknown option kind symbol: {other}")),
        },
        Datum::List(items) => {
            let head = items
                .first()
                .and_then(|d| d.as_sym())
                .ok_or("kind list missing head symbol")?;
            match head {
                "int" => {
                    let min = match items.get(1) {
                        Some(Datum::Int(n)) => *n,
                        _ => return Err("int kind missing or non-integer min".into()),
                    };
                    let max = match items.get(2) {
                        Some(Datum::Int(n)) => *n,
                        _ => return Err("int kind missing or non-integer max".into()),
                    };
                    Ok(OptionKind::Int { min, max })
                }
                "choice" => {
                    let choices: Vec<String> =
                        items[1..].iter().filter_map(|d| d.as_str().map(String::from)).collect();
                    if choices.is_empty() {
                        return Err("choice kind has no options".into());
                    }
                    Ok(OptionKind::Choice(choices))
                }
                other => Err(format!("unknown kind form head: {other}")),
            }
        }
        _ => Err("option kind must be a symbol or list".into()),
    }
}

/// Map a keymap-config datum (a list of groups) into bindings at `layer`.
pub fn parse_keymap_config(datum: &Datum, layer: Layer) -> Result<Vec<Binding>, String> {
    let groups = datum.as_list().ok_or("keymap config is not a list")?;
    let mut out = Vec::new();
    for group in groups {
        let gp = group.as_list().ok_or("keymap group is not a list")?;
        let mode = gp.first().and_then(|d| d.as_sym()).ok_or("group missing mode")?.to_string();
        let panel = match gp.get(1) {
            Some(Datum::Bool(false)) => None,
            Some(d) => d.as_sym().map(str::to_string),
            None => None,
        };
        let binds = gp.get(2).and_then(|d| d.as_list()).ok_or("group missing bindings")?;
        for b in binds {
            let bp = b.as_list().ok_or("binding is not a list")?;
            let key_name = bp.first().and_then(|d| d.text()).ok_or("binding missing key")?;
            let key = codec::parse_key(key_name).ok_or_else(|| format!("unknown key: {key_name}"))?;
            let intent = bp
                .get(1)
                .and_then(codec::intent_from_datum)
                .ok_or_else(|| format!("binding for '{key_name}' has an unknown intent"))?;
            let when = match bp.get(2) {
                Some(d) => codec::cond_from_datum(d),
                None => Cond::Always,
            };
            out.push(Binding { mode: mode.clone(), panel: panel.clone(), key, intent, when, layer });
        }
    }
    Ok(out)
}

// ── Theme config parsers ───────────────────────────────────────────────────────

/// Parse a `(quote ((theme "name" (role fg bg bold italic underline) …) …))`
/// datum (as produced by `theme_presets.scm`) into a list of `NamedTheme` presets.
///
/// Unknown role tokens and malformed role forms are silently skipped.
pub fn parse_theme_config(datum: &Datum) -> Vec<crate::extension::theme::NamedTheme> {
    let forms = match datum.as_list() {
        Some(l) => l,
        None => return Vec::new(),
    };
    let mut out = Vec::new();
    for form in forms {
        let parts = match form.as_list() {
            Some(p) => p,
            None => continue,
        };
        let head = parts.first().and_then(|d| d.as_sym());
        if head != Some("theme") {
            continue;
        }
        let name = match parts.get(1).and_then(|d| d.as_str()) {
            Some(s) => s.to_string(),
            None => continue,
        };
        let overrides = parse_role_style_overrides(&parts[2..]);
        out.push(crate::extension::theme::NamedTheme { name, preset: true, overrides });
    }
    out
}

/// Parse a single `(define-theme "name" (role fg bg bold italic underline) …)`
/// datum into a custom `NamedTheme`. Returns `None` if the form is malformed.
pub fn parse_define_theme(datum: &Datum) -> Option<crate::extension::theme::NamedTheme> {
    let parts = datum.as_list()?;
    let head = parts.first().and_then(|d| d.as_sym());
    if head != Some("define-theme") {
        return None;
    }
    let name = parts.get(1).and_then(|d| d.as_str())?.to_string();
    let overrides = parse_role_style_overrides(&parts[2..]);
    Some(crate::extension::theme::NamedTheme { name, preset: false, overrides })
}

/// Parse the active theme selection from a `(set-theme "name")` datum.
/// Returns the name string if valid, else `None`.
pub fn parse_set_theme(datum: &Datum) -> Option<String> {
    let parts = datum.as_list()?;
    if parts.first().and_then(|d| d.as_sym()) != Some("set-theme") {
        return None;
    }
    parts.get(1).and_then(|d| d.as_str()).map(String::from)
}

/// A color slot in a role-style form: the bare symbol `nil` means "no color"
/// (`Color::Reset`), anything else is read the same way any other themed
/// color is (`#rrggbb`, an ANSI name, or `default`/`reset`).
fn parse_theme_color(d: &Datum) -> Option<ratatui::style::Color> {
    let text = d.text()?;
    if text == "nil" { Some(ratatui::style::Color::Reset) } else { crate::core::parse_color(text) }
}

/// Parse each `(role fg bg bold italic underline)` form into a role's full
/// `RoleStyle` — one override is the whole tuple, never a partial attribute,
/// so there is exactly one shape to parse (no merged/standalone distinction
/// like the old per-attribute override lists needed). A form that doesn't
/// supply all six positions, or whose role/color/boolean fails to parse, is
/// silently skipped: this runs after the TUI has taken the alternate screen,
/// so an eprintln would corrupt the display. (disable, continue.)
fn parse_role_style_overrides(pairs: &[Datum]) -> Vec<(crate::core::Role, crate::extension::theme::RoleStyle)> {
    let mut out = Vec::new();
    for pair in pairs {
        let Some(pl) = pair.as_list() else { continue };
        let Some(role) = pl.first().and_then(|d| d.as_sym()).and_then(crate::core::Role::from_token) else { continue };
        let Some(fg) = pl.get(1).and_then(parse_theme_color) else { continue };
        let Some(bg) = pl.get(2).and_then(parse_theme_color) else { continue };
        let Some(bold) = pl.get(3).and_then(|d| d.as_bool()) else { continue };
        let Some(italic) = pl.get(4).and_then(|d| d.as_bool()) else { continue };
        let Some(underline) = pl.get(5).and_then(|d| d.as_bool()) else { continue };
        out.push((role, crate::extension::theme::RoleStyle { fg, bg, bold, italic, underline }));
    }
    out
}

// ── LsColors palette config parsers ─────────────────────────────────────────────

/// Parse a `(quote ((palette "name" (raw "...")) …))` datum (as produced by
/// `lscolors/presets.scm`) into a list of `NamedPalette` presets.
///
/// Malformed forms and missing `raw` fields are silently skipped.
pub fn parse_lscolors_presets_config(datum: &Datum) -> Vec<crate::extension::lscolors::NamedPalette> {
    let forms = match datum.as_list() {
        Some(l) => l,
        None => return Vec::new(),
    };
    let mut out = Vec::new();
    for form in forms {
        let parts = match form.as_list() {
            Some(p) => p,
            None => continue,
        };
        if parts.first().and_then(|d| d.as_sym()) != Some("palette") {
            continue;
        }
        let name = match parts.get(1).and_then(|d| d.as_str()) {
            Some(s) => s.to_string(),
            None => continue,
        };
        let raw = match parse_raw_field(&parts[2..]) {
            Some(s) => s,
            None => continue,
        };
        out.push(crate::extension::lscolors::NamedPalette { name, preset: true, raw });
    }
    out
}

/// Parse a single `(define-lscolors-palette "name" (raw "..."))` datum into a
/// custom `NamedPalette`. Returns `None` if the form is malformed.
pub fn parse_define_lscolors_palette(datum: &Datum) -> Option<crate::extension::lscolors::NamedPalette> {
    let parts = datum.as_list()?;
    if parts.first().and_then(|d| d.as_sym()) != Some("define-lscolors-palette") {
        return None;
    }
    let name = parts.get(1).and_then(|d| d.as_str())?.to_string();
    let raw = parse_raw_field(&parts[2..])?;
    Some(crate::extension::lscolors::NamedPalette { name, preset: false, raw })
}

/// Parse the active palette selection from a `(set-lscolors-palette "name")` datum.
/// Returns the name string if valid, else `None`.
pub fn parse_set_lscolors_palette(datum: &Datum) -> Option<String> {
    let parts = datum.as_list()?;
    if parts.first().and_then(|d| d.as_sym()) != Some("set-lscolors-palette") {
        return None;
    }
    parts.get(1).and_then(|d| d.as_str()).map(String::from)
}

/// Find and read the `(raw "...")` field among a palette form's trailing parts.
fn parse_raw_field(fields: &[Datum]) -> Option<String> {
    for f in fields {
        let Some(fl) = f.as_list() else { continue };
        if fl.first().and_then(|d| d.as_sym()) == Some("raw") {
            return fl.get(1).and_then(|d| d.as_str()).map(String::from);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::sexpr::parse;

    fn eval_quote(src: &str) -> Datum {
        // Parse `(quote (...))` and extract the inner list (the second element).
        let outer = parse(src).expect("parse failed");
        let list = outer.as_list().expect("outer is not a list");
        list.get(1).cloned().expect("quote has no inner datum")
    }

    #[test]
    fn parse_bool_option() {
        let d = eval_quote(r##"(quote ((option "x" "Cat" bool "#f" "x-label")))"##);
        let defs = parse_option_config(&d).unwrap();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].key, "x");
        assert_eq!(defs[0].category, "Cat");
        assert_eq!(defs[0].label, "x-label");
        assert_eq!(defs[0].kind, OptionKind::Bool);
        assert_eq!(defs[0].default, "#f");
    }

    #[test]
    fn parse_int_option() {
        let d = eval_quote(r#"(quote ((option "n" "C" (int 1 99) "5" "n-label")))"#);
        let defs = parse_option_config(&d).unwrap();
        assert_eq!(defs[0].kind, OptionKind::Int { min: 1, max: 99 });
        assert_eq!(defs[0].default, "5");
    }

    #[test]
    fn parse_choice_option() {
        let d = eval_quote(
            r#"(quote ((option "c" "C" (choice "a" "b" "c") "a" "c-label")))"#,
        );
        let defs = parse_option_config(&d).unwrap();
        assert_eq!(
            defs[0].kind,
            OptionKind::Choice(vec!["a".into(), "b".into(), "c".into()])
        );
    }

    #[test]
    fn parse_list_and_text_options() {
        let d = eval_quote(
            r#"(quote (
              (option "l" "C" list "" "l-label")
              (option "t" "C" text "" "t-label")))"#,
        );
        let defs = parse_option_config(&d).unwrap();
        assert_eq!(defs[0].kind, OptionKind::List);
        assert_eq!(defs[1].kind, OptionKind::Text);
    }

    #[test]
    fn parse_multiple_options() {
        let d = eval_quote(
            r##"(quote (
              (option "a" "Display" bool "#t" "a-label")
              (option "b" "Matcher" (int 0 100) "16" "b-label")))"##,
        );
        let defs = parse_option_config(&d).unwrap();
        assert_eq!(defs.len(), 2);
        assert_eq!(defs[0].category, "Display");
        assert_eq!(defs[1].category, "Matcher");
    }

    #[test]
    fn parse_option_missing_key_is_error() {
        let d = eval_quote(r#"(quote ((option)))"#);
        assert!(parse_option_config(&d).is_err());
    }

    #[test]
    fn parse_option_unknown_kind_is_error() {
        let d = eval_quote(r#"(quote ((option "k" "C" weird "x" "l")))"#);
        assert!(parse_option_config(&d).is_err());
    }

    #[test]
    fn parse_option_empty_choice_is_error() {
        let d = eval_quote(r#"(quote ((option "k" "C" (choice) "x" "l")))"#);
        assert!(parse_option_config(&d).is_err());
    }

    #[test]
    fn parse_theme_config_reads_name_and_overrides() {
        use crate::core::Role;
        let d = eval_quote(r##"(quote ((theme "x" (border "#102030" nil #f #f #f))))"##);
        let themes = parse_theme_config(&d);
        assert_eq!(themes.len(), 1);
        assert_eq!(themes[0].name, "x");
        assert!(themes[0].preset);
        assert_eq!(themes[0].overrides.len(), 1);
        let (role, style) = themes[0].overrides[0];
        assert_eq!(role, Role::Border);
        assert_eq!(style.fg, ratatui::style::Color::Rgb(0x10, 0x20, 0x30));
        assert_eq!(style.bg, ratatui::style::Color::Reset);
        assert!(!style.bold && !style.italic && !style.underline);
    }

    #[test]
    fn parse_define_theme_reads_a_custom() {
        use crate::core::Role;
        let d = crate::script::sexpr::parse(r#"(define-theme "y" (cursor-bg "blue" nil #f #f #f))"#)
            .expect("parse failed");
        let nt = parse_define_theme(&d).expect("should parse");
        assert_eq!(nt.name, "y");
        assert!(!nt.preset);
        assert_eq!(nt.overrides[0].0, Role::CursorBg);
        assert_eq!(nt.overrides[0].1.fg, ratatui::style::Color::Blue);
    }

    #[test]
    fn parse_theme_skips_unknown_role() {
        let d = eval_quote(
            r##"(quote ((theme "t" (nonexistent-role "#ff0000" nil #f #f #f) (border "red" nil #f #f #f))))"##,
        );
        let themes = parse_theme_config(&d);
        assert_eq!(themes[0].overrides.len(), 1, "unknown role is dropped, known one kept");
        assert_eq!(themes[0].overrides[0].0, crate::core::Role::Border);
    }

    #[test]
    fn parse_define_theme_reads_fg_bg_and_every_modifier() {
        use crate::core::Role;
        let d = crate::script::sexpr::parse(
            r##"(define-theme "y" (entry-dir "#83a598" "#000000" #t #f #t))"##,
        )
        .expect("parse failed");
        let nt = parse_define_theme(&d).expect("should parse");
        let (role, style) = nt.overrides[0];
        assert_eq!(role, Role::EntryDir);
        assert_eq!(style.fg, ratatui::style::Color::Rgb(0x83, 0xa5, 0x98));
        assert_eq!(style.bg, ratatui::style::Color::Rgb(0, 0, 0));
        assert!(style.bold);
        assert!(!style.italic);
        assert!(style.underline);
    }

    #[test]
    fn parse_define_theme_reads_nil_as_no_color_for_fg_or_bg() {
        use crate::core::Role;
        let d = crate::script::sexpr::parse(r#"(define-theme "y" (title nil nil #f #t #f))"#)
            .expect("parse failed");
        let nt = parse_define_theme(&d).expect("should parse");
        let (role, style) = nt.overrides[0];
        assert_eq!(role, Role::Title);
        assert_eq!(style.fg, ratatui::style::Color::Reset);
        assert_eq!(style.bg, ratatui::style::Color::Reset);
        assert!(style.italic);
    }

    #[test]
    fn parse_define_theme_skips_a_form_with_a_missing_position() {
        // Every override is the full 6-token form; a role form that stops
        // short (e.g. no underline) has nothing valid to fall back to.
        let d = crate::script::sexpr::parse(r##"(define-theme "y" (border "#102030" nil #f #f))"##)
            .expect("parse failed");
        let nt = parse_define_theme(&d).expect("should parse");
        assert!(nt.overrides.is_empty(), "an incomplete role form is dropped, not partially applied");
    }

    #[test]
    fn parse_lscolors_presets_config_reads_name_and_raw() {
        let d = eval_quote(r##"(quote ((palette "x" (raw "di=01;34"))))"##);
        let palettes = parse_lscolors_presets_config(&d);
        assert_eq!(palettes.len(), 1);
        assert_eq!(palettes[0].name, "x");
        assert!(palettes[0].preset);
        assert_eq!(palettes[0].raw, "di=01;34");
    }

    #[test]
    fn parse_lscolors_presets_config_skips_malformed_forms() {
        let d = eval_quote(r##"(quote ((palette "no-raw") (palette "y" (raw "ln=01;36"))))"##);
        let palettes = parse_lscolors_presets_config(&d);
        assert_eq!(palettes.len(), 1, "the malformed 'no-raw' entry is dropped");
        assert_eq!(palettes[0].name, "y");
    }

    #[test]
    fn parse_define_lscolors_palette_reads_a_custom() {
        let d = crate::script::sexpr::parse(r#"(define-lscolors-palette "z" (raw "ex=01;32"))"#)
            .expect("parse failed");
        let np = parse_define_lscolors_palette(&d).expect("should parse");
        assert_eq!(np.name, "z");
        assert!(!np.preset);
        assert_eq!(np.raw, "ex=01;32");
    }

    #[test]
    fn parse_set_lscolors_palette_reads_the_name() {
        let d = crate::script::sexpr::parse(r#"(set-lscolors-palette "dark")"#).expect("parse failed");
        assert_eq!(parse_set_lscolors_palette(&d), Some("dark".to_string()));
    }
}
