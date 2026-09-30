//! The Rust side of the settings store (doc/SCHEME.md, The Settings Store).
//!
//! The store itself lives in the resident Scheme session (`store.scm`, loaded
//! when the session starts). This module is its client: it declares options,
//! evaluates store programs in a layer (`config.scm` form by form,
//! `custom.scm`, a `Configure` write, a REPL expression), collects each
//! evaluation's change set, and fetches the `custom` layer as the text of
//! `custom.scm` (which the store writes itself).
//! Applying change sets to the native projections is the caller's job.

use std::sync::{Arc, Mutex};

use crate::core::OptionDef;
use crate::script::sexpr::{self, Datum};
use crate::script::SchemeSession;

/// A store layer. The effective value of an entry is the one in the highest
/// non-empty layer: `Session` > `Custom` > `Config` > `Default`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layer {
    Default,
    Config,
    Custom,
    Session,
}

impl Layer {
    fn name(self) -> &'static str {
        match self {
            Layer::Default => "default",
            Layer::Config => "config",
            Layer::Custom => "custom",
            Layer::Session => "session",
        }
    }
}

/// A store section.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Section {
    Option,
    Binding,
    Command,
    Order,
    Layout,
}

impl Section {
    fn parse(s: &str) -> Option<Section> {
        Some(match s {
            "option" => Section::Option,
            "binding" => Section::Binding,
            "command" => Section::Command,
            "order" => Section::Order,
            "layout" => Section::Layout,
            _ => return None,
        })
    }
}

/// One entry of a change set (or of a layer dump).
///
/// `id` is the entry's key as the store holds it: a string for options,
/// commands, orders, and layouts; `(mode panel-or-#f "key")` for bindings.
/// `value` is the entry's effective value after the change — `None` when no
/// layer holds one any more. For a binding the value is `(intent when label)`
/// (`#f` for an absent `when` / label) or the symbol `unbound` (a tombstone).
#[derive(Debug, Clone, PartialEq)]
pub struct Change {
    pub section: Section,
    pub id: Datum,
    pub value: Option<Datum>,
}

/// The result of one evaluation: its printed value (or error) and the entries
/// whose effective value it changed.
#[derive(Debug)]
pub struct Outcome {
    pub value: Result<String, String>,
    pub changes: Vec<Change>,
}

/// A top-level form of a store program that failed; `line` is 1-based.
#[derive(Debug, Clone, PartialEq)]
pub struct FormError {
    pub line: usize,
    pub message: String,
}

/// The client. Store operations are serialized by `lock`, so the
/// set-layer / evaluate / take-changes sequence of one operation is never
/// interleaved with another's.
pub struct Store {
    session: Arc<SchemeSession>,
    lock: Mutex<()>,
}

impl Store {
    pub fn new(session: Arc<SchemeSession>) -> Self {
        Store { session, lock: Mutex::new(()) }
    }

    /// Declare options: their kinds and `default`-layer values. Declaring is
    /// not a change (the projection is built from the same defs).
    pub fn declare_options(&self, defs: &[OptionDef]) -> Result<(), String> {
        if defs.is_empty() {
            return Ok(());
        }
        let _guard = self.lock.lock().unwrap();
        let src: String = defs
            .iter()
            .map(|d| {
                format!(
                    "(store-declare-option! {:?} (quote {}) (quote {}))\n",
                    d.key,
                    d.kind.to_scheme(),
                    d.default.to_scheme()
                )
            })
            .collect();
        self.session.eval(&src).map(|_| ())
    }

    /// Evaluate `src` (one or more forms) with `layer` current.
    pub fn eval(&self, layer: Layer, src: &str) -> Outcome {
        let _guard = self.lock.lock().unwrap();
        self.eval_locked(layer, src)
    }

    fn eval_locked(&self, layer: Layer, src: &str) -> Outcome {
        let value = self
            .session
            .eval(&format!("(store-set-layer! {:?})", layer.name()))
            .and_then(|_| self.session.eval(src));
        let changes = self.take_changes_locked().unwrap_or_else(|e| {
            crate::services::messages::log(format!("settings store: change set unreadable ({e})"), true);
            Vec::new()
        });
        Outcome { value, changes }
    }

    /// Like [`eval`](Self::eval), with the change set left in its printed
    /// form — what `Payload::Scheme` carries across the worker boundary (core
    /// does not depend on the script layer); the kernel decodes it with
    /// [`parse_changes`].
    pub fn eval_printed(&self, layer: Layer, src: &str) -> (Result<String, String>, String) {
        let _guard = self.lock.lock().unwrap();
        let value = self
            .session
            .eval(&format!("(store-set-layer! {:?})", layer.name()))
            .and_then(|_| self.session.eval(src));
        let changes = self.session.eval("(store-take-changes)").unwrap_or_else(|_| "()".into());
        (value, changes)
    }

    /// Evaluate a store program one top-level form at a time, in order, with
    /// `layer` current. A failing form is reported and skipped; the rest still
    /// apply, and definitions made by earlier forms are visible to later ones.
    pub fn eval_program(&self, layer: Layer, text: &str) -> (Vec<Change>, Vec<FormError>) {
        let _guard = self.lock.lock().unwrap();
        let mut changes = Vec::new();
        let mut errors = Vec::new();
        for (line, form) in split_forms(text) {
            let out = self.eval_locked(layer, &form);
            changes.extend(out.changes);
            if let Err(message) = out.value {
                errors.push(FormError { line, message });
            }
        }
        (changes, errors)
    }

    /// Load an extension's prelude: each top-level form must be a `define` or
    /// `define-syntax`. A form that is not, or that defines a name in `taken`
    /// (the core vocabulary, or an earlier prelude's), is reported and skipped;
    /// a form that fails to evaluate is reported. Names defined are added to
    /// `taken`. Returns the problems, one line each.
    pub fn load_prelude(&self, owner: &str, src: &str, taken: &mut std::collections::HashSet<String>) -> Vec<String> {
        let mut problems = Vec::new();
        for (line, form) in split_forms(src) {
            let Some(name) = defined_name(&form) else {
                problems.push(format!("{owner} prelude:{line}: a prelude may only define"));
                continue;
            };
            if !taken.insert(name.clone()) {
                problems.push(format!("{owner} prelude:{line}: `{name}` is already defined; skipped"));
                continue;
            }
            if let Err(e) = self.eval(Layer::Config, &form).value {
                problems.push(format!("{owner} prelude:{line}: {e}"));
            }
        }
        problems
    }

    /// Every entry of one layer (values always present), in a stable order.
    pub fn layer_entries(&self, layer: Layer) -> Result<Vec<Change>, String> {
        let _guard = self.lock.lock().unwrap();
        let printed = self.session.eval(&format!("(store-layer-entries {:?})", layer.name()))?;
        let mut entries: Vec<Change> = list_of(&printed)?
            .iter()
            .map(|e| {
                let parts = e.as_list().ok_or("layer entry is not a list")?;
                match parts {
                    [Datum::Str(section), id, value] => Ok(Change {
                        section: Section::parse(section).ok_or("unknown store section")?,
                        id: id.clone(),
                        value: Some(value.clone()),
                    }),
                    _ => Err("malformed layer entry".to_string()),
                }
            })
            .collect::<Result<_, String>>()?;
        entries.sort_by(|a, b| a.section.cmp(&b.section).then_with(|| sexpr::render(&a.id).cmp(&sexpr::render(&b.id))));
        Ok(entries)
    }

    /// The `custom` layer as the text of `custom.scm`, written by the store
    /// itself (`store-dump-custom`): literal store calls only, so the file is
    /// both a store program and plain data for the reader.
    pub fn dump_custom(&self) -> Result<String, String> {
        let _guard = self.lock.lock().unwrap();
        match sexpr::parse(&self.session.eval("(store-dump-custom)")?)? {
            Datum::Str(text) => Ok(text),
            _ => Err("store-dump-custom did not return a string".into()),
        }
    }

    fn take_changes_locked(&self) -> Result<Vec<Change>, String> {
        let printed = self.session.eval("(store-take-changes)")?;
        parse_changes(&printed)
    }
}

/// Read `custom.scm` without the session: its literal store calls, turned
/// into the change set loading it would produce. Used before the session is
/// ready (boot reads two values early) and as the fallback when it is not.
/// A call that is not one of the literal store calls is an error.
pub fn changes_from_literal_program(text: &str) -> Result<Vec<Change>, String> {
    sexpr::parse_all(text)?.iter().map(literal_call).collect()
}

fn literal_call(form: &Datum) -> Result<Change, String> {
    let bad = || format!("custom.scm: not a literal store call: {}", sexpr::render(form));
    let items = form.as_list().ok_or_else(bad)?;
    let args: Vec<Datum> = items.iter().skip(1).map(unquote).collect();
    let name = |d: &Datum| d.text().map(|s| Datum::Str(s.to_string())).ok_or_else(bad);
    let head = items.first().and_then(Datum::as_sym).ok_or_else(bad)?;
    let (section, id, value) = match (head, args.as_slice()) {
        ("opt-set!", [key, value]) => (Section::Option, name(key)?, value.clone()),
        ("bind!" | "unbind!", [mode, panel, key, rest @ ..]) => {
            let panel = match panel {
                Datum::Bool(false) => Datum::Bool(false),
                p => name(p)?,
            };
            let id = Datum::List(vec![name(mode)?, panel, key.clone()]);
            let value = if head == "unbind!" {
                Datum::Sym("unbound".into())
            } else {
                let [intent, extra @ ..] = rest else { return Err(bad()) };
                let when = extra.iter().find(|d| d.as_str().is_none()).cloned().unwrap_or(Datum::Bool(false));
                let label = extra.iter().find(|d| d.as_str().is_some()).cloned().unwrap_or(Datum::Bool(false));
                Datum::List(vec![intent.clone(), when, label])
            };
            (Section::Binding, id, value)
        }
        ("command!", [cmd, description, intent]) => {
            (Section::Command, name(cmd)?, Datum::List(vec![description.clone(), intent.clone()]))
        }
        ("order!", [order, items]) => {
            let items = items.as_list().ok_or_else(bad)?.iter().map(name).collect::<Result<_, _>>()?;
            (Section::Order, name(order)?, Datum::List(items))
        }
        ("layout!", [layout, tree]) => (Section::Layout, name(layout)?, tree.clone()),
        _ => return Err(bad()),
    };
    Ok(Change { section, id, value: Some(value) })
}

/// `(quote x)` → `x`; anything else unchanged.
fn unquote(d: &Datum) -> Datum {
    match d.as_list() {
        Some([Datum::Sym(q), inner]) if q == "quote" => inner.clone(),
        _ => d.clone(),
    }
}

/// The names the settings store itself defines — no prelude may redefine them.
pub const CORE_VOCABULARY: &[&str] = &[
    "opt", "opt-set!", "bind!", "unbind!", "command!", "order!", "layout!", "customize!", "reset!",
    "os", "wsl?", "hostname", "getenv", "executable?",
];

/// The name a `(define name …)`, `(define (name …) …)`, or
/// `(define-syntax name …)` form defines; `None` for any other form.
fn defined_name(form: &str) -> Option<String> {
    let rest = form.trim_start().strip_prefix('(')?.trim_start();
    let rest = rest.strip_prefix("define-syntax").or_else(|| rest.strip_prefix("define"))?;
    if !rest.starts_with(|c: char| c.is_whitespace()) {
        return None;
    }
    let rest = rest.trim_start();
    let rest = rest.strip_prefix('(').unwrap_or(rest).trim_start();
    let name: String = rest.chars().take_while(|c| !c.is_whitespace() && *c != '(' && *c != ')').collect();
    (!name.is_empty()).then_some(name)
}

fn list_of(printed: &str) -> Result<Vec<Datum>, String> {
    match sexpr::parse(printed)? {
        Datum::List(items) => Ok(items),
        _ => Err("expected a list".into()),
    }
}

/// Decode a printed change set: `((section id #t value) (section id #f) …)`.
pub fn parse_changes(printed: &str) -> Result<Vec<Change>, String> {
    list_of(printed)?
        .iter()
        .map(|c| {
            let parts = c.as_list().ok_or("change is not a list")?;
            let (section, id, value) = match parts {
                [Datum::Str(s), id, Datum::Bool(true), v] => (s, id, Some(v.clone())),
                [Datum::Str(s), id, Datum::Bool(false)] => (s, id, None),
                _ => return Err("malformed change".to_string()),
            };
            Ok(Change {
                section: Section::parse(section).ok_or("unknown store section")?,
                id: id.clone(),
                value,
            })
        })
        .collect()
}

/// Split a store program into its top-level forms, each with the 1-based line
/// it starts on. Knows strings, `;` line comments, `#| |#` block comments,
/// character literals (`#\(`), and quote prefixes; it does not interpret the
/// forms.
pub fn split_forms(text: &str) -> Vec<(usize, String)> {
    let chars: Vec<char> = text.chars().collect();
    let mut forms = Vec::new();
    let mut i = 0;
    let mut line = 1;
    while i < chars.len() {
        let c = chars[i];
        if c == '\n' {
            line += 1;
            i += 1;
        } else if c.is_whitespace() {
            i += 1;
        } else if c == ';' {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
        } else if c == '#' && chars.get(i + 1) == Some(&'|') {
            i = skip_block_comment(&chars, i, &mut line);
        } else {
            let start_line = line;
            let start = i;
            i = scan_datum(&chars, i, &mut line);
            forms.push((start_line, chars[start..i].iter().collect()));
        }
    }
    forms
}

fn skip_block_comment(chars: &[char], mut i: usize, line: &mut usize) -> usize {
    let mut depth = 0;
    while i < chars.len() {
        if chars[i] == '#' && chars.get(i + 1) == Some(&'|') {
            depth += 1;
            i += 2;
        } else if chars[i] == '|' && chars.get(i + 1) == Some(&'#') {
            depth -= 1;
            i += 2;
            if depth == 0 {
                return i;
            }
        } else {
            if chars[i] == '\n' {
                *line += 1;
            }
            i += 1;
        }
    }
    i
}

/// Scan one datum starting at `i`; returns the index just past it.
fn scan_datum(chars: &[char], mut i: usize, line: &mut usize) -> usize {
    // Quote prefixes belong to the datum that follows.
    while i < chars.len() && matches!(chars[i], '\'' | '`' | ',' | '@') {
        i += 1;
    }
    if i >= chars.len() {
        return i;
    }
    match chars[i] {
        '(' | '[' => {
            let mut depth = 0;
            while i < chars.len() {
                match chars[i] {
                    '(' | '[' => {
                        depth += 1;
                        i += 1;
                    }
                    ')' | ']' => {
                        depth -= 1;
                        i += 1;
                        if depth == 0 {
                            return i;
                        }
                    }
                    '"' => i = scan_string(chars, i, line),
                    ';' => {
                        while i < chars.len() && chars[i] != '\n' {
                            i += 1;
                        }
                    }
                    '#' if chars.get(i + 1) == Some(&'|') => i = skip_block_comment(chars, i, line),
                    '#' if chars.get(i + 1) == Some(&'\\') => i = (i + 3).min(chars.len()),
                    '\n' => {
                        *line += 1;
                        i += 1;
                    }
                    _ => i += 1,
                }
            }
            i
        }
        '"' => scan_string(chars, i, line),
        _ => {
            if chars[i] == '#' && chars.get(i + 1) == Some(&'\\') {
                i = (i + 3).min(chars.len());
            }
            while i < chars.len() && !chars[i].is_whitespace() && !matches!(chars[i], '(' | ')' | '[' | ']' | ';') {
                i += 1;
            }
            i
        }
    }
}

fn scan_string(chars: &[char], mut i: usize, line: &mut usize) -> usize {
    i += 1; // opening quote
    while i < chars.len() {
        match chars[i] {
            '\\' => i += 2,
            '"' => return i + 1,
            '\n' => {
                *line += 1;
                i += 1;
            }
            _ => i += 1,
        }
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{OptionKind, OptionValue};
    use std::time::Duration;

    fn store() -> Store {
        let session = SchemeSession::spawn();
        assert!(session.wait_ready(Duration::from_secs(30)), "session ready");
        let store = Store::new(session);
        let def = |key: &str, kind: OptionKind, default: OptionValue| OptionDef {
            key: key.into(),
            category: "Test".into(),
            label: key.into(),
            kind,
            default,
            owner: None,
        };
        let style = OptionKind::Record(vec![("fg".into(), OptionKind::Text), ("bold".into(), OptionKind::Bool)]);
        store
            .declare_options(&[
                def("border-style", OptionKind::Choice(vec!["single".into(), "double".into()]), OptionValue::Text("single".into())),
                def("n", OptionKind::Int { min: 0, max: 9 }, OptionValue::Int(1)),
                def("exts", OptionKind::text_list(), OptionValue::List(vec![OptionValue::Text("rs".into())])),
                def("themes", OptionKind::Map(Box::new(style)), OptionValue::Map(Default::default())),
            ])
            .expect("declare");
        store
    }

    fn option_change(key: &str, value: Option<Datum>) -> Change {
        Change { section: Section::Option, id: Datum::Str(key.into()), value }
    }

    #[test]
    fn a_write_changes_the_effective_value_and_reports_it() {
        let s = store();
        let out = s.eval(Layer::Config, "(opt-set! 'n 5)");
        assert!(out.value.is_ok(), "{:?}", out.value);
        assert_eq!(out.changes, vec![option_change("n", Some(Datum::Int(5)))]);
        assert_eq!(s.eval(Layer::Session, "(opt 'n)").value.unwrap(), "5");
    }

    #[test]
    fn layers_decide_the_effective_value_and_reset_reveals_config() {
        let s = store();
        s.eval(Layer::Config, "(opt-set! 'n 2)");
        s.eval(Layer::Session, "(opt-set! 'n 7)");
        // A custom write under a session value changes nothing effective.
        let hidden = s.eval(Layer::Custom, "(opt-set! 'n 3)");
        assert!(hidden.changes.is_empty(), "{:?}", hidden.changes);
        assert_eq!(s.eval(Layer::Session, "(opt 'n)").value.unwrap(), "7");
        // customize! targets the custom layer from any layer.
        let s2 = store();
        s2.eval(Layer::Config, "(opt-set! 'n 2)");
        let out = s2.eval(Layer::Session, "(customize! (opt-set! 'n 4))");
        assert_eq!(out.changes, vec![option_change("n", Some(Datum::Int(4)))]);
        let reset = s2.eval(Layer::Session, "(reset! 'n)");
        assert_eq!(reset.changes, vec![option_change("n", Some(Datum::Int(2)))], "config shows through");
    }

    #[test]
    fn invalid_and_undeclared_writes_are_errors_without_changes() {
        let s = store();
        for bad in ["(opt-set! 'n 10)", "(opt-set! 'n \"5\")", "(opt-set! 'border-style \"wavy\")", "(opt-set! 'nope 1)"] {
            let out = s.eval(Layer::Config, bad);
            assert!(out.value.is_err(), "{bad} should fail");
            assert!(out.changes.is_empty(), "{bad} changed {:?}", out.changes);
        }
    }

    #[test]
    fn structured_values_are_validated_and_normalized() {
        let s = store();
        let ok = s.eval(Layer::Config, "(opt-set! 'themes '((dark ((fg \"#000\") (bold #t)))))");
        assert!(ok.value.is_ok(), "{:?}", ok.value);
        let expected = sexpr::parse(r##"(("dark" (("fg" "#000") ("bold" #t))))"##).unwrap();
        assert_eq!(ok.changes, vec![option_change("themes", Some(expected))]);
        let missing_field = s.eval(Layer::Config, "(opt-set! 'themes '((dark ((fg \"#000\")))))");
        assert!(missing_field.value.is_err());
        let list = s.eval(Layer::Config, "(opt-set! 'exts '(\"rs\" \"md\"))");
        assert!(list.value.is_ok());
        assert!(s.eval(Layer::Config, "(opt-set! 'exts '(1 2))").value.is_err());
    }

    #[test]
    fn bindings_record_when_label_and_tombstones() {
        let s = store();
        let out = s.eval(Layer::Config, "(bind! 'file 'file \"o\" '(navigate-to \"/tmp\") 'has-selection \"open tmp\")");
        assert!(out.value.is_ok(), "{:?}", out.value);
        let id = sexpr::parse(r#"("file" "file" "o")"#).unwrap();
        let value = sexpr::parse(r#"((navigate-to "/tmp") has-selection "open tmp")"#).unwrap();
        assert_eq!(out.changes, vec![Change { section: Section::Binding, id: id.clone(), value: Some(value) }]);
        let un = s.eval(Layer::Custom, "(unbind! 'file 'file \"o\")");
        assert_eq!(un.changes, vec![Change { section: Section::Binding, id, value: Some(Datum::Sym("unbound".into())) }]);
        let any_panel = s.eval(Layer::Config, "(bind! 'file #f \"x\" 'quit)");
        let id = sexpr::parse(r#"("file" #f "x")"#).unwrap();
        assert_eq!(any_panel.changes[0].id, id);
    }

    #[test]
    fn a_program_runs_form_by_form_and_reports_failing_lines() {
        let s = store();
        let program = "; a comment with (parens\n\
                       (define (twice x) (* 2 x))\n\
                       (opt-set! 'n 99)\n\
                       (opt-set! 'n (twice 3))\n\
                       (bind! 'file 'file \"#\" '(navigate-to \"a;b(\"))\n";
        let (changes, errors) = s.eval_program(Layer::Config, program);
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert_eq!(errors[0].line, 3);
        assert!(changes.contains(&option_change("n", Some(Datum::Int(6)))), "{changes:?}");
        assert!(changes.iter().any(|c| c.section == Section::Binding), "the form after the failure applied");
    }

    #[test]
    fn the_custom_layer_dumps_and_reloads_to_the_same_entries() {
        let s = store();
        s.eval(
            Layer::Custom,
            "(opt-set! 'border-style \"double\") (opt-set! 'exts '(\"md\")) \
             (opt-set! 'themes '((x ((fg \"a\\\"b\") (bold #f))))) \
             (bind! 'file 'file \"o\" 'quit \"leave\") (unbind! 'select #f \"q\") \
             (command! 'hello \"say hi\" '(notify \"hi\")) (order! 'extensions '(pty preview))",
        );
        let entries = s.layer_entries(Layer::Custom).unwrap();
        assert_eq!(entries.len(), 7, "{entries:?}");
        let text = s.dump_custom().unwrap();
        for line in [
            "(opt-set! 'border-style \"double\")",
            "(opt-set! 'exts '(\"md\"))",
            "(bind! 'file 'file \"o\" 'quit \"leave\")",
            "(unbind! 'select #f \"q\")",
            "(command! 'hello \"say hi\" '(notify \"hi\"))",
            "(order! 'extensions '(\"pty\" \"preview\"))",
        ] {
            assert!(text.lines().any(|l| l == line), "missing {line} in\n{text}");
        }
        let calls: Vec<&str> = text.lines().filter(|l| !l.starts_with(';')).collect();
        assert!(calls.windows(2).all(|w| w[0] <= w[1]), "sorted:\n{text}");
        // The dump is plain data for the reader…
        assert!(sexpr::parse_all(&text).is_ok(), "{text}");
        // …and a program that rebuilds the same layer.
        // The fallback reader sees the same entries without the session.
        let mut fallback = changes_from_literal_program(&text).unwrap();
        fallback.sort_by(|a, b| a.section.cmp(&b.section).then_with(|| sexpr::render(&a.id).cmp(&sexpr::render(&b.id))));
        assert_eq!(fallback, entries);
        let fresh = store();
        let (_, errors) = fresh.eval_program(Layer::Custom, &text);
        assert!(errors.is_empty(), "{errors:?}\n{text}");
        assert_eq!(fresh.layer_entries(Layer::Custom).unwrap(), entries);
    }

    #[test]
    fn custom_scm_keeps_an_undeclared_option_but_elsewhere_it_is_an_error() {
        let s = store();
        let (changes, errors) = s.eval_program(Layer::Custom, "(opt-set! 'gone.ext \"kept\")\n(opt-set! 'n 3)\n");
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(changes, vec![option_change("n", Some(Datum::Int(3)))], "the undeclared entry has no effect");
        assert!(s.dump_custom().unwrap().lines().any(|l| l == "(opt-set! 'gone.ext \"kept\")"), "it is written back");
        assert!(s.eval(Layer::Session, "(customize! (opt-set! 'typo 1))").value.is_err(), "a REPL typo is still reported");
        assert!(s.eval(Layer::Config, "(opt-set! 'typo 1)").value.is_err());
    }

    #[test]
    fn layout_writes_are_checked_and_a_rejected_one_keeps_the_previous() {
        let s = store();
        let good = s.eval(Layer::Config, "(layout! 'file '(column (panel file) (panel help (size 1))))");
        assert!(good.value.is_ok(), "{:?}", good.value);
        assert_eq!(good.changes.len(), 1);
        let bad = s.eval(Layer::Config, "(layout! 'file '(column (panel nav)))");
        assert!(bad.value.as_ref().is_err_and(|e| e.contains("needs panel file")), "{:?}", bad.value);
        assert!(bad.changes.is_empty(), "the previous layout stays");
    }

    #[test]
    fn environment_procedures_answer() {
        let s = store();
        let os = s.eval(Layer::Session, "(os)").value.unwrap();
        assert!(["\"macos\"", "\"linux\"", "\"windows\""].contains(&os.as_str()), "{os}");
        assert_eq!(s.eval(Layer::Session, "(getenv \"WASDF_SURELY_UNSET_VAR\")").value.unwrap(), "#f");
        assert_eq!(s.eval(Layer::Session, "(executable? \"sh\")").value.unwrap(), "#t");
        assert!(s.eval(Layer::Session, "(string? (hostname))").value.unwrap() == "#t");
    }

    #[test]
    fn a_prelude_may_only_define_and_never_redefine() {
        let s = store();
        let mut taken: std::collections::HashSet<String> = CORE_VOCABULARY.iter().map(|s| s.to_string()).collect();
        let problems = s.load_prelude(
            "t",
            "(define (bump) (opt-set! 'n (+ (opt 'n) 1)))\n(opt-set! 'n 9)\n(define (opt k) k)\n(define-syntax twice (syntax-rules () ((_ e) (begin e e))))",
            &mut taken,
        );
        assert_eq!(problems.len(), 2, "{problems:?}");
        assert!(problems[0].contains(":2:") && problems[1].contains("`opt`"), "{problems:?}");
        let out = s.eval(Layer::Session, "(twice (bump))");
        assert!(out.value.is_ok(), "{:?}", out.value);
        assert_eq!(s.eval(Layer::Session, "(opt 'n)").value.unwrap(), "3");
        let again = s.load_prelude("u", "(define (bump) 0)", &mut taken);
        assert_eq!(again.len(), 1, "an earlier prelude's name is taken too");
    }

    #[test]
    fn split_forms_tracks_lines_and_skips_comments() {
        let text = "#| block\n comment |#\n'(a \"b)\" #\\( ) ; tail\n\nsym\n(x [y])";
        let forms = split_forms(text);
        let lines: Vec<usize> = forms.iter().map(|(l, _)| *l).collect();
        assert_eq!(lines, vec![3, 5, 6]);
        assert_eq!(forms[0].1, "'(a \"b)\" #\\( )");
        assert_eq!(forms[1].1, "sym");
        assert_eq!(forms[2].1, "(x [y])");
    }
}
