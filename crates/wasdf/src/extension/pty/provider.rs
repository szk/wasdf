//! `PtyProvider`: the `@/pty` mount. Lists live sessions, each as a directory
//! holding its console.

use std::io;
use std::path::Path;
use std::sync::{Arc, RwLock};

use crate::core::{Entry, ExtensionIntent, ExtensionValue};
use crate::vfs::{VfsProvider, tail_after_mount, vfs_root};

use super::sessions::SessionSummary;

/// The leaf inside each session directory that shows (and attaches to) its
/// screen.
const CONSOLE: &str = "console";

pub struct PtyProvider {
    id: String,
    sessions: Arc<RwLock<Vec<SessionSummary>>>,
}

impl PtyProvider {
    pub fn new(id: String, sessions: Arc<RwLock<Vec<SessionSummary>>>) -> Self {
        PtyProvider { id, sessions }
    }
}

impl VfsProvider for PtyProvider {
    fn mount(&self) -> &str {
        &self.id
    }

    fn list(&self, path: &Path) -> io::Result<Vec<Entry>> {
        let sessions = self
            .sessions
            .read()
            .map_err(|_| io::Error::other("pty session lock poisoned"))?;

        // `@/pty` itself: one directory per session. Deeper: that session's
        // contents. A session is a *directory* even though it holds a single
        // leaf today, so listing its child processes later needs no path change.
        let Some(seg) = tail_after_mount(path) else {
            return Ok(sessions
                .iter()
                .map(|s| {
                    Entry::virtual_dir(
                        vfs_root().join(&self.id).join(s.id.to_string()),
                        s.label.clone(),
                    )
                })
                .collect());
        };

        let Ok(id) = seg.parse::<u64>() else {
            return Err(io::Error::new(io::ErrorKind::NotFound, format!("no pty {seg}")));
        };
        if !sessions.iter().any(|s| s.id == id) {
            return Err(io::Error::new(io::ErrorKind::NotFound, format!("no pty {id}")));
        }
        let mut e = Entry::virtual_leaf(
            vfs_root().join(&self.id).join(seg).join(CONSOLE),
            CONSOLE.into(),
        );
        e.activation = Some(ExtensionIntent {
            extension: self.id.clone(),
            intent: "attach".into(),
            data: ExtensionValue::Int(id as i64),
        });
        Ok(vec![e])
    }
}
