//! The `:when` condition evaluator. The dynamic-predicate class: a closed
//! grammar of registered predicates combined with and/or/not, parsed once into
//! an AST and evaluated natively per keypress. Defaults are false.

use std::collections::HashMap;

use crate::core::{AppState, InputFormState, Mode, SelectPhase, SubLayout};

/// A condition AST. Unknown predicates evaluate to false.
#[derive(Debug, Clone, PartialEq)]
pub enum Cond {
    Always,
    Pred(String),
    /// `(option "key")`: a bool option is on. False when undeclared.
    Option(String),
    Not(Box<Cond>),
    And(Vec<Cond>),
    Or(Vec<Cond>),
}

impl Cond {
    pub fn pred(name: &str) -> Cond {
        Cond::Pred(name.into())
    }
    pub fn not(c: Cond) -> Cond {
        Cond::Not(Box::new(c))
    }
}

type PredFn = Box<dyn Fn(&AppState) -> bool + Send + Sync>;

/// The predicate registry. Core predicates are pre-registered; extensions add
/// their own by name without touching core.
pub struct Conditions {
    preds: HashMap<String, PredFn>,
    /// Read by `(option "key")`; bound once the OptionStore is built.
    options: Option<std::sync::Arc<crate::core::OptionStore>>,
}

impl Default for Conditions {
    fn default() -> Self {
        let mut c = Conditions { preds: HashMap::new(), options: None };
        c.register_core();
        c
    }
}

impl Conditions {
    /// Register a named predicate (extension predicates land here too).
    pub fn register(&mut self, name: impl Into<String>, f: PredFn) {
        self.preds.insert(name.into(), f);
    }

    pub fn bind_options(&mut self, options: std::sync::Arc<crate::core::OptionStore>) {
        self.options = Some(options);
    }

    /// Evaluate a condition against the state. Unknown predicate → false.
    pub fn eval(&self, cond: &Cond, state: &AppState) -> bool {
        match cond {
            Cond::Always => true,
            Cond::Pred(name) => self.preds.get(name).map(|f| f(state)).unwrap_or(false),
            Cond::Option(key) => self.options.as_ref().is_some_and(|o| o.def(key).is_some() && o.get_bool(key)),
            Cond::Not(c) => !self.eval(c, state),
            Cond::And(cs) => cs.iter().all(|c| self.eval(c, state)),
            Cond::Or(cs) => cs.iter().any(|c| self.eval(c, state)),
        }
    }

    fn register_core(&mut self) {
        self.register("has-selection", Box::new(|s| !s.selection.is_empty()));
        self.register(
            "cursor-is-dir",
            Box::new(|s| s.current_entry().map(|e| e.is_dir).unwrap_or(false)),
        );
        self.register(
            "cursor-is-file",
            Box::new(|s| s.current_entry().map(|e| !e.is_dir).unwrap_or(false)),
        );
        self.register(
            "select-phase-navigate",
            Box::new(|s| s.select.as_ref().map(|x| x.phase == SelectPhase::Navigate).unwrap_or(false)),
        );
        self.register(
            "select-phase-input",
            Box::new(|s| s.select.as_ref().map(|x| x.phase == SelectPhase::Input).unwrap_or(false)),
        );
        self.register(
            "sublayout-content",
            Box::new(|s| s.function.sublayout == SubLayout::Content),
        );
        self.register(
            "sublayout-exec",
            Box::new(|s| s.function.sublayout == SubLayout::Exec),
        );
        self.register("function-visible", Box::new(|s| s.function.visible));
        self.register("has-filter", Box::new(|s| !s.filter_stack.is_empty()));
        // Layout conditions for the select layout (app/layout.scm).
        self.register(
            "select-has-input",
            Box::new(|s| matches!(s.mode(), crate::core::Mode::Select(spec) if !matches!(spec.input, crate::core::SelectInput::None))),
        );
        self.register(
            "select-has-function",
            Box::new(|s| matches!(s.mode(), crate::core::Mode::Select(spec) if spec.function_hint.is_some())),
        );
        self.register("cmdop-active", Box::new(|s| s.function.form_owner.as_deref() == Some("cmdop")));
        self.register("input-captured", Box::new(|s| s.input_capture.is_some()));
        self.register(
            "filter-maxed",
            Box::new(|s| s.filter_stack.len() >= crate::core::state::MAX_FILTER_DEPTH),
        );
        self.register(
            "mode-file",
            Box::new(|s| matches!(s.mode(), Mode::File)),
        );
        // Shallow-vs-deep `f` split (SEARCH_RENEW.md): a virtual cwd (@/...)
        // has nothing for the recursive walk to read, so `f` there means
        // "shallow-filter the current listing" instead of "deep search".
        self.register("cwd-virtual", Box::new(|s| crate::vfs::is_virtual(&s.cwd)));
        // Whether the active Select already holds a non-empty entry view (deep
        // file-search results, or an earlier filter/refine) — gates `f` as
        // shallow-refine inside a Select, as opposed to the file panel's own
        // `filter-list` binding.
        self.register(
            "select-has-view",
            Box::new(|s| {
                s.select.as_ref().and_then(|sel| sel.view.as_ref()).is_some_and(|v| !v.entries.is_empty())
            }),
        );
        // InputForm: gate the shared wasd/arrow/Space navigation bindings off
        // while free text is being typed (the `Text` kind, or a `List` row's
        // nested item edit), so those keys fall through as RawKeyEvent to
        // tui-input instead of being intercepted as Cursor*/ToggleSelect.
        self.register(
            "input-form-typing",
            Box::new(|s| match &s.input_form {
                Some(InputFormState::Text { .. }) => true,
                Some(InputFormState::List { editing, .. }) => editing.is_some(),
                _ => false,
            }),
        );
        // InputForm's `List` kind, row-browsing sub-state (not mid-item-edit):
        // `a`/`d` mean add/remove a row here instead of their usual
        // cursor-left/cursor-right meaning.
        self.register(
            "input-form-list-browsing",
            Box::new(|s| {
                matches!(&s.input_form, Some(InputFormState::List { editing: None, .. }))
            }),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{OptionDef, OptionKind, OptionStore, OptionValue};
    use crate::script::codec::cond_from_datum;
    use crate::script::sexpr::parse;

    #[test]
    fn option_conditions_follow_the_store_and_unknown_forms_are_false() {
        let mut c = Conditions::default();
        let state = AppState::new(std::env::temp_dir());
        let on = cond_from_datum(&parse(r#"(option "x.on")"#).unwrap());
        assert!(!c.eval(&on, &state), "false before any store is bound");
        let store = std::sync::Arc::new(OptionStore::new(vec![OptionDef {
            key: "x.on".into(),
            category: "X".into(),
            label: "on".into(),
            kind: OptionKind::Bool,
            default: OptionValue::Bool(true),
            owner: None,
        }]));
        c.bind_options(store.clone());
        assert!(c.eval(&on, &state));
        store.set("x.on", OptionValue::Bool(false));
        assert!(!c.eval(&on, &state), "read live, not captured at parse time");
        assert!(!c.eval(&cond_from_datum(&parse(r#"(option "undeclared")"#).unwrap()), &state));
        assert!(!c.eval(&cond_from_datum(&parse("(frobnicate 1)").unwrap()), &state));
    }
}
