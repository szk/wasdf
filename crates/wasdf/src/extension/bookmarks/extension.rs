//! Bookmarks extension: handles `goto` intents from virtual bookmark leaves and
//! the `add-bookmark` command. Owns the shared path list and hands out its own
//! `BookmarksProvider` (sharing the same `Arc<RwLock<…>>`) via `vfs_providers()`
//! — an ordinary bundled extension, no different from any other that owns a
//! VFS mount. Its entry keybinding (`B` → add) is declared as Scheme data
//! ([`scheme_source`]), evaluated at registration, same as preview/cmdop.
//!
//! Persists to its own file, `bookmark.scm`, under the config directory — a
//! plain list of paths, not the shared `config.scm` managed block (bookmarks
//! aren't a *setting*, and the file name is this module's own business, not
//! the kernel's: it only hands the extension a directory).

use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use super::provider::BookmarksProvider;
use crate::core::{AppState, ExtensionIntent, Intent};
use crate::extension::Extension;
use crate::script::condition::Conditions;
use crate::vfs::VfsProvider;

/// The extension's entry keymap: `B` in the file panel adds the current path
/// as a bookmark. Declared as Scheme data and evaluated at registration (see
/// `preview`/`cmdop`'s `keymap.scm` for the same pattern).
const KEYMAP_SCHEME: &str = include_str!("keymap.scm");

/// The bookmarks file name, under the config directory the kernel hands in.
/// Chosen and owned here, not by the kernel — the kernel only knows the
/// directory.
const BOOKMARK_FILE: &str = "bookmark.scm";

pub struct BookmarksExtension {
    paths: Arc<RwLock<Vec<PathBuf>>>,
    /// The full path to `bookmark.scm`, or `None` when there is no config
    /// directory (persistence is then a no-op — in-memory only).
    file: Option<PathBuf>,
}

impl BookmarksExtension {
    pub fn new(config_dir: Option<PathBuf>) -> Self {
        let file = config_dir.map(|d| d.join(BOOKMARK_FILE));
        let paths = file.as_deref().map(load_bookmarks).unwrap_or_default();
        BookmarksExtension { paths: Arc::new(RwLock::new(paths)), file }
    }

    /// Add `path`, persisting on success. Returns `false` (no-op) if it was
    /// already bookmarked.
    pub fn add(&self, path: PathBuf) -> bool {
        let mut guard = self.paths.write().unwrap();
        if guard.contains(&path) {
            return false;
        }
        guard.push(path);
        if let Some(file) = &self.file {
            let _ = save_bookmarks(file, &guard);
        }
        true
    }

    /// Remove `path`, persisting on success. Returns `false` if it wasn't
    /// bookmarked.
    pub fn remove(&self, path: &Path) -> bool {
        let mut guard = self.paths.write().unwrap();
        let Some(pos) = guard.iter().position(|p| p == path) else { return false };
        guard.remove(pos);
        if let Some(file) = &self.file {
            let _ = save_bookmarks(file, &guard);
        }
        true
    }
}

/// True when the cursor sits on one of *this* extension's own bookmark leaves
/// (its `activation` is a `bookmarks "goto"` — the only shape `BookmarksProvider`
/// ever emits). Backs the `bookmarks-cursor` `:when` predicate that gates
/// `Backspace`/`Delete` removing the bookmark instead of falling through to the
/// core `delete-selected` (which only applies to real paths).
fn is_bookmark_leaf(state: &AppState) -> bool {
    state
        .current_entry()
        .and_then(|e| e.activation.as_ref())
        .map(|ei| ei.extension == "bookmarks" && ei.intent == "goto")
        .unwrap_or(false)
}

/// Parse `(quote ("/path/one" "/path/two" …))` from `path`; an absent or
/// malformed file yields an empty list (first run, or a fault — disable,
/// don't crash the boot).
fn load_bookmarks(path: &Path) -> Vec<PathBuf> {
    let Ok(text) = std::fs::read_to_string(path) else { return Vec::new() };
    let Ok(datum) = crate::script::sexpr::parse_quoted(&text) else { return Vec::new() };
    let Some(inner) = datum.as_list() else {
        return Vec::new();
    };
    inner.iter().filter_map(|d| d.as_str().map(PathBuf::from)).collect()
}

/// Overwrite `path` with the full current list — no managed block to
/// preserve, so a plain atomic rewrite is enough (unlike `theme`/`settings`,
/// which share `config.scm` with hand-editable content).
fn save_bookmarks(path: &Path, paths: &[PathBuf]) -> io::Result<()> {
    let mut body = String::from(
        ";; wasdf bookmarks — managed by the app; hand-editing is safe (one path per line).\n(quote (\n",
    );
    for p in paths {
        body.push_str(&format!("  {:?}\n", p.display().to_string()));
    }
    body.push_str("))\n");
    let dir = path.parent().ok_or_else(|| io::Error::other("no parent dir"))?;
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(".bookmark_scm_tmp");
    std::fs::write(&tmp, &body)?;
    std::fs::rename(&tmp, path)
}

impl Extension for BookmarksExtension {
    fn id(&self) -> &str {
        "bookmarks"
    }

    fn about(&self) -> crate::extension::About {
        crate::extension::About::new("Bookmarks", "Bookmark directories and open them from @/bookmarks")
    }

    fn vfs_providers(&self) -> Vec<Box<dyn VfsProvider>> {
        vec![Box::new(BookmarksProvider::new(self.id().to_string(), self.paths.clone()))]
    }

    fn scheme_source(&self) -> Option<String> {
        Some(KEYMAP_SCHEME.to_string())
    }

    fn register_conditions(&self, c: &mut Conditions) {
        c.register("bookmarks-cursor", Box::new(is_bookmark_leaf));
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
                let added = self.add(target.clone());
                let msg = if added {
                    format!("Bookmarked: {}", target.display())
                } else {
                    format!("Already bookmarked: {}", target.display())
                };
                vec![Intent::Notify(msg)]
            }
            "remove-cursor" => {
                // Only ever bound while `bookmarks-cursor` holds, but re-check:
                // an intent can also arrive from the palette/scripting, not just
                // the gated key.
                let Some(activation) = state.current_entry().and_then(|e| e.activation.clone())
                else {
                    return Vec::new();
                };
                if activation.extension != "bookmarks" || activation.intent != "goto" {
                    return Vec::new();
                }
                let Some(real) = activation.data.as_path().cloned() else { return Vec::new() };
                if self.remove(&real) {
                    vec![Intent::Refresh, Intent::Notify(format!("Bookmark removed: {}", real.display()))]
                } else {
                    Vec::new()
                }
            }
            _ => Vec::new(),
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch_dir() -> PathBuf {
        std::env::temp_dir().join(format!("wasdf-bookmarks-test-{}-{:?}", std::process::id(), std::thread::current().id()))
    }

    #[test]
    fn add_persists_and_reloading_restores_it() {
        let dir = scratch_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join(BOOKMARK_FILE);

        let ext = BookmarksExtension::new(Some(dir.clone()));
        ext.add(PathBuf::from("/tmp/projects"));
        assert!(file.exists(), "add() writes bookmark.scm");

        let reloaded = BookmarksExtension::new(Some(dir.clone()));
        assert_eq!(*reloaded.paths.read().unwrap(), vec![PathBuf::from("/tmp/projects")]);

        let _ = std::fs::remove_dir_all(&dir);
    }

}
