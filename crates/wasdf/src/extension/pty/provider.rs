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

#[cfg(test)]
mod tests {
    use super::*;

    fn provider(rows: Vec<SessionSummary>) -> PtyProvider {
        PtyProvider::new("pty".into(), Arc::new(RwLock::new(rows)))
    }

    fn row(id: u64, label: &str, running: bool) -> SessionSummary {
        SessionSummary { id, label: label.into(), running }
    }

    #[test]
    fn the_mount_root_lists_one_directory_per_session() {
        let p = provider(vec![row(1, "1: zsh", true), row(4, "4: rg foo (exit 1)", false)]);
        let got = p.list(&vfs_root().join("pty")).unwrap();
        assert_eq!(got.len(), 2);
        assert!(got.iter().all(|e| e.is_dir), "a session is a directory, so children can be added");
        assert_eq!(got[0].name, "1: zsh");
        assert_eq!(got[0].path, vfs_root().join("pty").join("1"));
        assert_eq!(got[1].name, "4: rg foo (exit 1)");
    }

    #[test]
    fn an_exited_session_is_still_listed_so_its_output_stays_readable() {
        let p = provider(vec![row(2, "2: cargo build (exit 101)", false)]);
        assert_eq!(p.list(&vfs_root().join("pty")).unwrap().len(), 1);
    }

    #[test]
    fn a_session_directory_holds_a_console_leaf_that_attaches_to_it() {
        let p = provider(vec![row(3, "3: zsh", true)]);
        let got = p.list(&vfs_root().join("pty").join("3")).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].name, "console");
        assert!(!got[0].is_dir);
        let a = got[0].activation.as_ref().expect("console must activate");
        assert_eq!(a.extension, "pty");
        assert_eq!(a.intent, "attach");
        assert_eq!(a.data.as_int(), Some(3));
    }

    #[test]
    fn an_unknown_session_is_not_found_rather_than_empty() {
        let p = provider(vec![row(1, "1: zsh", true)]);
        assert!(p.list(&vfs_root().join("pty").join("9")).is_err());
        assert!(p.list(&vfs_root().join("pty").join("bogus")).is_err());
    }
}
