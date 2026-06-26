//! The `:when` condition evaluator. The dynamic-predicate class: a closed
//! grammar of registered predicates combined with and/or/not, parsed once into
//! an AST and evaluated natively per keypress. Defaults are false.

use std::collections::HashMap;

use crate::core::{AppState, Mode, SelectPhase, SubLayout};

/// A condition AST. Unknown predicates evaluate to false.
#[derive(Debug, Clone, PartialEq)]
pub enum Cond {
    Always,
    Pred(String),
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
}

impl Default for Conditions {
    fn default() -> Self {
        let mut c = Conditions { preds: HashMap::new() };
        c.register_core();
        c
    }
}

impl Conditions {
    /// Register a named predicate (extension predicates land here too).
    pub fn register(&mut self, name: impl Into<String>, f: PredFn) {
        self.preds.insert(name.into(), f);
    }

    /// Evaluate a condition against the state. Unknown predicate → false.
    pub fn eval(&self, cond: &Cond, state: &AppState) -> bool {
        match cond {
            Cond::Always => true,
            Cond::Pred(name) => self.preds.get(name).map(|f| f(state)).unwrap_or(false),
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
        self.register(
            "mode-file",
            Box::new(|s| matches!(s.mode(), Mode::File)),
        );
    }
}
