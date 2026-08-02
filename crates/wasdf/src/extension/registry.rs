//! ExtensionRegistry: holds registered extensions, routes extension intents to
//! their owner by id, and exposes the active content provider.

use crate::core::{AppState, Entry, ExtensionIntent, Intent};
use crate::extension::Extension;

#[derive(Default)]
pub struct ExtensionRegistry {
    extensions: Vec<Box<dyn Extension>>,
}

impl ExtensionRegistry {
    pub fn new() -> Self {
        ExtensionRegistry { extensions: Vec::new() }
    }

    pub fn register(&mut self, ext: Box<dyn Extension>) {
        if self.extensions.iter().any(|e| e.id() == ext.id()) {
            crate::services::messages::log(format!("extension id collision, disabling later one: {}", ext.id()), true);
            return;
        }
        self.extensions.push(ext);
    }

    pub fn iter(&self) -> impl Iterator<Item = &dyn Extension> {
        self.extensions.iter().map(|b| b.as_ref())
    }

    /// Whether any registered extension produces function-panel content.
    pub fn has_function_content(&self) -> bool {
        self.extensions.iter().any(|e| e.provides_function_content())
    }

    /// The active function-panel content provider — the first extension that
    /// produces content — whose render hook the UI calls and whose
    /// `accept_content` receives reads.
    pub fn provider(&self) -> Option<&dyn Extension> {
        self.iter().find(|e| e.provides_function_content())
    }

    /// The extension with the given id, if registered.
    pub fn find(&self, id: &str) -> Option<&dyn Extension> {
        self.iter().find(|e| e.id() == id)
    }

    /// The first registered extension with a color opinion on `entry` (e.g.
    /// `lscolors`), if any — first-wins by registration order, same contract
    /// as `provider()`.
    pub fn entry_style(&self, entry: &Entry) -> Option<ratatui::style::Style> {
        self.iter().find_map(|e| e.entry_style(entry))
    }

    /// Route an extension intent to its owning extension.
    pub fn dispatch(&self, intent: &ExtensionIntent, state: &AppState) -> Vec<Intent> {
        match self.extensions.iter().find(|e| e.id() == intent.extension) {
            Some(e) => e.handle_intent(intent, state),
            None => Vec::new(),
        }
    }
}
