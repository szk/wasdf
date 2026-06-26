//! TaskManager: runs plans on worker threads and returns AsyncResults. At most
//! one live task per unique purpose (content, search, refresh); a new spawn
//! cancels the previous one. Handlers never hold task ids.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Mutex;

use crate::core::{
    AppEvent, AsyncResult, AsyncStatus, Entry, ExecOutput, Payload, Plan, Purpose, ReadResult,
};
use crate::script::SchemeSession;
use crate::services::{MatcherBackend, ResolverChain, rank_paths};

/// Bytes read for file content, bounded so huge files stay responsive.
const READ_LIMIT: usize = 64 * 1024;

pub struct TaskManager {
    tx: Sender<AppEvent>,
    resolver: Arc<ResolverChain>,
    matcher: Arc<dyn MatcherBackend>,
    scheme: Option<Arc<SchemeSession>>,
    next_id: AtomicU64,
    cancels: Mutex<HashMap<Purpose, Arc<AtomicBool>>>,
    /// Cancel flag of the most recent Execute, so KillProcess can stop it.
    execute_cancel: Mutex<Option<Arc<AtomicBool>>>,
}

impl TaskManager {
    pub fn new(
        tx: Sender<AppEvent>,
        resolver: Arc<ResolverChain>,
        matcher: Arc<dyn MatcherBackend>,
        scheme: Option<Arc<SchemeSession>>,
    ) -> Self {
        TaskManager {
            tx,
            resolver,
            matcher,
            scheme,
            next_id: AtomicU64::new(1),
            cancels: Mutex::new(HashMap::new()),
            execute_cancel: Mutex::new(None),
        }
    }

    /// Kill the currently streaming Execute process, if any (KillProcess).
    pub fn kill_execute(&self) {
        if let Some(flag) = self.execute_cancel.lock().unwrap().as_ref() {
            flag.store(true, Ordering::Relaxed);
        }
    }

    /// Spawn a plan. `cwd` resolves relative destinations; `generation` tags the
    /// result for staleness. Suspend is handled by the caller, not here. Execute
    /// streams output incrementally; every other plan returns once.
    pub fn spawn(&self, plan: Plan, generation: u64, cwd: PathBuf) {
        if let Plan::Execute { argv } = plan {
            self.spawn_execute(argv, generation, cwd);
            return;
        }

        let purpose = plan.purpose();
        let request_id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let cancel = self.install_cancel(purpose);

        let tx = self.tx.clone();
        let resolver = self.resolver.clone();
        let matcher = self.matcher.clone();
        let scheme = self.scheme.clone();

        std::thread::spawn(move || {
            let (status, payload) = run_plan(&plan, &resolver, matcher.as_ref(), scheme.as_deref(), &cwd);
            let status = if cancel.load(Ordering::Relaxed) {
                AsyncStatus::Cancelled
            } else {
                status
            };
            let _ = tx.send(AppEvent::Async(AsyncResult {
                request_id,
                purpose,
                mode_generation: generation,
                status,
                payload,
            }));
        });
    }

    /// Run an Execute command, streaming its output into the Exec frame as it
    /// arrives (one AsyncResult per stdout line, then a final result on exit).
    fn spawn_execute(&self, argv: Vec<String>, generation: u64, cwd: PathBuf) {
        let request_id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let cancel = Arc::new(AtomicBool::new(false));
        *self.execute_cancel.lock().unwrap() = Some(cancel.clone());
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            stream_execute(&argv, &cwd, &tx, request_id, generation, &cancel);
        });
    }

    /// For unique purposes, cancel any previous live task and return a fresh
    /// cancel flag. For non-unique purposes, return a standalone flag.
    fn install_cancel(&self, purpose: Purpose) -> Arc<AtomicBool> {
        let unique = matches!(purpose, Purpose::Content | Purpose::Search | Purpose::Refresh);
        let flag = Arc::new(AtomicBool::new(false));
        if unique {
            let mut map = self.cancels.lock().unwrap();
            if let Some(prev) = map.insert(purpose, flag.clone()) {
                prev.store(true, Ordering::Relaxed);
            }
        }
        flag
    }
}

fn run_plan(
    plan: &Plan,
    resolver: &ResolverChain,
    matcher: &dyn MatcherBackend,
    scheme: Option<&SchemeSession>,
    cwd: &std::path::Path,
) -> (AsyncStatus, Payload) {
    match plan {
        Plan::ReadDir { path, show_hidden } => match crate::fs::read_directory(path, *show_hidden) {
            Ok(entries) => (AsyncStatus::Ok, Payload::Entries { path: path.clone(), entries }),
            Err(e) => (AsyncStatus::Failed(format!("read {}: {e}", path.display())), Payload::None),
        },
        Plan::Search { root, query, show_hidden } => {
            let walked = crate::fs::walk(root, *show_hidden, 20_000);
            let ranked = rank_paths(matcher, root, walked, query);
            let entries: Vec<Entry> = ranked
                .into_iter()
                .map(|p| {
                    let name = p
                        .strip_prefix(root)
                        .unwrap_or(&p)
                        .to_string_lossy()
                        .into_owned();
                    crate::fs::make_entry(p, name)
                })
                .collect();
            (AsyncStatus::Ok, Payload::Entries { path: root.clone(), entries })
        }
        Plan::Read { owner, path, offset } => read_content(owner, path, *offset),
        Plan::ResolveAndRun { request } => match resolver.resolve(request, cwd) {
            Ok(cmd) => {
                let done = crate::exec::run_background(&cmd.label, &cmd.argv, cwd);
                (AsyncStatus::Ok, Payload::OpDone(done))
            }
            Err(e) => (AsyncStatus::Failed(e), Payload::None),
        },
        Plan::Execute { argv } => {
            let out = crate::exec::run_captured(argv, cwd);
            (AsyncStatus::Ok, Payload::Exec(out))
        }
        Plan::Suspend { .. } => (AsyncStatus::Cancelled, Payload::None),
        Plan::EvalScheme { expr } => match scheme {
            Some(session) => match session.eval_value(expr) {
                Ok(value) => (AsyncStatus::Ok, Payload::Scheme(value)),
                Err(e) => (AsyncStatus::Failed(e), Payload::None),
            },
            None => (AsyncStatus::Ok, Payload::Scheme(crate::core::ExtensionValue::Nil)),
        },
    }
}

/// Stream an Execute command's stdout line-by-line into the Exec frame, append
/// stderr at the end, and send a final result with the exit code. KillProcess
/// sets the cancel flag; the reader stops and kills the child.
fn stream_execute(
    argv: &[String],
    cwd: &std::path::Path,
    tx: &Sender<AppEvent>,
    request_id: u64,
    generation: u64,
    cancel: &AtomicBool,
) {
    let send = |lines: Vec<String>, finished: bool, exit: Option<i32>| {
        let _ = tx.send(AppEvent::Async(AsyncResult {
            request_id,
            purpose: Purpose::Execute,
            mode_generation: generation,
            status: AsyncStatus::Ok,
            payload: Payload::Exec(ExecOutput { lines, finished, exit }),
        }));
    };

    if argv.is_empty() {
        send(vec!["empty command".into()], true, Some(1));
        return;
    }
    let mut child = match Command::new(&argv[0])
        .args(&argv[1..])
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            send(vec![format!("spawn failed: {e}")], true, Some(1));
            return;
        }
    };

    let mut lines: Vec<String> = Vec::new();
    if let Some(stdout) = child.stdout.take() {
        for line in BufReader::new(stdout).lines() {
            if cancel.load(Ordering::Relaxed) {
                let _ = child.kill();
                break;
            }
            match line {
                Ok(l) => {
                    lines.push(l);
                    send(lines.clone(), false, None);
                }
                Err(_) => break,
            }
        }
    }

    // Drain stderr after stdout EOF, then report completion with the exit code.
    if let Some(mut stderr) = child.stderr.take() {
        let mut buf = String::new();
        let _ = stderr.read_to_string(&mut buf);
        for l in buf.lines() {
            lines.push(l.to_string());
        }
    }
    let exit = child.wait().ok().and_then(|s| s.code());
    send(lines, true, exit);
}

/// Read `path` for an extension's content: a directory's entries, or one bounded
/// byte chunk from `offset` (the owning extension pages further chunks by
/// re-issuing with a new offset). The extension decodes the result (MIME,
/// highlight, image, hexdump) on the main thread; core stays content-agnostic.
fn read_content(owner: &str, path: &std::path::Path, offset: u64) -> (AsyncStatus, Payload) {
    let result = if path.is_dir() {
        match crate::fs::read_directory(path, true) {
            Ok(entries) => ReadResult::Dir { entries },
            Err(e) => return (AsyncStatus::Failed(format!("{e}")), Payload::None),
        }
    } else {
        match wasdf_sdk::read_chunk_at(path, offset, READ_LIMIT) {
            Ok((bytes, eof)) => ReadResult::Bytes { offset, bytes, eof },
            Err(e) => {
                return (AsyncStatus::Failed(format!("read {}: {e}", path.display())), Payload::None)
            }
        }
    };
    (AsyncStatus::Ok, Payload::Read { owner: owner.to_string(), path: path.to_path_buf(), result })
}

