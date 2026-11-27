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

/// `datum` as a list, or `what` as the error.
fn list<'a>(datum: &'a Datum, what: &str) -> Result<&'a [Datum], String> {
    datum.as_list().ok_or_else(|| format!("{what} is not a list"))
}

/// The `i`th element of `parts` read by `get`, or `missing` as the error.
fn field<'a, T>(parts: &'a [Datum], i: usize, get: impl FnOnce(&'a Datum) -> Option<T>, missing: &str) -> Result<T, String> {
    parts.get(i).and_then(get).ok_or_else(|| missing.to_string())
}

/// Map a resolver-config datum (a list of `(key destructive (candidate…))`)
/// into resolver entries.
pub fn parse_resolver_config(datum: &Datum) -> Result<Vec<ResolverEntry>, String> {
    list(datum, "resolver config")?.iter().map(parse_resolver_entry).collect()
}

fn parse_resolver_entry(entry: &Datum) -> Result<ResolverEntry, String> {
    let parts = list(entry, "resolver entry")?;
    let key = field(parts, 0, Datum::as_sym, "resolver entry missing key")?.to_string();
    let destructive = parts.get(1).and_then(Datum::as_bool).unwrap_or(false);
    let cand_data = field(parts, 2, Datum::as_list, "missing candidates")?;
    // Optional 4th element: selectable options, a list of (token label) pairs.
    let options = parts
        .get(3)
        .and_then(Datum::as_list)
        .unwrap_or_default()
        .iter()
        .filter_map(|o| {
            let ol = o.as_list()?;
            Some(OptDef {
                token: ol.first()?.as_str()?.to_string(),
                label: ol.get(1).and_then(Datum::as_str).unwrap_or("").to_string(),
            })
        })
        .collect();
    let candidates = cand_data.iter().map(parse_candidate).collect::<Result<_, _>>()?;
    Ok(ResolverEntry { key, destructive, options, candidates })
}

fn parse_candidate(c: &Datum) -> Result<Candidate, String> {
    let cl = list(c, "candidate")?;
    let kind = match cl.first().and_then(Datum::as_sym) {
        Some("native") => Kind::Native,
        Some("uutils") => Kind::Uutils,
        Some("native-macos") => Kind::NativeMacos,
        Some("native-linux") => Kind::NativeLinux,
        other => return Err(format!("unknown candidate kind: {other:?}")),
    };
    let argv = cl[1..]
        .iter()
        .map(|a| match a {
            Datum::Str(s) => Ok(ArgEl::Lit(s.clone())),
            Datum::Sym(s) => Ok(match s.as_str() {
                "src" => ArgEl::Ph(Slot::Src),
                "dst" => ArgEl::Ph(Slot::Dst),
                "path" => ArgEl::Ph(Slot::Path),
                "paths" => ArgEl::Ph(Slot::Paths),
                "opts" => ArgEl::Ph(Slot::Opts),
                _ => ArgEl::Lit(s.clone()),
            }),
            _ => Err("invalid argv element".to_string()),
        })
        .collect::<Result<_, _>>()?;
    Ok(Candidate { kind, argv })
}

/// Map a command-config datum (a list of `(name description intent)`) into
/// palette commands.
pub fn parse_command_config(datum: &Datum) -> Result<Vec<CommandDef>, String> {
    list(datum, "command config")?
        .iter()
        .map(|entry| {
            let parts = list(entry, "command entry")?;
            let name = field(parts, 0, Datum::as_sym, "command missing name")?;
            let description = field(parts, 1, Datum::as_str, "command missing description")?;
            let intent = field(parts, 2, codec::intent_from_datum, &format!("command '{name}' has an unknown intent"))?;
            Ok(CommandDef { name: name.to_string(), description: description.to_string(), intent })
        })
        .collect()
}

/// Map an option-config datum (the inner list of a `(quote (…))` form) into
/// `OptionDef` records. Each element must be an `(option key category kind
/// default label)` form (doc/SCHEME.md).
pub fn parse_option_config(datum: &Datum) -> Result<Vec<OptionDef>, String> {
    list(datum, "option config")?
        .iter()
        .map(|form| {
            let parts = list(form, "option form")?;
            let head = field(parts, 0, Datum::as_sym, "option form missing head symbol")?;
            if head != "option" {
                return Err(format!("expected 'option' form, got '{head}'"));
            }
            let key = field(parts, 1, Datum::as_str, "option missing key")?.to_string();
            let category = field(parts, 2, Datum::as_str, "option missing category")?.to_string();
            let kind = parse_option_kind(field(parts, 3, Some, "option missing kind")?)?;
            let default = option_value_from_datum(field(parts, 4, Some, "option missing default")?, &kind)
                .ok_or_else(|| format!("option {key}: default does not fit its kind"))?;
            let label = field(parts, 5, Datum::as_str, "option missing label")?.to_string();
            // Parsed defs are unowned; the kernel tags an extension's defs with its id.
            Ok(OptionDef { key, category, label, kind, default, owner: None })
        })
        .collect()
}

/// Read an option value literal of `kind`: `#t`, `42`, `"text"`, `("a" "b")`,
/// a record `((field value) …)`, a map `(("key" value) …)`. For a scalar or
/// list kind a string in the flat form (`"#t"`, `"4"`, `"a,b"`) is accepted
/// too. `None` when it does not fit `kind`.
pub fn option_value_from_datum(datum: &Datum, kind: &OptionKind) -> Option<OptionValue> {
    let value = match (kind, datum) {
        (OptionKind::Text | OptionKind::Choice(_), Datum::Str(s)) => OptionValue::Text(s.clone()),
        (_, Datum::Str(s)) => return OptionValue::parse_flat(kind, s),
        (OptionKind::Bool, Datum::Bool(b)) => OptionValue::Bool(*b),
        (OptionKind::Int { .. }, Datum::Int(n)) => OptionValue::Int(*n),
        (OptionKind::List(item), Datum::List(items)) => OptionValue::List(
            items.iter().map(|d| option_value_from_datum(d, item)).collect::<Option<Vec<_>>>()?,
        ),
        (OptionKind::Record(fields), Datum::List(items)) => {
            pairs(items, |name| fields.iter().find(|(f, _)| f == name).map(|(_, k)| k))?
        }
        (OptionKind::Map(item), Datum::List(items)) => pairs(items, |_| Some(item))?,
        _ => return None,
    };
    kind.accepts(&value).then_some(value)
}

/// A record's or a map's `((name value) …)`; only how a name finds its
/// value's kind differs between the two.
fn pairs<'k>(items: &[Datum], kind_of: impl Fn(&str) -> Option<&'k OptionKind>) -> Option<OptionValue> {
    items
        .iter()
        .map(|pair| {
            let p = pair.as_list()?;
            let name = p.first()?.text()?;
            Some((name.to_string(), option_value_from_datum(p.get(1)?, kind_of(name)?)?))
        })
        .collect::<Option<_>>()
        .map(OptionValue::Map)
}

fn parse_option_kind(datum: &Datum) -> Result<OptionKind, String> {
    let items = match datum {
        Datum::Sym(s) => {
            return match s.as_str() {
                "bool" => Ok(OptionKind::Bool),
                "text" => Ok(OptionKind::Text),
                "list" => Ok(OptionKind::text_list()),
                other => Err(format!("unknown option kind symbol: {other}")),
            };
        }
        Datum::List(items) => items,
        _ => return Err("option kind must be a symbol or list".into()),
    };
    let int = |i, what| field(items, i, |d| if let Datum::Int(n) = d { Some(*n) } else { None }, what);
    let inner = |what| field(items, 1, Some, what).and_then(parse_option_kind).map(Box::new);
    match field(items, 0, Datum::as_sym, "kind list missing head symbol")? {
        "int" => Ok(OptionKind::Int {
            min: int(1, "int kind missing or non-integer min")?,
            max: int(2, "int kind missing or non-integer max")?,
        }),
        "list" => Ok(OptionKind::List(inner("list kind missing its item kind")?)),
        "map" => Ok(OptionKind::Map(inner("map kind missing its value kind")?)),
        "record" => {
            let fields = items[1..]
                .iter()
                .map(|f| {
                    let f = list(f, "record field")?;
                    let name = field(f, 0, Datum::text, "record field missing name")?;
                    Ok((name.to_string(), parse_option_kind(field(f, 1, Some, "record field missing kind")?)?))
                })
                .collect::<Result<Vec<_>, String>>()?;
            if fields.is_empty() {
                return Err("record kind has no fields".into());
            }
            Ok(OptionKind::Record(fields))
        }
        "choice" => {
            let choices: Vec<String> = items[1..].iter().filter_map(|d| d.as_str().map(String::from)).collect();
            if choices.is_empty() {
                return Err("choice kind has no options".into());
            }
            Ok(OptionKind::Choice(choices))
        }
        other => Err(format!("unknown kind form head: {other}")),
    }
}

/// Map a keymap-config datum (a list of groups) into bindings at `layer`.
pub fn parse_keymap_config(datum: &Datum, layer: Layer) -> Result<Vec<Binding>, String> {
    let mut out = Vec::new();
    for group in list(datum, "keymap config")? {
        let gp = list(group, "keymap group")?;
        let mode = field(gp, 0, Datum::as_sym, "group missing mode")?;
        // `#f` (or anything not a symbol) means "every panel".
        let panel = gp.get(1).and_then(Datum::as_sym);
        for b in field(gp, 2, Datum::as_list, "group missing bindings")? {
            let bp = list(b, "binding")?;
            let key_name = field(bp, 0, Datum::text, "binding missing key")?;
            let key = codec::parse_key(key_name).ok_or_else(|| format!("unknown key: {key_name}"))?;
            let intent = field(bp, 1, codec::intent_from_datum, &format!("binding for '{key_name}' has an unknown intent"))?;
            // Everything after the intent is `when` and/or the help label, in
            // either order: a `when` is always a symbol or a list, a label is
            // always a string, so they are told apart by type rather than by
            // position.
            let rest = bp.get(2..).unwrap_or_default();
            let help = rest.iter().find_map(Datum::as_str).map(str::to_string);
            let when = rest.iter().find(|d| d.as_str().is_none()).map(codec::cond_from_datum).unwrap_or(Cond::Always);
            out.push(Binding {
                mode: mode.to_string(),
                panel: panel.map(str::to_string),
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
    fn string_defaults_load_and_bad_defaults_are_rejected() {
        let d = eval_quote(r##"(quote ((option "b" "C" bool "#t" "b") (option "l" "C" list "a,b" "l")))"##);
        let defs = parse_option_config(&d).unwrap();
        assert_eq!(defs[0].default, OptionValue::Bool(true));
        assert_eq!(defs[1].default.flat(), "a,b");
        let bad = eval_quote(r#"(quote ((option "n" "C" (int 0 9) 42 "n")))"#);
        assert!(parse_option_config(&bad).is_err(), "an out-of-range default is an error");
    }

}
