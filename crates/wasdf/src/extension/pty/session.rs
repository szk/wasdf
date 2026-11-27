//! One PTY session: the pseudo-terminal, the child on it, the screen model the
//! reader thread keeps up to date, and the escape policy that says how the user
//! gets the keyboard back.

use std::cell::Cell;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

use portable_pty::{ChildKiller, CommandBuilder, ExitStatus, MasterPty, PtySize, native_pty_system};

use crate::core::{AppEvent, Intent};

use super::kbmode;

/// Per-process, never reused, so a `@/pty/<id>` path stays meaningful for the
/// life of the process even after the session behind it exits.
pub type SessionId = u64;

/// Bytes read from the child per `read` call. The reader wakes the UI at most
/// once per chunk, so this also sets the coarsest repaint granularity.
const READ_CHUNK: usize = 8192;

/// How a session gives the keyboard back. Every session releases on the prefix
/// key — that guarantee lives in core (`InputCapture::release`) and is not this
/// enum's business. What this picks is the *additional* release path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EscapeClass {
    /// Prefix key only: cursor movement goes to the child like everything else.
    /// A shell and an editor both need the whole alphabet.
    PrefixOnly,
    /// Prefix key, plus every file-cursor move. For a command the CLI builder
    /// assembled, whose interaction is at most a `y`/`n` prompt, so giving up
    /// the movement keys costs nothing and puts the file cursor one press away.
    PrefixAndCursor,
}

impl EscapeClass {
    /// The `InputCapture::escape` set this class installs.
    ///
    /// All six navigation intents, not only the four WASD moves: jumping to the
    /// top or bottom of a listing is the same gesture as stepping through it,
    /// and releasing on `s` but swallowing `S` would be arbitrary. `Activate`
    /// is deliberately absent — acting *on* an entry while a command still runs
    /// is a different decision from merely looking away from it.
    pub fn intents(self) -> Vec<Intent> {
        match self {
            EscapeClass::PrefixOnly => Vec::new(),
            EscapeClass::PrefixAndCursor => vec![
                Intent::CursorUp,
                Intent::CursorDown,
                Intent::CursorLeft,
                Intent::CursorRight,
                Intent::CursorTop,
                Intent::CursorBottom,
            ],
        }
    }
}

/// Why the session exists. Drives its `@/pty` label and what happens on exit.
#[derive(Debug, Clone, PartialEq)]
pub enum Origin {
    Shell,
    /// A command the CmdOp builder assembled.
    Command,
    /// The editor `StartEdit` opened on `path`; its exit triggers a Refresh so
    /// the saved file's new size and mtime show up.
    Edit { path: PathBuf },
}

impl Origin {
    pub fn escape_class(&self) -> EscapeClass {
        match self {
            // An editor needs `w` and `s` most of all, so it escapes on the
            // prefix alone — same as a shell.
            Origin::Shell | Origin::Edit { .. } => EscapeClass::PrefixOnly,
            Origin::Command => EscapeClass::PrefixAndCursor,
        }
    }
}

/// State the reader thread and the UI thread share. Split out from [`Session`]
/// so the thread can own an `Arc` of exactly what it touches.
pub struct Shared {
    /// The screen. A `Mutex`, not an `RwLock`: the reader is the only writer and
    /// holds it for microseconds per chunk.
    pub parser: Mutex<vt100::Parser>,
    /// The child's negotiated keyboard protocol, from the same bytes.
    pub kbmode: Mutex<kbmode::Stack>,
    /// Set when the child's output stream ends.
    pub exited: AtomicBool,
    pub exit: Mutex<Option<ExitStatus>>,
    /// Output has arrived that the UI has not drawn yet. Coalesces wake-ups: a
    /// chatty child (`yes`, a build log) would otherwise queue one event per
    /// read and starve the loop, so at most one is ever outstanding.
    pub dirty: AtomicBool,
    /// The write half of the master. Shared rather than owned by [`Session`]
    /// because `take_writer` may only be called once per master, and the reader
    /// thread needs it too: a child that sends the keyboard-protocol query
    /// blocks until the reply is written back into its input.
    pub writer: Mutex<Box<dyn Write + Send>>,
    /// Kitty graphics commands cut from the output, at the child's cursor,
    /// waiting for the next draw to forward them (pty/graphics.rs).
    pub graphics: Mutex<Vec<super::graphics::ChildGraphic>>,
}

pub struct Session {
    pub id: SessionId,
    pub origin: Origin,
    /// The command line, for the `@/pty` label and the completion notice.
    pub label: String,
    pub shared: Arc<Shared>,

    /// Kept so `resize` can be called. `MasterPty` is `Send` but not `Sync`, so
    /// this never leaves the main thread.
    master: Box<dyn MasterPty + Send>,
    /// The signal handle. The `Child` itself was moved into the reader thread,
    /// because `Child::wait` blocks and must not run on the UI thread; this is
    /// precisely what `clone_killer` exists for.
    killer: Box<dyn ChildKiller + Send + Sync>,

    /// The child's own pid, kept for the foreground-group comparison in
    /// [`at_prompt`](Self::at_prompt). Captured before the `Child` moves into
    /// the reader thread.
    shell_pid: Option<i32>,
    /// Last `(cols, rows)` pushed to the PTY, so a resize is issued only on
    /// change. The panel geometry is known at render time only, so this mirrors
    /// the viewport cache the markdown previewer uses.
    size: Cell<(u16, u16)>,
    /// The cell size in pixels last told to the child (it sizes images by it).
    px: Cell<(u16, u16)>,
}

impl Session {
    /// Open a PTY, spawn `argv` on it, and start the reader thread.
    #[allow(clippy::too_many_arguments)] // Nine plain values with no natural grouping;
    // a Params struct here would be one more type for one call site.
    pub fn spawn(
        id: SessionId,
        origin: Origin,
        label: String,
        argv: &[String],
        cwd: &std::path::Path,
        cols: u16,
        rows: u16,
        scrollback: usize,
        tx: Sender<AppEvent>,
    ) -> Result<Session, String> {
        let Some(program) = argv.first() else { return Err("empty command".into()) };
        let size = pty_size(cols, rows);
        let pair = native_pty_system().openpty(size).map_err(|e| e.to_string())?;

        let mut cmd = CommandBuilder::new(program);
        cmd.args(&argv[1..]);
        cmd.cwd(cwd);
        // Advertise the xterm subset vt100 actually implements. Kitty graphics
        // need no claim: they are passed through (pty/graphics.rs), and a
        // child that draws with them is told so explicitly (`mpv --vo=kitty`).
        cmd.env("TERM", "xterm-256color");

        let child = pair.slave.spawn_command(cmd).map_err(|e| e.to_string())?;
        // The slave must be dropped or the master never sees EOF, and the
        // reader thread would block forever after the child exits.
        drop(pair.slave);

        let shell_pid = child.process_id().map(|p| p as i32);
        let killer = child.clone_killer();
        let reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
        let writer = pair.master.take_writer().map_err(|e| e.to_string())?;

        let shared = Arc::new(Shared {
            parser: Mutex::new(vt100::Parser::new(rows, cols, scrollback)),
            kbmode: Mutex::new(kbmode::Stack::default()),
            exited: AtomicBool::new(false),
            exit: Mutex::new(None),
            dirty: AtomicBool::new(true),
            writer: Mutex::new(writer),
            graphics: Mutex::new(Vec::new()),
        });
        spawn_reader(shared.clone(), reader, child, tx);

        Ok(Session {
            id,
            origin,
            label,
            shared,
            master: pair.master,
            killer,
            shell_pid,
            size: Cell::new((cols, rows)),
            px: Cell::new(crate::ui::graphics::cell_px()),
        })
    }

    pub fn running(&self) -> bool {
        !self.shared.exited.load(Ordering::Acquire)
    }

    /// The `@/pty` display suffix: empty while running, else how it ended.
    pub fn exit_label(&self) -> String {
        if self.running() {
            return String::new();
        }
        match self.shared.exit.lock().ok().and_then(|g| g.clone()) {
            Some(s) if s.success() => " (exit 0)".into(),
            Some(s) => match s.signal() {
                Some(sig) => format!(" (signal {sig})"),
                None => format!(" (exit {})", s.exit_code()),
            },
            None => " (ended)".into(),
        }
    }

    /// Encode `key` for the child's negotiated protocol and write it. A failed
    /// write is dropped, never retried: the buffer being full means the child
    /// has stopped reading, and spinning here would freeze the UI thread.
    pub fn send_key(&self, key: crate::core::Key) -> bool {
        let flags = self.shared.kbmode.lock().map(|s| s.current()).unwrap_or(0);
        self.write(&super::encode::encode(key, flags))
    }

    /// Write raw bytes to the child. Returns false once the pipe is gone.
    pub fn write(&self, bytes: &[u8]) -> bool {
        if bytes.is_empty() {
            return true;
        }
        let Ok(mut w) = self.shared.writer.lock() else { return false };
        if w.write_all(bytes).is_err() || w.flush().is_err() {
            self.shared.exited.store(true, Ordering::Release);
            return false;
        }
        true
    }

    /// Match the PTY and the screen model to the panel. Both must be told, or
    /// the child and the screen disagree about where lines wrap.
    pub fn resize(&self, cols: u16, rows: u16) {
        let want = (cols.max(1), rows.max(1));
        let px = crate::ui::graphics::cell_px();
        if self.size.get() == want && self.px.get() == px {
            return;
        }
        self.size.set(want);
        self.px.set(px);
        // The PTY first, so the child gets its SIGWINCH and redraws at the new
        // size; the parser second, to receive that redraw already resized.
        let _ = self.master.resize(pty_size(want.0, want.1));
        if let Ok(mut p) = self.shared.parser.lock() {
            p.screen_mut().set_size(want.1, want.0);
        }
    }

    pub fn kill(&mut self) {
        let _ = self.killer.kill();
    }

    /// The tty's **foreground** process group.
    #[cfg(unix)]
    fn foreground_pid(&self) -> Option<i32> {
        self.master.process_group_leader()
    }

    #[cfg(not(unix))]
    fn foreground_pid(&self) -> Option<i32> {
        None
    }

    /// Whether the child is sitting at its prompt rather than running something.
    ///
    /// Two signals, because neither alone covers the shells people use:
    ///
    /// - **Bracketed paste.** A line editor turns DEC mode 2004 on while reading
    ///   a line and off while the command runs, so its state answers the question
    ///   directly. zsh, fish and nushell all do this.
    /// - **The foreground process group.** When the child never mentions 2004
    ///   (macOS's bash 3.2, dash — no line editor, or one predating the mode),
    ///   fall back to whether the tty's foreground group is still the shell.
    ///
    /// The fallback cannot be the primary: **zsh leaves job control off** when
    /// spawned this way, so its foreground group never moves and the test would
    /// always say "at a prompt". Measured, not assumed.
    ///
    /// Unknown answers as `false`: sending text into a running command is the
    /// failure worth avoiding.
    pub fn at_prompt(&self) -> bool {
        if !self.running() {
            return false;
        }
        if let Ok(m) = self.shared.kbmode.lock()
            && let Some(editing) = m.at_prompt()
        {
            return editing;
        }
        match (self.foreground_pid(), self.shell_pid) {
            (Some(fg), Some(shell)) => fg == shell,
            _ => false,
        }
    }

    /// Insert `text` at the child's prompt as if pasted — never executed, even
    /// if it contains a newline, when the child supports bracketed paste. The
    /// caller is responsible for [`at_prompt`](Self::at_prompt); a shell that is
    /// busy would take this as stdin for whatever is running.
    pub fn paste(&self, text: &str) -> bool {
        let bracketed = self.shared.kbmode.lock().map(|m| m.bracketed_paste()).unwrap_or(false);
        let mut out = Vec::new();
        if bracketed {
            out.extend_from_slice(b"\x1b[200~");
        }
        out.extend_from_slice(text.as_bytes());
        if bracketed {
            out.extend_from_slice(b"\x1b[201~");
        }
        self.write(&out)
    }

    /// Show the window at `from_top` rows down from the oldest scrollback row,
    /// and report `(available, actual)`.
    ///
    /// Core counts panel scroll from the top of the content; vt100 counts rows
    /// *back* from the live screen. Inverting here is what makes `k`/`j` mean
    /// the same thing over a terminal as over a document. vt100 publishes no
    /// scrollback length, so the ceiling is found by asking for more than could
    /// exist and reading the clamped answer back (`set_scrollback` is a `min`).
    pub fn scroll_to(&self, from_top: usize) -> (usize, usize) {
        let Ok(mut p) = self.shared.parser.lock() else { return (0, 0) };
        let screen = p.screen_mut();
        screen.set_scrollback(usize::MAX);
        let available = screen.scrollback();
        let actual = from_top.min(available);
        screen.set_scrollback(available - actual);
        (available, actual)
    }
}

/// The pty size, with pixels when the cell size is known: a child that draws
/// images (`mpv --vo=kitty`) reads them from TIOCGWINSZ to size its frames.
fn pty_size(cols: u16, rows: u16) -> PtySize {
    let (cw, ch) = crate::ui::graphics::cell_px();
    PtySize { rows, cols, pixel_width: cols.saturating_mul(cw), pixel_height: rows.saturating_mul(ch) }
}

/// Read the child until EOF, folding every chunk into the screen model and the
/// keyboard-mode tracker, then reap it.
fn spawn_reader(
    shared: Arc<Shared>,
    mut reader: Box<dyn std::io::Read + Send>,
    mut child: Box<dyn portable_pty::Child + Send + Sync>,
    tx: Sender<AppEvent>,
) {
    std::thread::spawn(move || {
        let mut buf = [0u8; READ_CHUNK];
        let mut split = super::graphics::Splitter::default();
        loop {
            let n = match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            let chunk = &buf[..n];
            // The keyboard-protocol query must be answered from here rather
            // than from the UI thread: a child that asks blocks until it is.
            let reply = shared.kbmode.lock().map(|mut m| m.feed(chunk)).unwrap_or_default();
            if !reply.is_empty()
                && let Ok(mut w) = shared.writer.lock()
            {
                let _ = w.write_all(&reply);
                let _ = w.flush();
            }
            // Text goes to the screen model; kitty commands are queued with the
            // cursor the text before them left.
            if let Ok(mut p) = shared.parser.lock() {
                for seg in split.feed(chunk) {
                    match seg {
                        super::graphics::Seg::Text(t) => p.process(t),
                        super::graphics::Seg::Apc(apc) => {
                            let Some(data) = super::graphics::kitty_command(&apc) else { continue };
                            let (row, col) = p.screen().cursor_position();
                            if let Ok(mut q) = shared.graphics.lock() {
                                super::graphics::push(&mut q, super::graphics::ChildGraphic { row, col, data });
                            }
                        }
                    }
                }
            }
            if !shared.dirty.swap(true, Ordering::AcqRel) {
                let _ = tx.send(AppEvent::Tick);
            }
        }
        // The read loop ends when the child closes its end, so this returns
        // straight away — but it *does* block, which is why the child lives
        // here and not in the Session.
        if let Ok(status) = child.wait()
            && let Ok(mut slot) = shared.exit.lock()
        {
            *slot = Some(status);
        }
        shared.exited.store(true, Ordering::Release);
        shared.dirty.store(true, Ordering::Release);
        let _ = tx.send(AppEvent::Tick);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefix_and_cursor_escapes_on_all_six_navigation_moves() {
        let got = EscapeClass::PrefixAndCursor.intents();
        assert_eq!(got.len(), 6);
        for want in [
            Intent::CursorUp,
            Intent::CursorDown,
            Intent::CursorLeft,
            Intent::CursorRight,
            Intent::CursorTop,
            Intent::CursorBottom,
        ] {
            assert!(got.contains(&want), "{want:?}");
        }
    }

    // ── real PTYs ─────────────────────────────────────────────────────────

    fn spawn_for_test(argv: &[&str]) -> (Session, std::sync::mpsc::Receiver<AppEvent>) {
        let (tx, rx) = std::sync::mpsc::channel();
        let argv: Vec<String> = argv.iter().map(|s| s.to_string()).collect();
        let s = Session::spawn(
            1,
            Origin::Command,
            argv.join(" "),
            &argv,
            std::path::Path::new("."),
            40,
            10,
            0,
            tx,
        )
        .expect("openpty");
        (s, rx)
    }

    /// Poll `f` until it holds or the budget runs out. The child runs on its own
    /// schedule, so a fixed sleep would be either flaky or slow.
    ///
    /// The budget is generous because these tests spawn real shells while the
    /// rest of the suite runs in parallel: a machine under that load took longer
    /// than two seconds to reach a prompt, which is a scheduling fact, not a
    /// failure. Success still returns immediately, so the slack costs nothing.
    fn wait_for(mut f: impl FnMut() -> bool) -> bool {
        for _ in 0..800 {
            if f() {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        false
    }

    fn screen_text(s: &Session) -> String {
        let p = s.shared.parser.lock().unwrap();
        p.screen().contents()
    }

    #[test]
    fn keys_written_to_the_child_come_back_through_its_echo() {
        let (s, _rx) = spawn_for_test(&["cat"]);
        for c in "hi".chars() {
            assert!(s.send_key(crate::core::Key::plain(crate::core::KeyCode::Char(c))));
        }
        assert!(s.send_key(crate::core::Key::plain(crate::core::KeyCode::Enter)));
        assert!(wait_for(|| screen_text(&s).contains("hi")), "got {:?}", screen_text(&s));
    }

    #[test]
    fn spawning_a_missing_program_fails_rather_than_leaving_a_dead_session() {
        let (tx, _rx) = std::sync::mpsc::channel();
        let argv = vec!["wasdf-no-such-program-xyz".to_string()];
        let r = Session::spawn(
            1,
            Origin::Command,
            argv[0].clone(),
            &argv,
            std::path::Path::new("."),
            40,
            10,
            0,
            tx,
        );
        assert!(r.is_err());
    }

}
