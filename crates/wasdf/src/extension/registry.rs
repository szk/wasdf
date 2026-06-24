//! ExtensionRegistry: holds registered extensions, routes extension intents to
//! their owner by id, and exposes the active content provider.

use crate::core::{AppState, ExtensionIntent, Intent};
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
            eprintln!("extension id collision, disabling later one: {}", ext.id());
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

    /// Route an extension intent to its owning extension.
    pub fn dispatch(&self, intent: &ExtensionIntent, state: &AppState) -> Vec<Intent> {
        match self.extensions.iter().find(|e| e.id() == intent.extension) {
            Some(e) => e.handle_intent(intent, state),
            None => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extension::Extension;

    /// A minimal extension that reacts to cursor-changed by emitting a Refresh,
    /// to prove the event is broadcast to *every* subscriber (not just the
    /// content provider).
    struct Subscriber(&'static str);
    impl Extension for Subscriber {
        fn id(&self) -> &str {
            self.0
        }
        fn on_cursor_changed(&self, _state: &AppState) -> Vec<Intent> {
            vec![Intent::Refresh]
        }
    }

    #[test]
    fn cursor_changed_broadcasts_to_all_subscribers() {
        let mut reg = ExtensionRegistry::new();
        reg.register(Box::new(Subscriber("a")));
        reg.register(Box::new(Subscriber("b")));
        let state = AppState::new(std::env::temp_dir());
        let reactions: Vec<Intent> =
            reg.iter().flat_map(|e| e.on_cursor_changed(&state)).collect();
        assert_eq!(reactions.len(), 2, "both subscribers react to cursor-changed");
        assert!(reactions.iter().all(|i| matches!(i, Intent::Refresh)));
    }
}
