//! The change-set applier: turns the settings store's change sets into the
//! kernel's native projections (doc/SCHEME.md, Projections). It is the only
//! writer of those projections: the OptionStore's values, the keymap's User
//! layer, and the user's palette commands.

use crate::core::{Intent, OptionStore};
use crate::script::codec;
use crate::script::condition::Cond;
use crate::script::config::option_value_from_datum;
use crate::script::keymap::{Binding, Layer};
use crate::script::sexpr::Datum;
use crate::script::store::{Change, Section};
use crate::services::command::CommandDef;

/// Apply the option entries of a change set to the OptionStore. An entry whose
/// value does not decode to the option's kind, or whose effective value is
/// gone (only the `default` layer can hold it, and it is never removed), is
/// reported. Returns the problems, one line each.
pub fn apply_options(changes: &[Change], options: &OptionStore) -> Vec<String> {
    let mut problems = Vec::new();
    for c in changes.iter().filter(|c| c.section == Section::Option) {
        let Some(key) = c.id.text() else { continue };
        // An undeclared key only reaches here from custom.scm on the fallback
        // path; the store keeps such entries without effect, and so does this.
        let Some(def) = options.def(key) else { continue };
        let applied = c
            .value
            .as_ref()
            .and_then(|v| option_value_from_datum(v, &def.kind))
            .is_some_and(|v| options.set(key, v));
        if !applied {
            problems.push(format!("settings: option {key}: value does not fit its kind"));
        }
    }
    problems
}

/// The user's layers of bindings and commands, as the store holds them (the
/// entries above the native defaults), kept in first-set order so the keymap
/// and palette stay stable as entries change.
#[derive(Debug, Default)]
pub struct UserLayer {
    bindings: Vec<(Datum, Datum)>,
    commands: Vec<(Datum, Datum)>,
    orders: Vec<(Datum, Datum)>,
    layouts: Vec<(Datum, Datum)>,
}

impl UserLayer {
    /// Fold a change set in: set, replace, or drop (no effective value) each
    /// binding and command entry.
    pub fn absorb(&mut self, changes: &[Change]) {
        for c in changes {
            let entries = match c.section {
                Section::Binding => &mut self.bindings,
                Section::Command => &mut self.commands,
                Section::Order => &mut self.orders,
                Section::Layout => &mut self.layouts,
                _ => continue,
            };
            match (&c.value, entries.iter().position(|(id, _)| *id == c.id)) {
                (Some(v), Some(i)) => entries[i].1 = v.clone(),
                (Some(v), None) => entries.push((c.id.clone(), v.clone())),
                (None, Some(i)) => {
                    entries.remove(i);
                }
                (None, None) => {}
            }
        }
    }

    /// The User keymap layer. An `unbind!` tombstone becomes a `Noop` binding
    /// with no label: it wins over the layers below and does nothing.
    /// Entries that do not decode are reported and skipped.
    pub fn bindings(&self) -> (Vec<Binding>, Vec<String>) {
        let mut out = Vec::new();
        let mut problems = Vec::new();
        for (id, value) in &self.bindings {
            match binding(id, value) {
                Some(b) => out.push(b),
                None => problems.push(format!(
                    "settings: binding {} does not decode",
                    crate::script::sexpr::render(id)
                )),
            }
        }
        (out, problems)
    }

    /// A named display order's items (empty when the user set none).
    pub fn order(&self, name: &str) -> Vec<String> {
        self.orders
            .iter()
            .find(|(id, _)| id.text() == Some(name))
            .and_then(|(_, v)| v.as_list())
            .map(|items| items.iter().filter_map(|i| i.text().map(str::to_string)).collect())
            .unwrap_or_default()
    }

    /// The user's layouts. The store checked each at its write; one that still
    /// does not read (custom.scm on the fallback path) is reported and skipped.
    pub fn layouts(&self) -> (Vec<(String, crate::script::layout::Node)>, Vec<String>) {
        let mut out = Vec::new();
        let mut problems = Vec::new();
        for (id, tree) in &self.layouts {
            let Some(id) = id.text() else { continue };
            match crate::script::layout::parse(id, tree) {
                Ok(node) => out.push((id.to_string(), node)),
                Err(e) => problems.push(format!("settings: layout {id}: {e}")),
            }
        }
        (out, problems)
    }

    /// The user's palette commands.
    pub fn commands(&self) -> (Vec<CommandDef>, Vec<String>) {
        let mut out = Vec::new();
        let mut problems = Vec::new();
        for (id, value) in &self.commands {
            let decoded = id.text().zip(value.as_list()).and_then(|(name, v)| match v {
                [Datum::Str(description), intent] => Some(CommandDef {
                    name: name.to_string(),
                    description: description.clone(),
                    intent: codec::intent_from_datum(intent)?,
                }),
                _ => None,
            });
            match decoded {
                Some(c) => out.push(c),
                None => problems.push(format!("settings: command {} does not decode", id.text().unwrap_or("?"))),
            }
        }
        (out, problems)
    }
}

fn binding(id: &Datum, value: &Datum) -> Option<Binding> {
    let [mode, panel, key] = id.as_list()? else { return None };
    let panel = match panel {
        Datum::Bool(false) => None,
        p => Some(p.text()?.to_string()),
    };
    let key = codec::parse_key(key.text()?)?;
    let (intent, when, help) = match value {
        Datum::Sym(s) if s == "unbound" => (Intent::Noop, Cond::Always, None),
        Datum::List(v) => {
            let [intent, when, label] = v.as_slice() else { return None };
            let when = match when {
                Datum::Bool(false) => Cond::Always,
                w => codec::cond_from_datum(w),
            };
            (codec::intent_from_datum(intent)?, when, label.as_str().map(str::to_string))
        }
        _ => return None,
    };
    Some(Binding { mode: mode.text()?.to_string(), panel, key, intent, when, layer: Layer::User, help, owner: None })
}
