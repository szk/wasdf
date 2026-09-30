//! `PtyExtension`: owns the session table, draws the attached session into the
//! function panel, and turns captured keys into child input.
//!
//! It is the reference "raw input" extension the way CmdOp is the reference
//! "form" one. Core / app / ui never name it: it reaches the screen through
//! `render_form`, the keyboard through `InputCapture`, and `@/pty` through an
//! ordinary `VfsProvider`.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::sync::{Arc, OnceLock};

use ratatui::Frame;
use ratatui::layout::Rect;
use tui_term::widget::{Cursor, PseudoTerminal};

use crate::core::{
    AppEvent, AppState, ExtensionIntent, ExtensionValue, InputCapture, Intent, Key, KeyCode, Mods,
    OptionStore,
};
use crate::extension::{About, Extension, FunctionRenderCtx};
use crate::script::codec;
use crate::services::command::CommandDef;
use crate::vfs::VfsProvider;

use super::provider::PtyProvider;
use super::session::{Origin, Session, SessionId};
use super::sessions::SessionTable;

const ID: &str = "pty";
const KEYMAP_SCHEME: &str = include_str!("keymap.scm");
const OPTIONS_SCHEME: &str = include_str!("options.scm");

/// Used before the first render tells us the real panel size, and whenever the
/// panel is too small to be worth resizing to.
const FALLBACK_SIZE: (u16, u16) = (80, 24);

/// The directory holding whatever the file cursor points at — its *parent*, so
/// a cursor on `/a/b/c.txt` yields `/a/b` and one on the directory `/a/b/d`
/// yields `/a/b`. `None` for a virtual location, which no shell can use.
fn cursor_dir(state: &AppState) -> Option<PathBuf> {
    let path = state.cursor_path().or_else(|| Some(state.cwd.clone()))?;
    if crate::vfs::is_virtual(&path) {
        return None;
    }
    path.parent().map(PathBuf::from)
}

/// The user's home, or the filesystem root when the environment does not say.
fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

#[derive(Default)]
pub struct PtyExtension {
    sessions: RefCell<SessionTable>,
    /// The session currently shown in the function panel, if any.
    attached: Cell<Option<SessionId>>,
    /// Panel `(cols, rows)` as of the last render — the geometry a newly
    /// spawned session should start at, since a PTY has to be sized before its
    /// child is spawned but the panel rect is only known while drawing.
    viewport: Cell<(u16, u16)>,
    /// Sessions whose exit has already been announced, so the notice fires once.
    reported: RefCell<Vec<SessionId>>,
    /// Where the file cursor stood when this session took the panel.
    ///
    /// Stepping aside is keyed on the cursor *moving*, and the cursor-changed
    /// broadcast is not that signal: its signature includes the function
    /// panel's scroll offset (so a content provider can page as the viewport
    /// advances), so `j`/`k` scrolling the terminal fires it just as a cursor
    /// move does. Comparing the target itself is what tells the two apart.
    cursor_when_shown: RefCell<Option<PathBuf>>,
    /// `(total rows, position)` for the panel's scrollbar, measured while
    /// drawing. `form_scroll` runs just after `render_form` and would otherwise
    /// have to redo the whole scrollback probe.
    scroll_extent: Cell<(usize, usize)>,
    /// An `Arc` rather than a plain `OnceLock` because `register_conditions`
    /// runs before `bind_store` and hands core a `'static` closure, which needs
    /// its own handle on the slot the store will later land in.
    store: Arc<OnceLock<Arc<OptionStore>>>,
    tx: OnceLock<Sender<AppEvent>>,
}

impl PtyExtension {
    pub fn new() -> Self {
        PtyExtension { viewport: Cell::new(FALLBACK_SIZE), ..Default::default() }
    }

    // ── options ───────────────────────────────────────────────────────────

    fn opt_str(&self, key: &str) -> String {
        self.store.get().map(|s| s.get_str(key)).unwrap_or_default()
    }

    fn opt_u32(&self, key: &str, fallback: u32) -> u32 {
        self.store.get().map(|s| s.get_u32(key)).unwrap_or(fallback)
    }

    /// The key core will honour unconditionally. A value that does not parse,
    /// or that carries no ctrl, is refused: without ctrl the prefix would be a
    /// bare letter, and that letter could then never reach any child.
    fn prefix_key(&self) -> Key {
        let default = Key::new(KeyCode::Char('b'), Mods::CTRL);
        let raw = self.opt_str("pty.prefix-key");
        if raw.is_empty() {
            return default;
        }
        match codec::parse_key(&raw) {
            Some(k) if k.mods.ctrl => k,
            _ => {
                crate::services::messages::log(
                    format!("pty.prefix-key: '{raw}' is not a Ctrl- key; using Ctrl-b"),
                    true,
                );
                default
            }
        }
    }

    /// The shell to run for `new-shell`. Empty option means the platform
    /// default: uutils gives us cross-platform coreutils but no shell, so this
    /// has to be resolved per platform rather than assumed.
    fn shell_argv(&self) -> Vec<String> {
        let configured = self.opt_str("pty.shell");
        if !configured.trim().is_empty() {
            return configured.split_whitespace().map(String::from).collect();
        }
        #[cfg(windows)]
        let fallback = std::env::var("COMSPEC").unwrap_or_else(|_| "cmd".into());
        #[cfg(not(windows))]
        let fallback = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
        vec![fallback]
    }

    /// Where a new session's child should start. The file panel's cwd is a
    /// real directory most of the time, but it can equally be a virtual
    /// location (`@/settings`, `@/pty`) that no process can chdir into — fall
    /// back to the home directory there rather than letting the spawn fail.
    fn start_dir(&self, state: &AppState) -> PathBuf {
        if crate::vfs::is_virtual(&state.cwd) {
            return home_dir();
        }
        state.cwd.clone()
    }

    // ── attach / detach ───────────────────────────────────────────────────

    /// Put `id` on screen without touching the keyboard. Stepping through
    /// sessions with `h`/`l` is browsing, not resuming: grabbing the keyboard
    /// back on each step would make looking at the next terminal cost an escape.
    fn show(&self, id: SessionId, state: &AppState) -> Vec<Intent> {
        if self.sessions.borrow().get(id).is_none() {
            return Vec::new();
        }
        self.attached.set(Some(id));
        *self.cursor_when_shown.borrow_mut() = state.cursor_path();
        vec![Intent::SetFunctionForm(Some(ID.into()))]
    }

    /// Claim the panel and the keyboard for `id`.
    fn attach(&self, id: SessionId, state: &AppState) -> Vec<Intent> {
        let escape = {
            let table = self.sessions.borrow();
            let Some(s) = table.get(id) else { return Vec::new() };
            s.origin.escape_class().intents()
        };
        self.attached.set(Some(id));
        *self.cursor_when_shown.borrow_mut() = state.cursor_path();
        vec![
            Intent::SetFunctionForm(Some(ID.into())),
            Intent::SetInputCapture(Some(Box::new(InputCapture {
                owner: ID.into(),
                release: self.prefix_key(),
                escape,
            }))),
        ]
    }

    /// Give the **keyboard** back but keep the session on screen. This is what
    /// the prefix key and a child's exit do: the panel stays, so its output is
    /// still readable and scrolls under the ordinary function-panel keys, and
    /// the file panel is usable again. Safe to emit when core has already
    /// dropped the capture itself.
    fn release(&self) -> Vec<Intent> {
        vec![Intent::SetInputCapture(None)]
    }

    /// Give the panel back as well, dropping the session from view.
    fn close(&self) -> Vec<Intent> {
        self.attached.set(None);
        *self.cursor_when_shown.borrow_mut() = None;
        vec![Intent::SetInputCapture(None), Intent::SetFunctionForm(None)]
    }

    // ── spawning ──────────────────────────────────────────────────────────

    fn spawn(
        &self,
        origin: Origin,
        label: String,
        argv: Vec<String>,
        cwd: PathBuf,
        state: &AppState,
    ) -> Vec<Intent> {
        let Some(tx) = self.tx.get() else {
            return vec![Intent::Notify("pty: no event channel".into())];
        };
        let max = self.opt_u32("pty.max-sessions", 8) as usize;
        let mut table = self.sessions.borrow_mut();
        if table.running_count() >= max {
            // Refuse rather than evict: a cap on consoles must not become a way
            // to silently kill a running job.
            return vec![Intent::Notify(format!("pty: {max} sessions already running"))];
        }
        let id = table.next_id();
        let (cols, rows) = self.viewport.get();
        let scrollback = self.opt_u32("pty.scrollback", 2000) as usize;
        match Session::spawn(
            id,
            origin,
            label,
            &argv,
            &cwd,
            cols,
            rows,
            scrollback,
            tx.clone(),
        ) {
            Ok(s) => {
                table.insert(s);
                drop(table);
                self.attach(id, state)
            }
            Err(e) => vec![Intent::Notify(format!("pty: {e}"))],
        }
    }

    fn new_shell(&self, state: &AppState) -> Vec<Intent> {
        let argv = self.shell_argv();
        let label = argv.first().cloned().unwrap_or_default();
        self.spawn(Origin::Shell, label, argv, self.start_dir(state), state)
    }

    /// Hand the resumed shell the directory the file cursor is pointing into,
    /// as pasted text at its prompt.
    ///
    /// It is *pasted*, not run: nothing is executed, no history is written, and
    /// no shell-specific quoting or command name is needed. The user decides
    /// what the path is for.
    ///
    /// The directory is the one **containing the cursor's entry**, not the file
    /// panel's `cwd`: after `f` the cursor can be sitting on a match from
    /// somewhere else entirely, and that match's own directory is the useful
    /// answer.
    fn offer_cursor_dir(&self, id: SessionId, state: &AppState) {
        let Some(dir) = cursor_dir(state) else { return };
        let table = self.sessions.borrow();
        let Some(s) = table.get(id) else { return };
        // A busy shell would take this as stdin for whatever is running.
        if !s.at_prompt() {
            return;
        }
        s.paste(&dir.to_string_lossy());
    }

    /// The `"run"` payload CmdOp (and the Edit override) sends.
    fn run_from_data(&self, data: &ExtensionValue, state: &AppState) -> Vec<Intent> {
        let ExtensionValue::Map(m) = data else { return Vec::new() };
        let argv: Vec<String> = match m.get("argv") {
            Some(ExtensionValue::List(items)) => {
                items.iter().filter_map(|v| v.as_str().map(String::from)).collect()
            }
            _ => Vec::new(),
        };
        if argv.is_empty() {
            return vec![Intent::Notify("pty: empty command".into())];
        }
        let label = m
            .get("label")
            .and_then(|v| v.as_str())
            .map(String::from)
            .unwrap_or_else(|| argv.join(" "));
        let cwd = m
            .get("cwd")
            .and_then(|v| v.as_path().cloned())
            .unwrap_or_else(|| state.cwd.clone());
        let origin = match m.get("path").and_then(|v| v.as_path()) {
            Some(p) => Origin::Edit { path: p.clone() },
            None => Origin::Command,
        };
        self.spawn(origin, label, argv, cwd, state)
    }

    // ── per-frame housekeeping ────────────────────────────────────────────

    /// Announce any child that has exited since the last frame, refresh the
    /// `@/pty` snapshot, and apply `close-on-exit`. Driven from `view` /
    /// `render_form` because a child exits on a background thread with nothing
    /// to notify the reducer.
    fn reap(&self) -> Vec<Intent> {
        let mut out = Vec::new();
        let mut table = self.sessions.borrow_mut();
        let mut reported = self.reported.borrow_mut();
        for (id, label, ok, origin) in table.take_newly_exited(&mut reported) {
            out.push(Intent::Notify(format!("{label}: {}", if ok { "ok" } else { "failed" })));
            crate::services::messages::log(format!("pty {id}: {label} finished"), !ok);
            // A file edited in the panel has a new size and mtime; the Suspend
            // path already refreshes for exactly this reason.
            if matches!(origin, Origin::Edit { .. }) {
                out.push(Intent::Refresh);
            }
            if self.attached.get() == Some(id) {
                // The keyboard always comes back — a dead child cannot use it.
                // Whether the *panel* also goes is the user's choice; keeping it
                // leaves the final output readable and scrollable.
                let close = match self.opt_str("pty.close-on-exit").as_str() {
                    "always" => true,
                    "success" => ok,
                    _ => false,
                };
                drop(table);
                drop(reported);
                out.extend(if close { self.close() } else { self.release() });
                self.sessions.borrow().publish();
                return out;
            }
        }
        table.evict_exited(self.opt_u32("pty.keep-exited", 4) as usize);
        table.publish();
        out
    }

    /// The attached session, if it still exists. A vanished session must not
    /// leave the capture installed — though the prefix key would free the user
    /// regardless, which is why this is tidiness rather than the guarantee.
    fn attached_ok(&self) -> Option<SessionId> {
        let id = self.attached.get()?;
        self.sessions.borrow().get(id).is_some().then_some(id)
    }

    fn title(&self, id: SessionId) -> String {
        let table = self.sessions.borrow();
        match table.get(id) {
            Some(s) => format!("pty {}: {}{}", s.id, s.label, s.exit_label()),
            None => "pty".into(),
        }
    }
}

impl Extension for PtyExtension {
    fn id(&self) -> &str {
        ID
    }

    fn about(&self) -> About {
        About::new("PTY", "Terminal sessions inside the function panel")
    }

    fn scheme_source(&self) -> Option<String> {
        Some(KEYMAP_SCHEME.to_string())
    }

    fn option_scheme(&self) -> Option<String> {
        Some(OPTIONS_SCHEME.to_string())
    }

    fn commands(&self) -> Vec<CommandDef> {
        // The release key detaches outright rather than opening a submenu, so
        // session management lives in the palette instead of behind a prefix.
        ["next", "prev", "kill", "detach"]
            .into_iter()
            .map(|name| CommandDef {
                name: format!("pty-{name}"),
                description: format!("pty: {name} session"),
                intent: Intent::Extension(ExtensionIntent {
                    extension: ID.into(),
                    intent: name.into(),
                    data: ExtensionValue::Nil,
                }),
            })
            .collect()
    }

    fn bind_store(&self, store: &Arc<OptionStore>) {
        let _ = self.store.set(store.clone());
    }

    fn register_conditions(&self, c: &mut crate::script::condition::Conditions) {
        // A session on screen whose keyboard has been released: the state where
        // the panel scrolls like any other and Enter takes the keyboard back.
        c.register(
            "pty-released",
            Box::new(|s| {
                s.function.form_owner.as_deref() == Some(ID) && s.input_capture.is_none()
            }),
        );
    }

    fn bind_events(&self, tx: Sender<AppEvent>) {
        let _ = self.tx.set(tx);
    }

    fn vfs_providers(&self) -> Vec<Box<dyn VfsProvider>> {
        vec![Box::new(PtyProvider::new(ID.into(), self.sessions.borrow().snapshot()))]
    }

    fn handle_intent(&self, intent: &ExtensionIntent, state: &AppState) -> Vec<Intent> {
        match intent.intent.as_str() {
            // Every captured key that is not one of core's two escapes. By the
            // time this runs the decision has already been made, so there is
            // nothing to check here: the key is the child's.
            "key" => {
                let Some(key) = codec::decode_key(&intent.data) else { return Vec::new() };
                let Some(id) = self.attached_ok() else { return self.close() };
                let table = self.sessions.borrow();
                let Some(s) = table.get(id) else { return Vec::new() };
                if !s.send_key(key) {
                    drop(table);
                    return self.reap();
                }
                Vec::new()
            }
            // Core has *already* cleared the capture; the prefix key means
            // "give me the file panel back", nothing more. It deliberately does
            // not open a submenu: an extra key to dismiss would make the one
            // guaranteed way out of a captured keyboard cost two presses.
            "prefix" => self.release(),
            "attach" => match intent.data.as_int() {
                Some(id) => self.attach(id as SessionId, state),
                None => Vec::new(),
            },
            "run" => self.run_from_data(&intent.data, state),
            // Core's `e` (StartEdit → Plan::Suspend) hands the editor the whole
            // terminal; this runs it in the panel instead. Same target rule, so
            // the two behave identically when there is not exactly one.
            "edit" => {
                let targets = state.targets();
                let [path] = targets.as_slice() else {
                    return vec![Intent::Notify("edit needs exactly one target".into())];
                };
                let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".into());
                let mut argv: Vec<String> = editor.split_whitespace().map(String::from).collect();
                argv.push(path.to_string_lossy().into_owned());
                let label = argv.join(" ");
                self.spawn(Origin::Edit { path: path.clone() }, label, argv, self.start_dir(state), state)
            }
            // `x`: pick up the shell already open, or start one if there is
            // none. Attaching (not just showing) is right here — pressing the
            // shell key means "let me type in it".
            "shell" => {
                let existing = self.sessions.borrow().newest_running(&Origin::Shell);
                match existing {
                    Some(id) => {
                        self.offer_cursor_dir(id, state);
                        self.attach(id, state)
                    }
                    None => self.new_shell(state),
                }
            }
            // `X`: always another one.
            "new-shell" => self.new_shell(state),
            "detach" => self.close(),
            "next" | "prev" => {
                let target = self
                    .sessions
                    .borrow()
                    .neighbour(self.attached.get(), intent.intent == "next");
                match target {
                    // Preserve the keyboard state: released stays released.
                    Some(id) if state.input_capture.is_none() => self.show(id, state),
                    Some(id) => self.attach(id, state),
                    None => self.close(),
                }
            }
            "reattach" => match self.attached_ok() {
                Some(id) => self.attach(id, state),
                None => Vec::new(),
            },
            "kill" => {
                let Some(id) = self.attached_ok() else { return Vec::new() };
                if let Some(s) = self.sessions.borrow_mut().get_mut(id) {
                    s.kill();
                }
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    fn render_form(&self, frame: &mut Frame, inner: Rect, ctx: &FunctionRenderCtx) -> Option<String> {
        self.viewport.set((inner.width.max(1), inner.height.max(1)));
        let id = self.attached_ok()?;
        let table = self.sessions.borrow();
        let s = table.get(id)?;
        s.resize(inner.width, inner.height);
        s.shared.dirty.store(false, std::sync::atomic::Ordering::Release);

        // Core's panel scroll counts rows from the top of the content; vt100
        // counts rows *back* from the live screen. They meet in the middle here,
        // so the pty scrolls under the same k/j the rest of the panel uses.
        let rows = inner.height.max(1) as usize;
        let (available, position) = s.scroll_to(ctx.func.scroll.offset());
        self.scroll_extent.set((available + rows, position));

        // Never unwrap a lock here: a poisoned parser would take the whole UI
        // down on every frame instead of losing one session.
        let parser = s.shared.parser.lock().ok()?;
        let term = PseudoTerminal::new(parser.screen())
            .cursor(Cursor::default().visibility(ctx.focused && s.running()));
        frame.render_widget(&term, inner);
        Some(self.title(id))
    }

    fn on_cursor_changed(&self, state: &AppState) -> Vec<Intent> {
        let released = state.function.form_owner.as_deref() == Some(ID)
            && state.input_capture.is_none();
        if !released {
            return Vec::new();
        }
        // Only a *move* means the user is back to browsing files. The broadcast
        // itself does not: its signature carries the function panel's scroll
        // offset, so scrolling the terminal with `j`/`k` fires it too, and
        // treating that as a move made the panel flip to the preview mid-scroll.
        if state.cursor_path() == *self.cursor_when_shown.borrow() {
            return Vec::new();
        }
        self.close()
    }

    fn form_scroll(&self, _ctx: &FunctionRenderCtx) -> Option<(usize, usize)> {
        self.attached_ok()?;
        Some(self.scroll_extent.get())
    }

    fn on_tick(&self, _state: &AppState) -> Vec<Intent> {
        // A child exits on the reader thread, leaving no trace in AppState, so
        // the cursor-changed broadcast (deduplicated on the cursor) would never
        // fire for it. This is the hook that runs anyway.
        if self.sessions.borrow().ids().is_empty() {
            return Vec::new();
        }
        let mut out = self.reap();
        // New output jumps to the live tail — a terminal that kept showing old
        // scrollback while the child wrote would look frozen. Core clamps the
        // saturating value down to the real extent.
        if let Some(id) = self.attached_ok()
            && self
                .sessions
                .borrow()
                .get(id)
                .is_some_and(|s| s.shared.dirty.load(std::sync::atomic::Ordering::Acquire))
        {
            out.push(Intent::SetScroll(usize::MAX));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ext() -> PtyExtension {
        PtyExtension::new()
    }

    #[test]
    fn the_default_prefix_is_ctrl_b() {
        assert_eq!(ext().prefix_key(), Key::new(KeyCode::Char('b'), Mods::CTRL));
    }

    /// An extension bound to a store whose `pty.prefix-key` default is `value`.
    fn ext_with_prefix(value: &str) -> PtyExtension {
        use crate::core::{OptionDef, OptionKind};
        let e = ext();
        e.bind_store(&Arc::new(OptionStore::new(vec![OptionDef {
            key: "pty.prefix-key".into(),
            category: "Pty".into(),
            label: "prefix-key".into(),
            kind: OptionKind::Text,
            default: crate::core::OptionValue::Text(value.into()),
            owner: Some(ID.into()),
        }])));
        e
    }

    #[test]
    fn a_prefix_without_ctrl_is_refused_so_the_letter_stays_sendable() {
        assert_eq!(
            ext_with_prefix("x").prefix_key(),
            Key::new(KeyCode::Char('b'), Mods::CTRL),
            "a bare letter as prefix would make that letter unreachable in every child"
        );
    }

    #[test]
    fn an_unparseable_prefix_falls_back_rather_than_leaving_no_way_out() {
        assert_eq!(
            ext_with_prefix("Ctrl-nonsense").prefix_key(),
            Key::new(KeyCode::Char('b'), Mods::CTRL)
        );
    }

    #[test]
    fn a_configured_ctrl_prefix_is_honoured() {
        assert_eq!(
            ext_with_prefix("Ctrl-a").prefix_key(),
            Key::new(KeyCode::Char('a'), Mods::CTRL)
        );
    }

    #[test]
    fn releasing_gives_back_the_keyboard_but_keeps_the_session_on_screen() {
        // The panel has to stay for the scrollbar to mean anything: that is how
        // a released session scrolls like every other function-panel frame.
        let e = ext();
        e.attached.set(Some(1));
        let out = e.release();
        assert_eq!(out, vec![Intent::SetInputCapture(None)]);
        assert_eq!(e.attached.get(), Some(1), "still attached, just not captured");
    }

    #[test]
    fn closing_gives_back_the_panel_too() {
        let e = ext();
        e.attached.set(Some(1));
        let out = e.close();
        assert!(e.attached.get().is_none());
        assert!(out.contains(&Intent::SetInputCapture(None)));
        assert!(out.contains(&Intent::SetFunctionForm(None)));
    }

    #[test]
    fn a_key_for_a_vanished_session_detaches_instead_of_stranding_the_capture() {
        let e = ext();
        e.attached.set(Some(99)); // never inserted
        let out = e.handle_intent(
            &ExtensionIntent {
                extension: ID.into(),
                intent: "key".into(),
                data: codec::encode_key(Key::plain(KeyCode::Char('a'))),
            },
            &AppState::new(PathBuf::from("/")),
        );
        assert!(out.contains(&Intent::SetInputCapture(None)));
    }

    /// A render context standing in for a panel that has not been drawn.
    fn ctx<'a>() -> FunctionRenderCtx<'a> {
        use std::sync::OnceLock as Once;
        static FUNC: Once<crate::core::state::FunctionPanelState> = Once::new();
        static THEME: Once<crate::core::Theme> = Once::new();
        FunctionRenderCtx {
            width: 80,
            height: 24,
            focused: true,
            func: FUNC.get_or_init(Default::default),
            sources: &[],
            theme: THEME.get_or_init(crate::core::Theme::default),
        }
    }

    fn ei(name: &str) -> ExtensionIntent {
        ExtensionIntent { extension: ID.into(), intent: name.into(), data: ExtensionValue::Nil }
    }

    #[test]
    fn the_prefix_key_hands_the_panel_straight_back_with_no_submenu() {
        // Core has already dropped the capture by then; a submenu here would
        // make the one guaranteed way out cost a second keystroke.
        let e = ext();
        e.attached.set(Some(1));
        let out = e.handle_intent(&ei("prefix"), &AppState::new(PathBuf::from("/")));
        assert_eq!(out, vec![Intent::SetInputCapture(None)]);
        assert!(!out.iter().any(|i| matches!(i, Intent::PushMode(_))), "no mode is pushed");
        assert_eq!(e.attached.get(), Some(1), "the session stays visible and scrollable");
    }

    #[test]
    fn a_shell_opened_on_a_virtual_path_starts_at_home_instead_of_failing() {
        let e = ext();
        let mut s = AppState::new(crate::vfs::vfs_root().join("settings"));
        assert_eq!(e.start_dir(&s), home_dir(), "no process can chdir into @/settings");
        s.cwd = PathBuf::from("/tmp");
        assert_eq!(e.start_dir(&s), PathBuf::from("/tmp"), "a real directory is used as-is");
    }

    #[test]
    fn the_scroll_extent_covers_the_scrollback_plus_the_live_screen() {
        // Nothing has been drawn, so this is the "no session" answer.
        assert!(ext().form_scroll(&ctx()).is_none());
    }

    /// A file panel sitting on `name`, with a second entry to move to, and the
    /// pty already holding the panel.
    fn state_on(name: &str) -> AppState {
        let mut s = AppState::new(PathBuf::from("/d"));
        s.entries = vec![
            crate::core::Entry::virtual_leaf(PathBuf::from("/d/a.txt"), "a.txt".into()),
            crate::core::Entry::virtual_leaf(PathBuf::from("/d/b.txt"), "b.txt".into()),
        ];
        s.cursor = usize::from(name == "b.txt");
        s.function.form_owner = Some(ID.into());
        s
    }

    #[test]
    fn moving_the_file_cursor_while_released_hands_the_panel_back_to_the_preview() {
        let e = ext();
        e.attached.set(Some(1));
        *e.cursor_when_shown.borrow_mut() = state_on("a.txt").cursor_path();
        assert!(
            e.on_cursor_changed(&state_on("b.txt")).contains(&Intent::SetFunctionForm(None)),
            "released + cursor moved means the user is browsing files again"
        );
    }

    #[test]
    fn scrolling_the_terminal_does_not_hand_the_panel_back() {
        // The cursor-changed signature carries the panel's scroll offset, so
        // `j`/`k` fire the broadcast exactly as a cursor move does. Only the
        // target tells them apart — without that, scrolling flipped the panel
        // to the preview mid-scroll.
        let e = ext();
        let mut s = state_on("a.txt");
        e.attached.set(Some(1));
        *e.cursor_when_shown.borrow_mut() = s.cursor_path();

        s.function.scroll.advance(5);
        assert!(
            e.on_cursor_changed(&s).is_empty(),
            "scrolling is not browsing; the terminal stays on screen"
        );
        assert_eq!(e.attached.get(), Some(1));
    }

    #[test]
    fn a_captured_session_never_steps_aside() {
        let e = ext();
        let mut s = state_on("a.txt");
        s.input_capture = Some(InputCapture {
            owner: ID.into(),
            release: Key::new(KeyCode::Char('b'), Mods::CTRL),
            escape: Vec::new(),
        });
        e.attached.set(Some(1));
        assert!(e.on_cursor_changed(&s).is_empty());
    }

    #[test]
    fn the_offered_directory_is_the_cursor_entrys_parent_not_the_listing() {
        // After `f` the cursor can sit on a match from another directory, and
        // that match's own directory is the useful answer — not `state.cwd`.
        let mut s = AppState::new(PathBuf::from("/listing"));
        assert_eq!(cursor_dir(&s), Some(PathBuf::from("/")), "no entry: the listing's own parent");

        s.entries = vec![crate::core::Entry::virtual_leaf(
            PathBuf::from("/elsewhere/deep/hit.txt"),
            "hit.txt".into(),
        )];
        s.cursor = 0;
        assert_eq!(cursor_dir(&s), Some(PathBuf::from("/elsewhere/deep")));
    }

    #[test]
    fn a_directory_under_the_cursor_offers_the_directory_holding_it() {
        let mut s = AppState::new(PathBuf::from("/a"));
        s.entries =
            vec![crate::core::Entry::virtual_dir(PathBuf::from("/a/b/d"), "d".into())];
        s.cursor = 0;
        assert_eq!(cursor_dir(&s), Some(PathBuf::from("/a/b")));
    }

    #[test]
    fn a_virtual_location_offers_nothing_because_no_shell_can_use_it() {
        let s = AppState::new(crate::vfs::vfs_root().join("settings"));
        assert_eq!(cursor_dir(&s), None);
    }

    #[test]
    fn session_management_is_reachable_from_the_palette() {
        let names: Vec<String> = ext().commands().into_iter().map(|c| c.name).collect();
        for want in ["pty-next", "pty-prev", "pty-kill", "pty-detach"] {
            assert!(names.contains(&want.to_string()), "{want} missing from {names:?}");
        }
    }

    #[test]
    fn run_with_no_argv_reports_rather_than_spawning() {
        let e = ext();
        let out = e.run_from_data(
            &ExtensionValue::Map(Default::default()),
            &AppState::new(PathBuf::from("/")),
        );
        assert!(matches!(out.as_slice(), [Intent::Notify(_)]));
    }
}
