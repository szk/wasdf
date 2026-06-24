//! Decoding parsed config data (s-expression datums) into kernel registry
//! types: resolver entries, palette commands, and keymap bindings. Shared by
//! the embedded-default loader (kernel boot) and the optional-extension loader.

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
