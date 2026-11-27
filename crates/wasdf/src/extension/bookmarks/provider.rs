//! `BookmarksProvider`: the VFS side of the bookmarks extension. A data provider
//! backed by a shared list of saved real paths. Leaves carry an `activation`
//! intent so `BookmarksExtension` can receive and forward it as
//! `Intent::NavigateTo(real_path)`. Shares its `Arc<RwLock<…>>` with the
//! extension so both see the same data — the mount is otherwise a plain
//! `VfsProvider`, no different from any other extension's.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use crate::core::{Entry, ExtensionIntent, ExtensionValue};
use crate::vfs::{vfs_root, VfsProvider};

pub struct BookmarksProvider {
    id: String,
    paths: Arc<RwLock<Vec<PathBuf>>>,
}

impl BookmarksProvider {
    pub fn new(id: String, paths: Arc<RwLock<Vec<PathBuf>>>) -> Self {
        BookmarksProvider { id, paths }
    }
}

impl VfsProvider for BookmarksProvider {
    fn mount(&self) -> &str {
        &self.id
    }

    fn list(&self, _path: &Path) -> io::Result<Vec<Entry>> {
        let saved = self.paths.read().map_err(|_| {
            io::Error::other("bookmark lock poisoned")
        })?;
        Ok(saved
            .iter()
            .enumerate()
            .map(|(i, real)| {
                let name = real
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| real.display().to_string());
                let vpath = vfs_root().join(&self.id).join(i.to_string());
                let mut e = Entry::virtual_leaf(vpath, name);
                e.activation = Some(ExtensionIntent {
                    extension: self.id.clone(),
                    intent: "goto".into(),
                    data: ExtensionValue::Path(real.clone()),
                });
                e
            })
            .collect())
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────
