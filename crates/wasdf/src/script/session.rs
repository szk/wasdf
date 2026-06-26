//! The resident Scheme REPL session (pillar #3). A dedicated OS thread owns a
//! stak R7RS virtual machine running an eval loop; the kernel talks to it over
//! a byte pipe. This is the genuine stak interpreter — not a one-shot VM — used
//! to parse the embedded configuration and for the EvalScheme plan (dynamic
//! `:when` and expression-valued intent args).
//!
//! The REPL bootstrap is kept as Scheme *source* and compiled to bytecode at
//! runtime (no build-time/embedded bytecode). Because stak's compiler is itself
//! a Scheme program on the VM, that compile is costly, so the result is cached
//! on disk under the user config dir (keyed by the source hash) and reused — the
//! spec's bytecode cache. The source remains authoritative; editing it
//! recompiles once.

use std::collections::VecDeque;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::Duration;

use stak::device::Device;
use stak::file::VoidFileSystem;
use stak::process_context::OsProcessContext;
use stak::r7rs::SmallPrimitiveSet;
use stak::time::VoidClock;
use stak::vm::Vm;

use crate::core::ExtensionValue;

/// The resident REPL loop, kept as Scheme *source* and compiled at runtime when
/// the session spawns (no embedded/precompiled bytecode). It reads one datum,
/// evaluates it in a fixed import environment, writes the result, then a NUL
/// delimiter, and loops; errors are guarded to `#err`.
const REPL_SOURCE: &str =
    "(import (scheme base) (scheme eval) (scheme read) (scheme repl) (scheme write)
             (scheme process-context))
     (define env (environment
                   '(scheme base) '(scheme write) '(scheme process-context)))
     (let loop ()
       (let ((expr (read)))
         (if (eof-object? expr)
             #t
             (begin
               (guard (e (#t (write-string \"#err: \")
                             (if (error-object? e)
                                 (write-string (error-object-message e))
                                 (write e))))
                 (write (eval expr env)))
               (write-char (integer->char 0))
               (flush-output-port)
               (loop)))))";

const HEAP_SIZE: usize = 1 << 20;
// Generous: covers the REPL's one-time environment initialization (which runs
// after readiness is signalled) under heavy parallel load. Steady-state evals
// return in microseconds.
const EVAL_TIMEOUT: Duration = Duration::from_secs(20);

/// A blocking byte pipe between the kernel and the VM thread.
struct Pipe {
    buf: Mutex<(VecDeque<u8>, bool)>, // (bytes, closed)
    cv: Condvar,
}

impl Pipe {
    fn new() -> Self {
        Pipe { buf: Mutex::new((VecDeque::new(), false)), cv: Condvar::new() }
    }

    fn push(&self, byte: u8) {
        let mut g = self.buf.lock().unwrap();
        g.0.push_back(byte);
        self.cv.notify_all();
    }

    fn push_bytes(&self, bytes: &[u8]) {
        let mut g = self.buf.lock().unwrap();
        g.0.extend(bytes.iter().copied());
        self.cv.notify_all();
    }

    /// Block until a byte is available; None when the pipe is closed and empty.
    fn pop_blocking(&self) -> Option<u8> {
        let mut g = self.buf.lock().unwrap();
        loop {
            if let Some(b) = g.0.pop_front() {
                return Some(b);
            }
            if g.1 {
                return None;
            }
            g = self.cv.wait(g).unwrap();
        }
    }

    /// Pop one byte, waiting at most `timeout`. None on timeout or close.
    fn pop_timeout(&self, timeout: Duration) -> Option<u8> {
        let mut g = self.buf.lock().unwrap();
        loop {
            if let Some(b) = g.0.pop_front() {
                return Some(b);
            }
            if g.1 {
                return None;
            }
            let (next, res) = self.cv.wait_timeout(g, timeout).unwrap();
            if res.timed_out() {
                return None;
            }
            g = next;
        }
    }

    fn drain(&self) {
        self.buf.lock().unwrap().0.clear();
    }
}

/// The stak Device backed by the kernel pipes.
struct PipeDevice {
    input: Arc<Pipe>,
    output: Arc<Pipe>,
}

impl Device for PipeDevice {
    type Error = std::convert::Infallible;

    fn read(&mut self) -> Result<Option<u8>, Self::Error> {
        Ok(self.input.pop_blocking())
    }

    fn write(&mut self, byte: u8) -> Result<(), Self::Error> {
        self.output.push(byte);
        Ok(())
    }

    fn write_error(&mut self, _byte: u8) -> Result<(), Self::Error> {
        Ok(())
    }
}

/// The resident Scheme session. Calls are serialized; the line-oriented REPL
/// needs no correlation ids beyond that.
pub struct SchemeSession {
    input: Arc<Pipe>,
    output: Arc<Pipe>,
    call_lock: Mutex<()>,
    /// Set true once the VM is running the REPL (compile/load done). Lets boot
    /// fall back to native config without freezing on a cold cache compile.
    ready: Arc<(Mutex<bool>, Condvar)>,
}

impl SchemeSession {
    /// Spawn the session. Compiling-from-source (cold cache) or loading the
    /// cached bytecode happens on the VM thread, so spawning never blocks boot.
    pub fn spawn() -> Arc<Self> {
        let input = Arc::new(Pipe::new());
        let output = Arc::new(Pipe::new());
        let ready = Arc::new((Mutex::new(false), Condvar::new()));
        let (i2, o2, r2) = (input.clone(), output.clone(), ready.clone());
        std::thread::Builder::new()
            .name("wasdf-scheme".into())
            .spawn(move || run_vm(PipeDevice { input: i2, output: o2 }, r2))
            .expect("spawn scheme thread");
        Arc::new(SchemeSession { input, output, call_lock: Mutex::new(()), ready })
    }

    /// Block until the REPL is ready to evaluate, up to `timeout`. Returns
    /// whether it became ready.
    pub fn wait_ready(&self, timeout: Duration) -> bool {
        let (lock, cvar) = &*self.ready;
        let guard = lock.lock().unwrap();
        let (guard, _) = cvar.wait_timeout_while(guard, timeout, |ready| !*ready).unwrap();
        *guard
    }

    /// Evaluate one expression, returning the printed result (or an error).
    pub fn eval(&self, expr: &str) -> Result<String, String> {
        let _guard = self.call_lock.lock().unwrap();
        self.output.drain();
        self.input.push_bytes(expr.as_bytes());
        self.input.push(b'\n');

        let mut out = Vec::new();
        loop {
            match self.output.pop_timeout(EVAL_TIMEOUT) {
                Some(0) => break,
                Some(b) => out.push(b),
                None => return Err("scheme: evaluation timed out".into()),
            }
        }
        let s = String::from_utf8_lossy(&out).trim().to_string();
        if let Some(msg) = s.strip_prefix("#err:") {
            Err(format!("scheme: {}", msg.trim()))
        } else {
            Ok(s)
        }
    }

    /// Evaluate to a structural value for the SchemeValue payload.
    pub fn eval_value(&self, expr: &str) -> Result<ExtensionValue, String> {
        self.eval(expr).map(|s| parse_value(&s))
    }
}

fn run_vm(device: PipeDevice, ready: Arc<(Mutex<bool>, Condvar)>) {
    // Compile-from-source (cold) or load the cached bytecode on this thread.
    let Some(bytecode) = repl_bytecode() else {
        return; // compilation failed; evals time out and the session disables.
    };
    let mut heap: Vec<stak::vm::Value> = vec![Default::default(); HEAP_SIZE];
    let primitives = SmallPrimitiveSet::new(
        device,
        VoidFileSystem::new(),
        OsProcessContext::new(),
        VoidClock::new(),
    );
    let mut vm = match Vm::new(heap.as_mut_slice(), primitives) {
        Ok(vm) => vm,
        Err(_) => return, // session unavailable; evals will time out and disable.
    };
    // The REPL is about to start reading: signal readiness.
    {
        let (lock, cvar) = &*ready;
        *lock.lock().unwrap() = true;
        cvar.notify_all();
    }
    let _ = vm.run(bytecode.iter().copied());
}

/// The REPL bytecode, compiled from REPL_SOURCE at most once per process and
/// cached on disk between runs (the spec's bytecode cache). The source remains
/// authoritative; the cache key is its hash, so editing the source recompiles.
fn repl_bytecode() -> Option<Vec<u8>> {
    static CACHE: OnceLock<Option<Vec<u8>>> = OnceLock::new();
    CACHE.get_or_init(compile_or_load).clone()
}

fn compile_or_load() -> Option<Vec<u8>> {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    REPL_SOURCE.hash(&mut hasher);
    let cache_path = cache_dir().map(|d| d.join(format!("repl-{:016x}.scbc", hasher.finish())));

    if let Some(path) = &cache_path {
        if let Ok(bytes) = std::fs::read(path) {
            if !bytes.is_empty() {
                return Some(bytes);
            }
        }
    }
    let mut bytecode = Vec::new();
    match stak_compiler::compile_r7rs(REPL_SOURCE.as_bytes(), &mut bytecode) {
        Ok(()) => {
            if let Some(path) = &cache_path {
                write_atomic(path, &bytecode);
            }
            Some(bytecode)
        }
        Err(e) => {
            eprintln!("scheme: REPL compile failed: {e:?}");
            None
        }
    }
}

/// The bytecode cache directory: $WASDF_CONFIG_DIR, else $XDG_CONFIG_HOME/wasdf,
/// else ~/.config/wasdf. Created on demand.
fn cache_dir() -> Option<PathBuf> {
    let dir = if let Some(d) = std::env::var_os("WASDF_CONFIG_DIR") {
        PathBuf::from(d)
    } else if let Some(x) = std::env::var_os("XDG_CONFIG_HOME") {
        PathBuf::from(x).join("wasdf")
    } else {
        PathBuf::from(std::env::var_os("HOME")?).join(".config").join("wasdf")
    };
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

/// Write via a unique temp file + rename so concurrent processes never observe
/// a partially written cache file.
fn write_atomic(path: &std::path::Path, bytes: &[u8]) {
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    if std::fs::write(&tmp, bytes).is_ok() {
        let _ = std::fs::rename(&tmp, path);
    }
}

/// Parse a printed Scheme datum into an ExtensionValue (scalars; complex forms
/// are kept as their string rendering).
pub fn parse_value(s: &str) -> ExtensionValue {
    let s = s.trim();
    match s {
        "" => ExtensionValue::Nil,
        "#t" | "#true" => ExtensionValue::Bool(true),
        "#f" | "#false" => ExtensionValue::Bool(false),
        _ if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') => {
            ExtensionValue::String(unescape(&s[1..s.len() - 1]))
        }
        _ => match s.parse::<i64>() {
            Ok(i) => ExtensionValue::Int(i),
            Err(_) => ExtensionValue::String(s.to_string()),
        },
    }
}

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('\\') => out.push('\\'),
                Some('"') => out.push('"'),
                Some(other) => out.push(other),
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Wait for the session (cold-cache compile may take minutes the first time).
    fn ready(session: &SchemeSession) {
        assert!(session.wait_ready(Duration::from_secs(600)), "REPL did not become ready");
    }

    #[test]
    fn resident_repl_evaluates_r7rs() {
        let session = SchemeSession::spawn();
        ready(&session);
        // Many expressions evaluated in sequence on the one resident VM.
        assert_eq!(session.eval("(+ 1 2)").unwrap(), "3");
        assert_eq!(session.eval("(string-append \"a\" \"b\")").unwrap(), "\"ab\"");
        assert_eq!(session.eval("(if (< 1 2) 'yes 'no)").unwrap(), "yes");
    }

    #[test]
    fn errors_do_not_kill_the_session() {
        let session = SchemeSession::spawn();
        ready(&session);
        // A raised error is caught by the REPL's guard, not propagated.
        let err = session.eval("(error \"boom\")");
        assert!(err.is_err(), "raised error surfaces as Err, got {err:?}");
        // The session stays alive and usable afterwards.
        assert_eq!(session.eval("(* 6 7)").unwrap(), "42");
    }

    #[test]
    fn eval_value_parses_scalars() {
        let session = SchemeSession::spawn();
        ready(&session);
        assert_eq!(session.eval_value("(= 1 1)").unwrap(), ExtensionValue::Bool(true));
        assert_eq!(session.eval_value("(+ 20 22)").unwrap(), ExtensionValue::Int(42));
    }
}
