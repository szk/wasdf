//! The diagnostic message log behind `@/messages`: wasdf-generated warnings
//! and errors (a rejected user-config value, a skipped optional extension, a
//! preset name collision, …) that would otherwise be `eprintln!`ed into the
//! TUI's alternate screen and lost.
//!
//! The log is process-global (`global()`), not an `Arc` threaded through
//! constructors: its producers include the extension loader and the theme /
//! lscolors registries, which run before — or entirely independent of — any
//! owner that could hand them a handle. A global sink is the same shape the
//! `log` crate ecosystem settled on for exactly this reason. `MessageLog`
//! itself stays a plain testable struct; the global is just one instance.

use std::collections::VecDeque;
use std::io;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

use crate::core::{Entry, OptionStore};
use crate::vfs::{vfs_root, VfsProvider};

/// `max-message-line`'s default, mirrored in `app/options.scm` (defaults must
/// stay byte-identical — see the header there).
pub const DEFAULT_MAX_LINES: usize = 200;

// ── MessageLog ────────────────────────────────────────────────────────────────

/// A bounded FIFO of diagnostic lines — `(text, error)`, the same pair a
/// `Notice` carries, so surfacing a line as a notice loses nothing. Pushing
/// past the cap drops the oldest.
pub struct MessageLog {
    inner: Mutex<LogState>,
}

struct LogState {
    lines: VecDeque<(String, bool)>,
    max_lines: usize,
    /// Total pushes ever (never decremented by trimming) — the cursor
    /// `latest_since` compares against, so the kernel can surface new lines
    /// as notices without re-surfacing ones it has already shown.
    seq: u64,
}

impl Default for MessageLog {
    fn default() -> Self {
        MessageLog {
            inner: Mutex::new(LogState {
                lines: VecDeque::new(),
                max_lines: DEFAULT_MAX_LINES,
                seq: 0,
            }),
        }
    }
}

impl MessageLog {
    /// Append one diagnostic line, dropping the oldest beyond the cap.
    pub fn push(&self, msg: impl Into<String>, error: bool) {
        let mut s = self.inner.lock().unwrap();
        s.lines.push_back((msg.into(), error));
        s.seq += 1;
        let max = s.max_lines;
        while s.lines.len() > max {
            s.lines.pop_front();
        }
    }

    /// The current push counter — capture before a batch of work, then hand
    /// to `latest_since` to ask "anything new since?".
    pub fn seq(&self) -> u64 {
        self.inner.lock().unwrap().seq
    }

    /// The newest line pushed after cursor `seen`, plus the new cursor —
    /// `None` when nothing new. Only the newest: the notice UI is a single
    /// 4-second slot, so surfacing an intermediate line would just be
    /// overwritten before it could be read; the full history stays in
    /// `@/messages`.
    pub fn latest_since(&self, seen: u64) -> Option<(String, bool, u64)> {
        let s = self.inner.lock().unwrap();
        if s.seq > seen {
            s.lines.back().map(|(l, e)| (l.clone(), *e, s.seq))
        } else {
            None
        }
    }

    /// Set the retention cap (`@/settings/System/max-message-line`), trimming
    /// oldest-first immediately if the log already exceeds it.
    pub fn set_max_lines(&self, max: usize) {
        let mut s = self.inner.lock().unwrap();
        s.max_lines = max.max(1);
        while s.lines.len() > s.max_lines {
            s.lines.pop_front();
        }
    }

    /// A snapshot of the retained `(text, error)` lines, oldest first.
    pub fn lines(&self) -> Vec<(String, bool)> {
        self.inner.lock().unwrap().lines.iter().cloned().collect()
    }
}

/// The process-wide log every diagnostic site writes to (via [`log`]).
pub fn global() -> &'static Arc<MessageLog> {
    static GLOBAL: OnceLock<Arc<MessageLog>> = OnceLock::new();
    GLOBAL.get_or_init(|| Arc::new(MessageLog::default()))
}

/// Record one message — **the single entry point for everything wasdf tells
/// the user**: completion notices ("copy done", error = false) and
/// diagnostics (a rejected config value, error = true) alike. Replaces
/// `eprintln!` everywhere the TUI may hold the alternate screen (which is
/// everywhere past `main`'s first line — boot runs after `terminal::init`),
/// where stderr output is invisible and corrupts the display. The kernel's
/// per-frame pump surfaces the newest line as the on-screen notice; the full
/// history stays under `@/messages`.
pub fn log(msg: impl Into<String>, error: bool) {
    global().push(msg, error);
}

// ── MessagesProvider ──────────────────────────────────────────────────────────

/// Serves `@/messages`: one virtual leaf per retained line, **newest first** —
/// the panel cursor lands on the first row, and the most recent diagnostic is
/// the one the user came to read. Leaves carry no activation; there is nothing
/// to do with a message but read it.
pub struct MessagesProvider {
    log: Arc<MessageLog>,
    store: Arc<OptionStore>,
}

impl MessagesProvider {
    pub fn new(log: Arc<MessageLog>, store: Arc<OptionStore>) -> Self {
        MessagesProvider { log, store }
    }
}

impl VfsProvider for MessagesProvider {
    fn mount(&self) -> &str {
        "messages"
    }

    fn list(&self, _path: &Path) -> io::Result<Vec<Entry>> {
        // Re-read the cap on every listing so an edit under @/settings/System
        // applies by the next view, with no settings-side propagation wiring.
        self.log.set_max_lines(self.store.get_u32("max-message-line") as usize);
        let base = vfs_root().join(self.mount());
        Ok(self
            .log
            .lines()
            .into_iter()
            .rev()
            .enumerate()
            .map(|(i, (msg, _))| Entry::virtual_leaf(base.join(i.to_string()), msg))
            .collect())
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{OptionDef, OptionKind};

    #[test]
    fn push_beyond_the_cap_drops_the_oldest_lines() {
        let log = MessageLog::default();
        log.set_max_lines(2);
        log.push("one", true);
        log.push("two", true);
        log.push("three", true);
        let texts: Vec<String> = log.lines().into_iter().map(|(t, _)| t).collect();
        assert_eq!(texts, vec!["two".to_string(), "three".to_string()]);
    }

    #[test]
    fn latest_since_returns_only_the_newest_unseen_line_then_goes_quiet() {
        let log = MessageLog::default();
        let seen = log.seq();
        assert!(log.latest_since(seen).is_none(), "nothing pushed yet");

        log.push("first", false);
        log.push("second", true);
        let (line, error, seq) = log.latest_since(seen).expect("two unseen lines");
        assert_eq!(line, "second", "only the newest — the notice UI is a single slot");
        assert!(error, "the error attribute rides along for notice styling");
        assert!(log.latest_since(seq).is_none(), "advancing the cursor consumes them");

        log.push("third", false);
        let (line, error, _) = log.latest_since(seq).expect("a fresh line after the cursor");
        assert_eq!(line, "third");
        assert!(!error);
    }

    #[test]
    fn lowering_the_cap_trims_immediately() {
        let log = MessageLog::default();
        log.push("one", true);
        log.push("two", true);
        log.push("three", true);
        log.set_max_lines(1);
        assert_eq!(log.lines(), vec![("three".to_string(), true)]);
    }

    fn store_with_cap(default: &str) -> Arc<OptionStore> {
        Arc::new(OptionStore::new(vec![OptionDef {
            key: "max-message-line".into(),
            category: "System".into(),
            kind: OptionKind::Int { min: 1, max: 100000 },
            default: crate::core::OptionValue::Int(default.parse().unwrap()),
            label: "max-message-line".into(),
            owner: None,
        }]))
    }

    #[test]
    fn provider_lists_lines_newest_first_as_leaves() {
        let log = Arc::new(MessageLog::default());
        log.push("older warning", true);
        log.push("newer error", true);
        let provider = MessagesProvider::new(log, store_with_cap("200"));
        let entries = provider.list(&vfs_root().join("messages")).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name, "newer error", "the most recent diagnostic comes first");
        assert_eq!(entries[1].name, "older warning");
        assert_eq!(entries[0].path, vfs_root().join("messages").join("0"));
        assert!(!entries[0].is_dir);
        assert!(entries[0].activation.is_none(), "a message is read, not activated");
    }

    #[test]
    fn provider_applies_the_stores_cap_on_every_listing() {
        let log = Arc::new(MessageLog::default());
        for i in 0..5 {
            log.push(format!("m{i}"), true);
        }
        let provider = MessagesProvider::new(log, store_with_cap("2"));
        let entries = provider.list(&vfs_root().join("messages")).unwrap();
        assert_eq!(entries.len(), 2, "the store's max-message-line wins over the log's default");
        assert_eq!(entries[0].name, "m4");
        assert_eq!(entries[1].name, "m3");
    }
}
