//! The command registry: palette commands. Each command has a name, a
//! description, and a registered intent re-injected when chosen. Merged by name
//! (last wins).

use crate::core::{Candidate, ExtensionValue, Intent};

#[derive(Debug, Clone, PartialEq)]
pub struct CommandDef {
    pub name: String,
    pub description: String,
    pub intent: Intent,
}

#[derive(Debug, Default, Clone)]
pub struct CommandRegistry {
    commands: Vec<CommandDef>,
}

impl CommandRegistry {
    pub fn new() -> Self {
        CommandRegistry { commands: Vec::new() }
    }

    /// Register or replace a command by name (last wins).
    pub fn register(&mut self, def: CommandDef) {
        if let Some(slot) = self.commands.iter_mut().find(|c| c.name == def.name) {
            *slot = def;
        } else {
            self.commands.push(def);
        }
    }

    pub fn extend(&mut self, defs: impl IntoIterator<Item = CommandDef>) {
        for d in defs {
            self.register(d);
        }
    }

    /// The intent registered for a command name.
    pub fn intent_of(&self, name: &str) -> Option<Intent> {
        self.commands.iter().find(|c| c.name == name).map(|c| c.intent.clone())
    }

    /// Candidates for the command palette: value carries the command name.
    pub fn candidates(&self) -> Vec<Candidate> {
        self.commands
            .iter()
            .map(|c| {
                let label = format!("{:<16} {}", c.name, c.description);
                Candidate::new(label, ExtensionValue::String(c.name.clone()))
            })
            .collect()
    }
}

impl crate::core::CommandLookup for CommandRegistry {
    fn intent_of(&self, name: &str) -> Option<Intent> {
        CommandRegistry::intent_of(self, name)
    }
    fn command_candidates(&self) -> Vec<Candidate> {
        self.candidates()
    }
}

