//! The resident Scheme session (pillar #3). A dedicated OS thread owns a
//! steel Engine; the kernel talks to it via mpsc channels. The engine is
//! `!Send`/`!Sync`, so it cannot leave its thread — the session holds only
//! `Sender` and `Condvar`, both of which are `Send + Sync`, allowing
//! `Arc<SchemeSession>` to be shared with worker threads safely.
//!
//! `.scm` configuration files are pure `(quote (...))` data; they require no
//! R7RS-specific features and evaluate without issue on steel. Dynamic
//! evaluation (`Plan::EvalScheme`) also works with steel's built-ins.

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use steel::steel_vm::engine::Engine;
use steel::SteelVal;

use crate::core::ExtensionValue;

// Generous: covers Engine::new() stdlib load under heavy parallel load.
// Steady-state evals return in microseconds.
const EVAL_TIMEOUT: Duration = Duration::from_secs(20);

type Reply = Sender<Result<String, String>>;

/// The resident Scheme session. Calls are serialized via `call_lock`; the
/// engine thread is single-consumer so no additional correlation is needed.
pub struct SchemeSession {
    req: Sender<(String, Reply)>,
    ready: Arc<(Mutex<bool>, Condvar)>,
    call_lock: Mutex<()>,
}

impl SchemeSession {
    /// Spawn the session. `Engine::new()` (stdlib load) happens on the engine
    /// thread, so spawning never blocks the caller.
    pub fn spawn() -> Arc<Self> {
        let (req_tx, req_rx) = mpsc::channel::<(String, Reply)>();
        let ready = Arc::new((Mutex::new(false), Condvar::new()));
        let r2 = ready.clone();
        std::thread::Builder::new()
            .name("wasdf-scheme".into())
            .spawn(move || engine_loop(req_rx, r2))
            .expect("spawn scheme thread");
        Arc::new(Self { req: req_tx, ready, call_lock: Mutex::new(()) })
    }

    /// Block until the engine is ready to evaluate, up to `timeout`. Returns
    /// whether it became ready (false = stdlib load stalled or thread died).
    pub fn wait_ready(&self, timeout: Duration) -> bool {
        let (lock, cvar) = &*self.ready;
        let guard = lock.lock().unwrap();
        let (guard, _) = cvar.wait_timeout_while(guard, timeout, |r| !*r).unwrap();
        *guard
    }

    /// Evaluate one expression, returning the printed result (or an error).
    pub fn eval(&self, expr: &str) -> Result<String, String> {
        let _guard = self.call_lock.lock().unwrap();
        let (tx, rx) = mpsc::channel();
        self.req
            .send((expr.to_string(), tx))
            .map_err(|_| "scheme: thread gone".to_string())?;
        rx.recv_timeout(EVAL_TIMEOUT)
            .unwrap_or_else(|_| Err("scheme: evaluation timed out".into()))
    }

    /// Evaluate to a structural value for the SchemeValue payload.
    pub fn eval_value(&self, expr: &str) -> Result<ExtensionValue, String> {
        self.eval(expr).map(|s| parse_value(&s))
    }
}

/// Engine thread. Owns the `Engine` (which is `!Send`) for its lifetime,
/// signals readiness after stdlib load, then processes requests FIFO.
fn engine_loop(req: Receiver<(String, Reply)>, ready: Arc<(Mutex<bool>, Condvar)>) {
    let mut engine = Engine::new();
    {
        let (lock, cvar) = &*ready;
        *lock.lock().unwrap() = true;
        cvar.notify_all();
    }
    while let Ok((src, reply)) = req.recv() {
        let res = engine
            .run(src)
            .map_err(|e| format!("scheme: {e}"))
            .map(|vals| steelval_to_write_string(vals.last().unwrap_or(&SteelVal::Void)));
        let _ = reply.send(res);
    }
}

/// Convert a `SteelVal` to canonical Scheme `write` format, matching what
/// `sexpr::parse` expects:
///   - symbol  → bare word
///   - string  → `"..."` with `\\ \" \n \t` escaping
///   - integer → decimal digits
///   - bool    → `#t` / `#f`
///   - list    → `(a b c)` recursively
///   - void    → `""` (empty → Nil in parse_value)
fn steelval_to_write_string(val: &SteelVal) -> String {
    match val {
        SteelVal::BoolV(b) => if *b { "#t" } else { "#f" }.to_string(),
        SteelVal::IntV(i) => i.to_string(),
        SteelVal::BigNum(n) => n.to_string(),
        SteelVal::NumV(f) => {
            // Only integers appear in the config; round-trip as integer if lossless.
            let i = *f as i64;
            if i as f64 == *f { i.to_string() } else { f.to_string() }
        }
        SteelVal::StringV(s) => {
            let mut out = String::with_capacity(s.len() + 2);
            out.push('"');
            for c in s.as_str().chars() {
                match c {
                    '"' => out.push_str("\\\""),
                    '\\' => out.push_str("\\\\"),
                    '\n' => out.push_str("\\n"),
                    '\t' => out.push_str("\\t"),
                    other => out.push(other),
                }
            }
            out.push('"');
            out
        }
        SteelVal::SymbolV(s) => s.as_str().to_string(),
        SteelVal::ListV(list) => {
            let mut out = String::from("(");
            let mut first = true;
            for item in list.iter() {
                if !first {
                    out.push(' ');
                }
                first = false;
                out.push_str(&steelval_to_write_string(item));
            }
            out.push(')');
            out
        }
        SteelVal::Void => String::new(),
        other => {
            // Unexpected type: fall back to steel's own Display (with external=true
            // this quotes strings, so the worst case is a type we don't recognise
            // surfaces as an opaque string in the error path).
            format!("{other}")
        }
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

    fn ready(session: &SchemeSession) {
        assert!(session.wait_ready(Duration::from_secs(30)), "engine did not become ready");
    }

    #[test]
    fn evaluates_basic_expressions() {
        let session = SchemeSession::spawn();
        ready(&session);
        assert_eq!(session.eval("(+ 1 2)").unwrap(), "3");
        assert_eq!(session.eval("(string-append \"a\" \"b\")").unwrap(), "\"ab\"");
        assert_eq!(session.eval("(if (< 1 2) 'yes 'no)").unwrap(), "yes");
    }

    #[test]
    fn errors_do_not_kill_the_session() {
        let session = SchemeSession::spawn();
        ready(&session);
        let err = session.eval("(error \"boom\")");
        assert!(err.is_err(), "raised error surfaces as Err, got {err:?}");
        // Engine stays alive and usable after an error.
        assert_eq!(session.eval("(* 6 7)").unwrap(), "42");
    }

    #[test]
    fn eval_value_parses_scalars() {
        let session = SchemeSession::spawn();
        ready(&session);
        assert_eq!(session.eval_value("(= 1 1)").unwrap(), ExtensionValue::Bool(true));
        assert_eq!(session.eval_value("(+ 20 22)").unwrap(), ExtensionValue::Int(42));
    }

    #[test]
    fn quote_list_round_trips_through_sexpr() {
        let session = SchemeSession::spawn();
        ready(&session);
        // The canonical smoke test: a nested list with all datum types.
        let result = session.eval("(quote (a \"b\" 1 #t (c)))").unwrap();
        assert_eq!(result, "(a \"b\" 1 #t (c))");
    }
}
