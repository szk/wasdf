//! The live session map, and the read-only snapshot the VFS provider sees.

use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

use super::session::{Origin, Session, SessionId};

/// What `@/pty` needs to know about a session. A snapshot rather than a
/// borrow: [`super::provider::PtyProvider`] lists from the async task thread,
/// while the sessions themselves are `!Sync` and live on the UI thread.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionSummary {
    pub id: SessionId,
    /// Already rendered, e.g. `"3: rg foo (exit 1)"`.
    pub label: String,
    pub running: bool,
}

/// Every live session, plus the shared snapshot the provider reads.
#[derive(Default)]
pub struct SessionTable {
    sessions: BTreeMap<SessionId, Session>,
    next_id: SessionId,
    snapshot: Arc<RwLock<Vec<SessionSummary>>>,
}

impl SessionTable {
    /// The snapshot handle to hand [`super::provider::PtyProvider`].
    pub fn snapshot(&self) -> Arc<RwLock<Vec<SessionSummary>>> {
        self.snapshot.clone()
    }

    /// The next never-before-used id.
    pub fn next_id(&mut self) -> SessionId {
        self.next_id += 1;
        self.next_id
    }

    pub fn insert(&mut self, s: Session) {
        self.sessions.insert(s.id, s);
        self.publish();
    }

    pub fn get(&self, id: SessionId) -> Option<&Session> {
        self.sessions.get(&id)
    }

    pub fn get_mut(&mut self, id: SessionId) -> Option<&mut Session> {
        self.sessions.get_mut(&id)
    }

    pub fn remove(&mut self, id: SessionId) -> Option<Session> {
        let s = self.sessions.remove(&id);
        self.publish();
        s
    }

    pub fn ids(&self) -> Vec<SessionId> {
        self.sessions.keys().copied().collect()
    }

    /// The newest still-running session of a given origin. `x` reuses the shell
    /// it last opened rather than stacking a new one on every press; `X` is the
    /// key that always makes a new one.
    pub fn newest_running(&self, origin: &Origin) -> Option<SessionId> {
        self.sessions
            .values()
            .rev()
            .find(|s| s.running() && &s.origin == origin)
            .map(|s| s.id)
    }

    pub fn running_count(&self) -> usize {
        self.sessions.values().filter(|s| s.running()).count()
    }

    /// The id after (or before) `from` in id order, wrapping. `None` when there
    /// is nothing else to move to.
    pub fn neighbour(&self, from: Option<SessionId>, forward: bool) -> Option<SessionId> {
        let ids = self.ids();
        if ids.is_empty() {
            return None;
        }
        let Some(cur) = from.and_then(|c| ids.iter().position(|&i| i == c)) else {
            return ids.first().copied();
        };
        let n = ids.len();
        let next = if forward { (cur + 1) % n } else { (cur + n - 1) % n };
        ids.get(next).copied()
    }

    /// Drop exited sessions past `keep`, oldest first. Running sessions are
    /// never evicted — a cap on consoles must not become a way to lose a job.
    pub fn evict_exited(&mut self, keep: usize) {
        let mut dead: Vec<SessionId> =
            self.sessions.values().filter(|s| !s.running()).map(|s| s.id).collect();
        if dead.len() <= keep {
            return;
        }
        dead.truncate(dead.len() - keep);
        for id in dead {
            self.sessions.remove(&id);
        }
        self.publish();
    }

    /// Re-publish the snapshot. Called on every structural change, and by the
    /// extension each frame, since `running` flips on a background thread with
    /// nothing to notify the table.
    pub fn publish(&self) {
        let rows: Vec<SessionSummary> = self
            .sessions
            .values()
            .map(|s| SessionSummary {
                id: s.id,
                label: format!("{}: {}{}", s.id, s.label, s.exit_label()),
                running: s.running(),
            })
            .collect();
        if let Ok(mut w) = self.snapshot.write() {
            *w = rows;
        }
    }

    /// Sessions whose child has exited since the last sweep, as
    /// `(id, label, success)` — the extension turns these into notices and, for
    /// an `Origin::Edit`, a Refresh. Each session is reported exactly once.
    pub fn take_newly_exited(&mut self, reported: &mut Vec<SessionId>) -> Vec<(SessionId, String, bool, Origin)> {
        let mut out = Vec::new();
        for s in self.sessions.values() {
            if s.running() || reported.contains(&s.id) {
                continue;
            }
            reported.push(s.id);
            let ok = s
                .shared
                .exit
                .lock()
                .ok()
                .and_then(|g| g.clone())
                .map(|e| e.success())
                .unwrap_or(false);
            out.push((s.id, s.label.clone(), ok, s.origin.clone()));
        }
        out
    }
}
