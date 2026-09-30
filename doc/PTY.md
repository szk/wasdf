# PTY Extension

`pty` is a bundled extension that allocates pseudo-terminals, runs child
processes on them, and renders their screens inside `PANEL_FUNCTION`. It hosts
three things: a free-standing shell session, commands from the built-in CmdOp
builder (when `cmdop.runner` is `"pty"`), and (optionally) the editor.

This document is the canon for the extension. It assumes
[ARCHITECTURE.md](ARCHITECTURE.md) (the pipeline, the purity and async
contracts), [EXTENSION.md](EXTENSION.md) (the trait, `Host`, input capture, the
Zero-Edit Principle), and [UI.md](UI.md) (the function panel, the key model,
focus). The string `"pty"` appears outside `extension/pty/` only in
`extension::bundled()`; CmdOp reaches it through the user-set value of
`cmdop.runner`, never by name ([EXTENSION.md](EXTENSION.md), Coupling between
extensions).

## 1. Core facilities used

A PTY is long-lived, bidirectional, and self-driven. It relies only on generic
core vocabulary, each usable by any extension:

| Facility | Canon |
|----------|-------|
| Faithful `Key` (Insert, F1–F35, super / hyper / meta, Press / Repeat / Release) and the Kitty keyboard protocol on wasdf's own terminal | [UI.md](UI.md), Key Model |
| Input capture: every key to the owner, a core-honoured release key, intent escapes, lossless `codec::encode_key` | [EXTENSION.md](EXTENSION.md), Input capture |
| Waking the loop from a reader thread (`Host.events`, `AppEvent::Tick`) and the per-pass `on_tick` hook | [EXTENSION.md](EXTENSION.md), The Extension Trait |
| A hosted form that scrolls like any panel frame (`render_form` + `form_scroll`) | [EXTENSION.md](EXTENSION.md) |
| The cursor-changed broadcast re-running once when a form releases the panel | [ARCHITECTURE.md](ARCHITECTURE.md), The Intent Pipeline |
| The function panel's title coming from whoever drew the frame | [UI.md](UI.md), Borders and Scrollbars |

No `PanelView` variant, `Plan`, `Purpose`, or `SubLayout` is specific to it.

## 2. Module layout

`no mod.rs` — the facade is `<dirname>.rs`.

```
crates/wasdf/src/extension/pty.rs             facade: `mod` declarations + re-exports
crates/wasdf/src/extension/pty/extension.rs   PtyExtension: the Extension impl
crates/wasdf/src/extension/pty/session.rs     Session: one PTY + child + parser + reader thread
crates/wasdf/src/extension/pty/sessions.rs    SessionTable: the live map, id allocation
crates/wasdf/src/extension/pty/encode.rs      Key -> child bytes (legacy + Kitty CSI-u)
crates/wasdf/src/extension/pty/kbmode.rs      sniff the child's keyboard-mode stack from its output
crates/wasdf/src/extension/pty/provider.rs    PtyProvider: the `@/pty` VfsProvider
crates/wasdf/src/extension/pty/keymap.scm     entry bindings
crates/wasdf/src/extension/pty/options.scm    option declarations
```

`PtyExtension` must be `Send` (the trait requires it) but is only ever touched
on the main thread; all cross-thread state lives behind the `Arc`s described in
§3.

---

## 3. Session model

```rust
/// A monotonically increasing per-process id. Never reused, so `@/pty/<id>`
/// paths stay stable for the life of the process.
pub type SessionId = u64;

/// How a session gives keyboard control back to wasdf. Every session releases
/// on the prefix key (`pty.prefix-key`, default `C-b`) — that is core's
/// unconditional guarantee (input capture, EXTENSION.md). This enum picks the *additional*
/// release path a session class may ask for, and it is expressed as intents,
/// never as keys, so it follows the user's keymap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EscapeClass {
    /// Prefix key only. Everything else — including cursor movement — goes to
    /// the child. Used by `Origin::Shell` and `Origin::Edit`: a shell and an
    /// editor both need the full alphabet.
    PrefixOnly,
    /// Prefix key, plus **every** file-cursor move. Used by `Origin::Command`,
    /// where the child is a one-shot command whose interaction is at most a
    /// `y/n` prompt, so surrendering the movement keys costs nothing and puts
    /// the file cursor one keystroke away.
    PrefixAndCursor,
}

impl EscapeClass {
    /// The `InputCapture::escape` set this class installs.
    ///
    /// All six navigation intents, not just the four WASD moves: reaching for
    /// the top or the bottom of the listing is the same gesture as stepping
    /// through it, and a session that broke out on `s` but swallowed
    /// `CursorBottom` would be arbitrary. `Activate` and `NavigateTo` are
    /// deliberately *not* here — those act on an entry, and acting on the file
    /// panel while a command is still running is a different decision from
    /// merely looking away from it.
    fn intents(self) -> Vec<Intent> {
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

/// Why the session exists. Drives the `@/pty` label and the exit behaviour.
#[derive(Debug, Clone, PartialEq)]
pub enum Origin {
    /// A free-standing interactive shell the user opened.
    Shell,
    /// A command CmdOp's builder assembled. `label` is the command line.
    Command { label: String },
    /// The editor `StartEdit` opened on `path`.
    Edit { path: PathBuf },
}

pub struct Session {
    pub id: SessionId,
    pub origin: Origin,
    pub escape: EscapeClass,
    /// The command line, for `@/pty` display and the completion notice.
    pub argv: Vec<String>,
    pub cwd: PathBuf,

    /// vt100 screen state. Written by the reader thread, read by `render_form`.
    /// A `Mutex` (not `RwLock`): the reader is the only writer and holds it for
    /// microseconds per chunk.
    pub parser: Arc<Mutex<vt100::Parser>>,
    /// The child's keyboard-protocol mode stack, updated by the same reader
    /// thread from the same bytes (see §4.3).
    pub kbmode: Arc<Mutex<kbmode::Stack>>,

    /// **The write half lives in `Shared`, not here.** `take_writer` may only
    /// be called once per master, and the reader thread needs it too: a child
    /// that sends the keyboard-protocol query (`CSI ? u`) blocks until the
    /// reply is written back into its own input.
    /// Kept alive so the PTY is not closed, and so `resize` can be called.
    /// `MasterPty` is `Send` but **not** `Sync`, so this never leaves the main
    /// thread.
    master: Box<dyn portable_pty::MasterPty + Send>,
    /// The `Child` itself is **moved into the reader thread**, because
    /// `Child::wait(&mut self)` blocks and must not run on the main thread.
    /// What stays here is the signal handle `ChildKiller::clone_killer()`
    /// returns — that is exactly what it exists for.
    killer: Box<dyn portable_pty::ChildKiller + Send + Sync>,

    /// Set by the reader thread when the child's output stream ends.
    pub exited: Arc<AtomicBool>,
    /// The exit status, once reaped. `None` while running.
    /// `ExitStatus::exit_code()` is a `u32` and `signal()` an `Option<&str>`;
    /// keep the whole struct rather than flattening it, so a signalled child
    /// can be reported as such.
    pub exit: Arc<Mutex<Option<portable_pty::ExitStatus>>>,
    /// Last `(cols, rows)` pushed to the PTY, so resize is issued only on change.
    pub size: Cell<(u16, u16)>,
}
```

`SessionTable` is a `RefCell<BTreeMap<SessionId, Session>>` inside
`PtyExtension`, plus an `Arc<RwLock<Vec<SessionSummary>>>` shared with
`PtyProvider` (the provider runs on the async task thread and must not touch
the `RefCell`). `SessionSummary` is a plain `{ id, label, origin_kind, running,
exit_label }` snapshot (`exit_label` already rendered, e.g. `"exit 1"` /
`"signal SIGTERM"`) refreshed whenever the table changes — this mirrors how
`BookmarksProvider` shares an `Arc<RwLock<Vec<PathBuf>>>` with its extension.

**Attached session.** At most one session is displayed at a time:
`attached: Cell<Option<SessionId>>`. Attaching means:

1. `Intent::SetFunctionForm(Some("pty"))` — claims the panel as an **active**
   form and moves focus.
2. `Intent::SetInputCapture(Some(InputCapture { owner: "pty", release: prefix_key, escape: session.escape.intents() }))`.

**Released** is the state after the keyboard came back — the prefix key, or
the child exiting — while the session stays on screen: the extension re-claims
the panel as a **passive** form (`HostFunctionForm(Some("pty"))`), so core's
keys work normally around it (`wasd` move the file cursor, `j`/`k` scroll the
scrollback) and the `pty-released` predicate holds. **Detaching** drops the
display entirely (`HostFunctionForm(None)` / `SetFunctionForm(None)`,
`SetInputCapture(None)`). Either way the session keeps running and its output
keeps accumulating in its `vt100::Parser`.

### Spawning

```rust
use portable_pty::{native_pty_system, CommandBuilder, PtySize};

let sys = native_pty_system();
let pair = sys.openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })?;
let mut cmd = CommandBuilder::new(&argv[0]);
cmd.args(&argv[1..]);
cmd.cwd(&cwd);
cmd.env("TERM", "xterm-256color");
let child = pair.slave.spawn_command(cmd)?;   // Box<dyn Child + Send + Sync>
drop(pair.slave);                             // must drop, or EOF never arrives
let killer = child.clone_killer();
let mut reader = pair.master.try_clone_reader()?;
let writer = pair.master.take_writer()?;      // valid exactly once per master
let master = pair.master;
// `child` is moved into the reader thread below; `killer`, `writer` and
// `master` stay in the Session on the main thread.
```

Opening the PTY and spawning the child happen synchronously inside
`handle_intent` — a sanctioned exception of the Purity Contract, bounded to one
spawn per explicit user action.

`TERM=xterm-256color` is deliberate: vt100 0.16 implements the xterm subset.
Do not advertise `xterm-kitty` — we do not implement the graphics protocol.

### Reader thread

One thread per session:

```rust
std::thread::spawn(move || {
    let mut buf = [0u8; 8192];
    loop {
        match reader.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                kbmode.lock().unwrap().feed(&buf[..n]);   // §4.3
                parser.lock().unwrap().process(&buf[..n]);
                // Coalesce: only wake the loop if it is not already awake.
                if !dirty.swap(true, Ordering::AcqRel) {
                    let _ = tx.send(AppEvent::Tick);
                }
            }
        }
    }
    // The read loop ends when the child closes its end, so `wait` returns
    // immediately here. It blocks, which is why the child was moved into this
    // thread rather than kept in the Session.
    if let Ok(status) = child.wait() {
        *exit.lock().unwrap() = Some(status);
    }
    exited.store(true, Ordering::Release);
    let _ = tx.send(AppEvent::Tick);
});
```

The reader thread wakes the loop through `Host.events`. The
`dirty: Arc<AtomicBool>` is cleared by `render_form`. Without it, a chatty
child (`yes`, a build log) floods the channel with one `Tick` per 8 KB read and
the loop starves. With it, at most one wake-up is outstanding at a time, so the
repaint rate self-limits to the render rate.

**Reaping.** `child.wait()` blocks, so it must not run on the main thread. The
reader thread calls it after the read loop ends and stores the status in
`exit`. `PtyExtension` observes `exited` from `on_tick` — the per-pass hook,
since a child exiting leaves no trace in `AppState` for the deduplicated
cursor-changed broadcast to notice — and emits the completion notice, the
`@/messages` line, (for `Origin::Edit`) the Refresh, and the `close-on-exit`
handling from there.

---

## 4. Input

### 4.1 The capture contract

While an `InputCapture` naming `pty` is installed, `handle_key` sends
**every** key that is not one of the two escapes to `PtyExtension::handle_intent`
as `ExtensionIntent { extension: "pty", intent: "key", data: encode_key(key) }`.
That includes `q`, `Ctrl-C`, and the arrow keys.

### 4.2 Escape handling

**All escape logic lives in core** ([EXTENSION.md](EXTENSION.md), Input capture). The extension does not inspect keys
for escapes, and `handle_intent("key", …)` never has to decide whether to
release — if it was called, the key is the child's. This is what makes the
lockout guarantee real: the extension is not a participant in it.

The extension's two obligations are:

- **`handle_intent("key", …)`** — decode, encode for the child (§4.3), write
  (§4.4). Return no intents.
- **`handle_intent("prefix", …)`** — core has *already* cleared the capture, so
  all that is left is to switch to the released state (a passive form) and let
  the session stay on screen. It opens no
  submenu: an extra key to dismiss one would make the single guaranteed way out
  of a captured keyboard cost two presses.

**Releasing is not closing.** The prefix key, and a child exiting, give back the
*keyboard* and leave the session in the panel. That is what makes the terminal
scroll like every other function-panel frame (§5): the output stays readable,
`k`/`j` walk the scrollback, and `x` takes the keyboard back. `Enter` keeps
core's meaning while released — Open, the file under the file cursor, via the
resolver; the pty extension binds no Enter. Closing the panel outright is a
separate action (`pty-detach`, or `pty.close-on-exit`).

`Ctrl-C` is not special. Under capture it encodes to `\x03` and goes to the
child, which is the whole point of a terminal panel. wasdf does not need its
own `Ctrl-C` while attached; `Ctrl-b` is the way out.

Note that `portable-pty` puts the child in its own session on Unix, so a
`Ctrl-C` typed at the *outer* terminal reaches wasdf, not the child — only the
encoded `\x03` reaches the child. That is the correct and intended split.

### 4.2.1 Why `Origin::Edit` uses `PrefixOnly`

An editor needs every letter, `w` and `s` most of all. `Origin::Command` can
afford to give up the six navigation moves because a command CmdOp assembled is a
one-shot whose interaction is at most a `y/n` confirmation; an editor cannot.
So `Edit` escapes on the prefix key alone, exactly like a shell.

### 4.3 Encoding for the child

**Both encodings are supported.** The child gets CSI-u the moment it asks for
it.

The child decides its own keyboard protocol, and wasdf's own input protocol
([UI.md](UI.md), Key Model) is irrelevant to that decision — the two are independent negotiations
that happen to use the same escape sequences. `kbmode.rs` tracks the child's
side by sniffing its output byte stream, because `vt100` 0.16 does not model
these sequences:

| Sequence | Meaning |
|----------|---------|
| `CSI > flags u` | push `flags` onto the child's keyboard stack |
| `CSI < n u` | pop `n` entries |
| `CSI = flags ; mode u` | set flags on the current entry |
| `CSI ? u` | query — reply `CSI ? flags u` **into the child's input** |

`kbmode::Stack` is a `Vec<u8>` of flag words with `Stack::current() -> u8`
(0 when empty = legacy). Implement `feed(&[u8])` as a small state machine over
`ESC [` … final byte `u`; anything else is passed over. It never modifies the
stream — `vt100` still sees the full bytes.

`encode.rs` then has exactly two paths:

```rust
/// The bytes to write to the child for `key`, given the child's current
/// keyboard flags. `flags == 0` selects the legacy encoding.
pub fn encode(key: Key, flags: u8) -> Vec<u8>;
```

**Legacy path** (`flags == 0`) — the xterm encoding every TUI understands:

| Key | Bytes |
|-----|-------|
| `Char(c)`, no mods | UTF-8 of `c` |
| `Char(c)` + ctrl, `c` in `@A-Z[\]^_` or `a-z` | `c as u8 & 0x1f` |
| `Char(c)` + alt | `ESC` then the above |
| `Enter` | `\r` (**not** `\n`) |
| `Tab` / `BackTab` | `\t` / `ESC [ Z` |
| `Backspace` | `\x7f` |
| `Esc` | `\x1b` |
| `Up`/`Down`/`Right`/`Left` | `ESC [ A/B/C/D`, or `ESC [ 1 ; <m> A` etc. when modified |
| `Home`/`End` | `ESC [ H` / `ESC [ F` |
| `Insert`/`Delete`/`PageUp`/`PageDown` | `ESC [ 2~` / `3~` / `5~` / `6~` |
| `F(1..=4)` | `ESC O P/Q/R/S` |
| `F(5..=12)` | `ESC [ 15~ 17~ 18~ 19~ 20~ 21~ 23~ 24~` |
| `KeyKind::Repeat` | same as `Press` |
| `KeyKind::Release` | **nothing** (legacy terminals have no release events) |

The xterm modifier parameter is `1 + shift(1) + alt(2) + ctrl(4) + meta(8)`.

**Kitty path** (`flags != 0`) — CSI-u per the
[Kitty keyboard protocol](https://sw.kovidgoyal.net/kitty/keyboard-protocol/):

```
CSI <unicode-key-code> [ : <alternate> ] [ ; <modifiers> [ : <event-type> ] ] u
```

- modifiers = `1 + shift(1) + alt(2) + ctrl(4) + super(8) + hyper(16) + meta(32)`
  (caps-lock 64 and num-lock 128 are not tracked by core `Mods`; always 0)
- event-type = `1` press, `2` repeat, `3` release; emitted only when the child
  set `REPORT_EVENT_TYPES` (bit 0b10)
- functional keys use their CSI-u numbers (`Enter` 13, `Tab` 9, `Backspace`
  127, `Esc` 27, `Insert` 2, `Delete` 3, `Left` 57417 … or the legacy `ESC [ D`
  form when `DISAMBIGUATE_ESCAPE_CODES` alone is set)

**Only emit a release event when the child asked for it.** Sending release
events to a child that did not set `REPORT_EVENT_TYPES` doubles every keystroke.

**The prefix key is never encoded, under either path.** Core consumes it before
the extension sees it, so no child in any session class can receive it — that
is the price of the lockout guarantee. A child that genuinely needs the key
(`Ctrl-b` is `vim`'s page-up) needs `pty.prefix-key` rebound.

### 4.4 Writing

`handle_intent` writes the encoded bytes directly:

```rust
let _ = session.writer.write_all(&bytes);
let _ = session.writer.flush();
```

This is one of the Purity Contract's sanctioned exceptions
([ARCHITECTURE.md](ARCHITECTURE.md)): a write to a handle the extension owns
that cannot block. If the buffer is full (the child has stopped reading), the
write is **dropped, not retried**. Never spin.

### 4.5 Session management

There is no prefix submenu. The release key moves the session to the
released state and that is all it does (§4.2), so the remaining session
operations are palette commands — an existing
mechanism that costs no new mode, no new keymap scope, and no extra keystroke
on the path out of a capture:

| Command | Action |
|---------|--------|
| `pty-detach` | give the panel back; the session keeps running |
| `pty-next` / `pty-prev` | attach the next / previous session in id order |
| `pty-kill` | kill the attached child |

Sessions are reached with:

| Key | Scope | Action |
|-----|-------|--------|
| `x` | file / function panel | Attach to the shell already open, or start one if there is none |
| `X` | file / function panel | Always start another shell |
| `e` | file panel | The editor on the file under the cursor (`pty.edit-host`) |
| `h` / `l` | function panel, released | Step left / right through the open sessions |
| `j` / `k` | function panel, released | Scroll the terminal (core's own bindings) |
| `w` `a` `s` `d` | either | Move the file cursor — which gives the panel back to the preview |

Stepping with `h`/`l` *shows* a session without
grabbing the keyboard, since browsing terminals should not cost an escape per
step; `x` is what resumes typing in one.

Moving the file cursor while a session is on screen but released closes the
panel, so the preview follows the cursor exactly as it would have if no session
had ever been open (the kernel re-runs the cursor-changed broadcast once when
a form releases the panel, so this lands on the first keystroke).

---

## 5. Rendering

```rust
fn render_form(&self, frame: &mut Frame, inner: Rect, ctx: &FunctionRenderCtx) -> Option<String> {
    let id = self.attached.get()?;
    let table = self.sessions.borrow();
    let s = table.get(&id)?;

    self.resize_if_needed(s, inner);        // §6
    s.dirty.store(false, Ordering::Release);

    let parser = s.parser.lock().ok()?;
    let term = PseudoTerminal::new(parser.screen())
        .cursor(Cursor::default().visibility(ctx.focused));
    frame.render_widget(&term, inner);

    Some(self.title(s))                     // e.g. "pty 3: zsh" / "pty 4: rg foo (exit 1)"
}
```

Core wraps the border, title and open-bottom chrome; the interior is entirely
the extension's. `PseudoTerminal` paints every cell including the background,
so the panel does not need clearing first.

**No `PanelView`.** `view()` returns `None` for the pty extension — a terminal
grid is pre-styled cells, which is exactly what the logical `PanelView`
vocabulary refuses to carry. `render_form` is the sanctioned bundled-only
escape and this is precisely its intended use.

**Scrollback.** `form_scroll` reports `(scrollback + rows, position)`, so
the panel draws the same scrollbar as any other frame and core clamps the scroll
for us. Core counts rows from the top of the content and vt100 counts rows back
from the live screen, so `Session::scroll_to` inverts between them — without
that, `k` and `j` would mean the opposite of what they mean over a document.
New output pushes the view back to the live tail (`Intent::SetScroll`), the way
a terminal behaves.

**Search.** There is no `/` search over a terminal screen; the extension
handles no search intents. (Under capture, `/` goes to the child anyway.)

---

## 6. Resize

The panel's inner rect is known only at render time, so resizing happens in
`render_form` — the Render Contract's one sanctioned I/O, issued only when the
size changed:

```rust
fn resize_if_needed(&self, s: &Session, inner: Rect) {
    let want = (inner.width.max(1), inner.height.max(1));
    if s.size.get() == want { return; }
    s.size.set(want);
    let _ = s.master.resize(PtySize {
        rows: want.1, cols: want.0, pixel_width: 0, pixel_height: 0,
    });
    if let Ok(mut p) = s.parser.lock() {
        // Note: `set_size` lives on `Screen`, not `Parser` — go through
        // `screen_mut()`. There is no `Parser::set_size` in vt100 0.16.
        p.screen_mut().set_size(want.1, want.0);
    }
}
```

Order matters: resize the PTY first (so the child gets `SIGWINCH` and redraws
at the new size), then the parser. Both must be told, or the screen and the
child disagree about wrapping.

A detached session is never resized — it keeps the geometry it had when it was
last visible. Re-attaching resizes it on the first `render_form`.

---

## 7. `@/pty` — the session tree

`PtyProvider` implements `VfsProvider` with `mount() == "pty"`. Virtual paths
are lowercase (`@/pty/...`); the VFS root listing capitalizes display names on
its own, so the user sees **PTY** next to FILESYSTEM / BOOKMARKS / SETTINGS.

```
@/pty/                        one directory per live session
@/pty/<id>/                   e.g. @/pty/3
@/pty/<id>/console            leaf, activation = attach
```

`@/pty/<id>` is a **directory** holding one leaf, reserving the shape for a
child-process listing (`@/pty/<id>/<pid>`) without a path change.

Session directory names are `<id>`; the *display* name is
`"<id>: <label>"` (`zsh`, `rg foo`, `vim README.md`), suffixed `" (exit N)"`
once the child has exited.

`console`'s activation is the standard leaf pattern:

```rust
e.activation = Some(ExtensionIntent {
    extension: "pty".into(),
    intent: "attach".into(),
    data: ExtensionValue::Int(id as i64),
});
```

`PtyProvider::list` reads the shared `Arc<RwLock<Vec<SessionSummary>>>` only —
it runs on a worker thread and stays in-memory.

**Exited sessions stay listed** until the user removes them, so a command's
console output is inspectable after the fact. `pty.keep-exited` (§8) bounds how
many; the oldest exited session is dropped past the cap.

---

### 7.1 Handing the shell a directory

Resuming a shell with `x` pastes the **directory holding whatever the file
cursor points at** into its prompt — the entry's *parent*, so a cursor on
`/a/b/c.txt` gives `/a/b` and one on the directory `/a/b/d` also gives `/a/b`.

The cursor's entry, not the file panel's `cwd`: after `f` the cursor can be
sitting on a match from somewhere else entirely, and that match's own directory
is the useful answer.

It is **pasted, not run**. Nothing executes, no history line is written, and no
shell-specific quoting or command name is needed — the user decides what the
path is for. Where the child supports bracketed paste the text is wrapped in
`ESC[200~` … `ESC[201~`, so even an embedded newline could not run it.

A virtual location (`@/settings`) offers nothing; no shell can use it.

### 7.2 Knowing the shell is at its prompt

Pasting into a *running* command would feed it stdin, so the paste is gated on
[`Session::at_prompt`], which reads two signals because neither alone covers the
shells people use:

| Signal | Covers | How |
|--------|--------|-----|
| **Bracketed paste** (DEC mode 2004) | zsh, fish, nushell | A line editor turns it on while reading a line and off while the command runs, so its state answers the question directly |
| **The tty's foreground process group** | bash 3.2, dash | Fallback for a child that never mentions 2004 — no line editor, or one predating the mode |

The fallback cannot be the primary: **zsh leaves job control off** when spawned
this way, so its foreground group never moves and the test would always answer
"at a prompt".

Unknown answers as "not at a prompt": silently doing nothing is a better failure
than typing into a running program.

**There is no standard for the other direction.** OSC 7 (cwd) and OSC 133
(prompt state) are established, widely-implemented ways for a shell to *report*
to its terminal, and kitty, WezTerm, iTerm2, foot and Ghostty all consume them —
but no terminal protocol, kitty's included, can *set* a shell's directory. That
is why this pastes text instead: pushing a `cd` would mean typing a command on
the user's behalf, with per-shell quoting, a per-shell line-kill key, per-shell
history suppression, and no effect at all on nushell, whose `cd` never
`chdir(2)`s the process.

---

## 8. Options

`option_scheme()` returns `include_str!("options.scm")`. Every key is tagged
with the extension id automatically and appears under
`@/settings/Extensions/pty`.

```scheme
;; PTY extension options.
(quote (
  (option "pty.prefix-key"    "Pty" text                                "Ctrl-b" "prefix-key")
  (option "pty.shell"         "Pty" text                                ""       "shell")
  (option "pty.scrollback"    "Pty" (int 0 100000)                      "2000"   "scrollback")
  (option "pty.max-sessions"  "Pty" (int 1 64)                          "8"      "max-sessions")
  (option "pty.keep-exited"   "Pty" (int 0 32)                          "4"      "keep-exited")
  (option "pty.edit-host"     "Pty" bool                                "#t"     "run the editor on a PTY")
  (option "pty.close-on-exit" "Pty" (choice "never" "success" "always") "never"  "close-on-exit")))
```

- **`pty.prefix-key`** becomes `InputCapture::release` — the key core honours
  unconditionally, and therefore the one key no child can ever receive.
  It is parsed with the existing `script::codec::parse_key`. A value that does
  not parse, or that carries no ctrl modifier, is rejected at load with a
  `@/messages` line and the default is used; requiring ctrl is what keeps a
  bare letter from becoming unsendable. Whatever it is set to, **no child can
  ever receive it** — it is the one key wasdf keeps. `C-b` collides with
  `vim`'s page-up, so a user who turns on `pty.edit-host` will likely want to
  move it.
- **`pty.shell`** empty means: `$SHELL` on Unix, `%COMSPEC%` on Windows. See
  §11. A session starts in the file panel's directory when that is a real one,
  and in the home directory when it is virtual (`@/settings`, `@/pty`) — no
  process can chdir into a VFS path.
- **`pty.scrollback`** is `vt100::Parser::new`'s third argument.
- **`pty.close-on-exit`** — whether the *panel* also goes when the child exits.
  The keyboard always comes back regardless (a dead child cannot use it), so
  `never` (the default) leaves the final output on screen and scrollable, which
  is usually what you want after a command finishes. `always` clears the panel
  too; `success` clears it only on exit code 0.

Options are read through `Host.options`.

---

## 9. The command-runner contract (CmdOp)

pty implements core's **runner contract** ([CMDOP.md](CMDOP.md)): with the core
option `cmdop.runner` set to `"pty"`, a command the built-in CmdOp builder
assembles runs on a PTY instead of the Exec frame. CmdOp then emits:

```rust
vec![
    Intent::SetFunctionForm(None),          // CmdOp releases its form first…
    Intent::Extension(ExtensionIntent {       // …then the runner claims the panel
        extension: runner,                    // the value of `cmdop.runner`
        intent: "run".into(),
        data: ExtensionValue::Map(btreemap! {
            "argv"   => ExtensionValue::List(argv.iter().map(|s| ExtensionValue::String(s.clone())).collect()),
            "cwd"    => ExtensionValue::Path(cwd),
            "label"  => ExtensionValue::String(display_line),
            "escape" => ExtensionValue::String("prefix-and-cursor".into()),
        }),
    }),
]
```

Core never names `pty`: the id comes from the user's option value. With the
option empty (the default), or naming an extension that is not registered,
CmdOp runs the command as `Execute`.

### What `"run"` does

1. Spawn a session with `Origin::Command { label }`,
   `EscapeClass::PrefixAndCursor`.
2. Attach it (§3): `SetFunctionForm` + `SetInputCapture` with `escape =
   [CursorUp, CursorDown, CursorLeft, CursorRight, CursorTop, CursorBottom]`.
3. `PANEL_FUNCTION` now shows the child's live screen; the user can type into
   it (it is a real PTY, so a prompt from `rm -i` or a `git` credential query
   works — `y`/`n` and the whole alphabet except the movement keys reach the
   child).
4. Any key the keymap resolves to one of the six navigation intents detaches:
   core clears the capture and dispatches the move, so the file cursor actually
   moves on that same keystroke. Because the escape is declared as intents,
   this is `w`/`a`/`s`/`d` (plus whatever is bound to top/bottom) by default
   and follows the user's keymap if they rebind it. The session keeps running.
5. On exit, the extension emits `Intent::Notify(...)` with the same shape the
   resolver's `OpDone` notice uses ("`<label>`: ok" / "`<label>`: exit 1"), and
   pushes the same line to `@/messages`.
6. The console remains at `@/pty/<id>/console` until evicted by
   `pty.keep-exited`.

---

## 10. Edit integration

Core's `StartEdit` (`E`) reduces to `Plan::Suspend { argv: [editor, path] }`,
which leaves the alternate screen entirely. With `pty.edit-host` on, `e` instead
emits the same `"run"` intent with
`Origin::Edit { path }`, **`EscapeClass::PrefixOnly`**, and issues
`Intent::Refresh` when the child exits (so a saved file's new mtime/size shows
up) — matching what the Suspend path already does.

`PrefixOnly` is the whole reason an editor is viable here: `w`, `a`, `s` and
`d` reach `vim` like every other letter, and the prefix key is the only key
wasdf takes.

That one key is taken unconditionally. `Ctrl-b` is also `vim`'s page-up, and
the prefix wins — a user who edits in the panel should rebind
`pty.prefix-key` to something the editor does not want.

**The default is on (`#t`)**: `e` opens the editor in the panel, and the file
panel stays visible beside it. Turning the option off gives `e` back to core
(Activate); core's own editor, which suspends the TUI and hands the editor the
whole terminal, stays on `E` either way.

Core never names `pty` for this: the extension binds `e` at the Extension layer
to `(ext "pty" "edit")`, gated on `(option "pty.edit-host")`.
Layer priority (Core < Extension < User) makes the override work with zero core
edits, and a user binding still wins over both.

---

## 11. Windows

- `portable-pty` uses **ConPTY** on Windows (`src/win/conpty.rs`), which needs
  Windows 10 1809+. Its winpty fallback needs an external `winpty-agent.exe`;
  do not depend on it. If `openpty` fails, follow the project rule — notify,
  and let the caller fall back (a runner request to `Intent::Execute`, Edit to
  `Plan::Suspend`).
- **uutils does not provide a shell.** `uutils/coreutils` gives cross-platform
  `ls` / `cat` / `cp`, which is what the *resolver* needs, but a terminal panel
  needs a shell to run. `pty.shell` therefore defaults per platform:
  `$SHELL` → `/bin/sh` on Unix, `%COMSPEC%` → `cmd.exe` on Windows. A user who
  wants one shell everywhere can point it at `nu` or `pwsh`.
- Windows terminals do not implement the Kitty keyboard protocol. The
  `supports_keyboard_enhancement()` guard returns false there and everything
  degrades to the legacy encoding, which is the correct behaviour — do not
  special-case the platform.
- `CommandBuilder::new` on Windows resolves through `PATH` with `.exe`
  appended; pass a bare `"cmd"`, not `"cmd.exe"`.

---

## 12. Failure policy

Uniform with the rest of the project: **disable, notify, continue.**

| Failure | Behaviour |
|---------|-----------|
| `openpty` fails | notice + `@/messages`; no session; caller falls back |
| `spawn_command` fails | same, plus the PTY is closed |
| reader thread read error | treat as EOF: mark exited, wake once |
| `writer.write_all` fails | drop the keystroke; mark the session exited if the error is `BrokenPipe` |
| a lock is poisoned | detach and mark the session dead; never `unwrap` a lock in `render_form` |
| `pty.prefix-key` unparseable | `@/messages` line, use `C-b` |
| session cap reached | notice; refuse the spawn, do not evict a running session |

**A dead session should release capture, but nothing depends on it doing so.**
The tidy path is in `render_form` and `handle_intent("key", …)`: if the
attached session is missing or exited *and* `pty.close-on-exit` says to close,
emit `SetInputCapture(None)` + `SetFunctionForm(None)`. If that path is never
reached — the extension panicked, a lock is poisoned, the session vanished —
the prefix key still frees the user, because core honours it without consulting
the extension. That is the difference between a convention and a
guarantee, and it is why the release key lives in core.

---

## 13. Dependencies

```toml
portable-pty = "0.9"   # Unix openpty and Windows ConPTY behind one API
vt100 = "0.16"         # the screen model
tui-term = "0.3"       # blits the vt100 screen into a ratatui Buffer
```

`tui-term`'s `Screen` / `Cell` traits abstract the backend, so `vt100` can be
swapped for another screen model without touching `render_form`.
