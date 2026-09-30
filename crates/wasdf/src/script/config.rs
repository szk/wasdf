//! Decoding parsed config data (s-expression datums) into kernel registry
//! types: resolver entries, palette commands, and keymap bindings. Shared by
//! the embedded-default loader (kernel boot) and the optional-extension loader.

use crate::core::{OptionDef, OptionKind, OptionValue};
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
        let default_datum = parts.get(4).ok_or("option missing default")?;
        let default = option_value_from_datum(default_datum, &kind)
            .ok_or_else(|| format!("option {key}: default does not fit its kind"))?;
        let label =
            parts.get(5).and_then(|d| d.as_str()).ok_or("option missing label")?.to_string();
        // Parsed defs are unowned; the kernel tags an extension's defs with its id.
        out.push(OptionDef { key, category, label, kind, default, owner: None });
    }
    Ok(out)
}

/// Read an option value literal of `kind`: `#t`, `42`, `"text"`, `("a" "b")`,
/// a record `((field value) …)`, a map `(("key" value) …)`. For a scalar or
/// list kind a string in the flat form (`"#t"`, `"4"`, `"a,b"`) is accepted
/// too, so older manifests keep loading. `None` when it does not fit `kind`.
pub fn option_value_from_datum(datum: &Datum, kind: &OptionKind) -> Option<OptionValue> {
    let value = match (kind, datum) {
        (OptionKind::Text | OptionKind::Choice(_), Datum::Str(s)) => OptionValue::Text(s.clone()),
        (_, Datum::Str(s)) => return OptionValue::parse_flat(kind, s),
        (OptionKind::Bool, Datum::Bool(b)) => OptionValue::Bool(*b),
        (OptionKind::Int { .. }, Datum::Int(n)) => OptionValue::Int(*n),
        (OptionKind::List(item), Datum::List(items)) => OptionValue::List(
            items.iter().map(|d| option_value_from_datum(d, item)).collect::<Option<Vec<_>>>()?,
        ),
        (OptionKind::Record(fields), Datum::List(pairs)) => {
            let mut m = std::collections::BTreeMap::new();
            for pair in pairs {
                let p = pair.as_list()?;
                let name = p.first()?.text()?;
                let (_, field_kind) = fields.iter().find(|(f, _)| f == name)?;
                m.insert(name.to_string(), option_value_from_datum(p.get(1)?, field_kind)?);
            }
            OptionValue::Map(m)
        }
        (OptionKind::Map(item), Datum::List(pairs)) => {
            let mut m = std::collections::BTreeMap::new();
            for pair in pairs {
                let p = pair.as_list()?;
                m.insert(p.first()?.text()?.to_string(), option_value_from_datum(p.get(1)?, item)?);
            }
            OptionValue::Map(m)
        }
        _ => return None,
    };
    kind.accepts(&value).then_some(value)
}

fn parse_option_kind(datum: &Datum) -> Result<OptionKind, String> {
    match datum {
        Datum::Sym(s) => match s.as_str() {
            "bool" => Ok(OptionKind::Bool),
            "text" => Ok(OptionKind::Text),
            "list" => Ok(OptionKind::text_list()),
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
                "list" => {
                    let item = items.get(1).ok_or("list kind missing its item kind")?;
                    Ok(OptionKind::List(Box::new(parse_option_kind(item)?)))
                }
                "map" => {
                    let item = items.get(1).ok_or("map kind missing its value kind")?;
                    Ok(OptionKind::Map(Box::new(parse_option_kind(item)?)))
                }
                "record" => {
                    let mut fields = Vec::new();
                    for f in &items[1..] {
                        let f = f.as_list().ok_or("record field is not a list")?;
                        let name = f.first().and_then(|d| d.text()).ok_or("record field missing name")?;
                        let kind = parse_option_kind(f.get(1).ok_or("record field missing kind")?)?;
                        fields.push((name.to_string(), kind));
                    }
                    if fields.is_empty() {
                        return Err("record kind has no fields".into());
                    }
                    Ok(OptionKind::Record(fields))
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
            // Everything after the intent is `when` and/or the help label, in
            // either order: a `when` is always a symbol or a list, a label is
            // always a string, so they are told apart by type rather than by
            // position. Every binding written before labels existed therefore
            // still parses unchanged.
            let rest = bp.get(2..).unwrap_or_default();
            let help = rest.iter().find_map(|d| d.as_str()).map(str::to_string);
            let when = rest
                .iter()
                .find(|d| d.as_str().is_none())
                .map(codec::cond_from_datum)
                .unwrap_or(Cond::Always);
            out.push(Binding {
                mode: mode.clone(),
                panel: panel.clone(),
                key,
                intent,
                when,
                layer,
                help,
                owner: None,
            });
        }
    }
    Ok(out)
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
        assert_eq!(defs[0].default, OptionValue::Bool(false));
    }

    #[test]
    fn parse_int_option() {
        let d = eval_quote(r#"(quote ((option "n" "C" (int 1 99) "5" "n-label")))"#);
        let defs = parse_option_config(&d).unwrap();
        assert_eq!(defs[0].kind, OptionKind::Int { min: 1, max: 99 });
        assert_eq!(defs[0].default, OptionValue::Int(5));
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
        assert_eq!(defs[0].kind, OptionKind::text_list());
        assert_eq!(defs[1].kind, OptionKind::Text);
    }

    #[test]
    fn typed_defaults_and_structured_kinds_parse() {
        let d = eval_quote(
            r##"(quote (
              (option "b" "C" bool #t "b")
              (option "n" "C" (int 0 9) 3 "n")
              (option "l" "C" list ("rs" "md") "l")
              (option "s" "C" (record (fg text) (bold bool)) ((fg "#fff") (bold #f)) "s")
              (option "m" "C" (map (list text)) (("dark" ("a")) ("light" ())) "m")))"##,
        );
        let defs = parse_option_config(&d).unwrap();
        assert_eq!(defs[0].default, OptionValue::Bool(true));
        assert_eq!(defs[1].default, OptionValue::Int(3));
        assert_eq!(defs[2].default.flat(), "rs,md");
        assert!(matches!(&defs[3].kind, OptionKind::Record(f) if f.len() == 2));
        assert!(defs[3].kind.accepts(&defs[3].default));
        assert!(matches!(&defs[4].default, OptionValue::Map(m) if m.len() == 2));
    }

    #[test]
    fn legacy_string_defaults_still_load_and_bad_defaults_are_rejected() {
        let d = eval_quote(r##"(quote ((option "b" "C" bool "#t" "b") (option "l" "C" list "a,b" "l")))"##);
        let defs = parse_option_config(&d).unwrap();
        assert_eq!(defs[0].default, OptionValue::Bool(true));
        assert_eq!(defs[1].default.flat(), "a,b");
        let bad = eval_quote(r#"(quote ((option "n" "C" (int 0 9) 42 "n")))"#);
        assert!(parse_option_config(&bad).is_err(), "an out-of-range default is an error");
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

}
