//! Bookmarks extension: handles `goto` intents from virtual bookmark leaves and
//! the `add-bookmark` command. Shares its path list with the VFS bookmarks
//! provider via an `Arc<RwLock<…>>` so both see the same data.

use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use crate::core::{AppState, ExtensionIntent, Intent};
use crate::extension::Extension;

pub struct BookmarksExtension {
    paths: Arc<RwLock<Vec<PathBuf>>>,
}

impl BookmarksExtension {
    pub fn new(paths: Arc<RwLock<Vec<PathBuf>>>) -> Self {
        BookmarksExtension { paths }
    }

    pub fn add(&self, path: PathBuf) {
        let mut guard = self.paths.write().unwrap();
        if !guard.contains(&path) {
            guard.push(path);
        }
    }
}

impl Extension for BookmarksExtension {
    fn id(&self) -> &str {
        "bookmarks"
    }

    fn handle_intent(&self, intent: &ExtensionIntent, state: &AppState) -> Vec<Intent> {
        match intent.intent.as_str() {
            "goto" => {
                if let Some(path) = intent.data.as_path() {
                    vec![Intent::NavigateTo(path.clone())]
                } else {
                    Vec::new()
                }
            }
            "add" => {
                // Add the current cwd (or the cursor entry if a real file/dir) to bookmarks.
                let target = state
                    .current_entry()
                    .map(|e| e.path.clone())
                    .filter(|p| p.is_absolute())
                    .unwrap_or_else(|| state.cwd.clone());
                self.add(target);
                Vec::new()
            }
            _ => Vec::new(),
        }
    }
}
