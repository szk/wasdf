//! Terminal init/restore and the Suspended execution form (the user's editor).
//! Converts crossterm key events into the core Key type at the boundary.

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};

use crossterm::event::{
    KeyCode as CtKeyCode, KeyEvent, KeyEventKind, KeyModifiers, KeyboardEnhancementFlags,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::supports_keyboard_enhancement;
use ratatui::DefaultTerminal;

use crate::core::{Key, KeyCode, KeyKind, Mods};
use crate::services::messages;

/// The Kitty keyboard-protocol flags wasdf asks its own terminal for. Without
/// these, `Ctrl-i` is indistinguishable from Tab, releases never arrive, and
/// the super/hyper/meta modifiers are invisible — so a PTY child could never be
/// driven faithfully no matter how good the encoder is (doc/PTY.md §C2).
const KITTY_FLAGS: KeyboardEnhancementFlags = KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
    .union(KeyboardEnhancementFlags::REPORT_EVENT_TYPES)
    .union(KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS)
    .union(KeyboardEnhancementFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES);

/// Whether [`init`] managed to push [`KITTY_FLAGS`], so [`restore`] knows
/// whether it owes the terminal a pop. Process-global rather than threaded
/// through a handle because `init`/`restore` are already free functions called
/// from three places (boot, teardown, and each suspended child).
static KITTY_PUSHED: AtomicBool = AtomicBool::new(false);

/// Enter the alternate screen and raw mode, returning the ratatui terminal.
pub fn init() -> DefaultTerminal {
    let term = ratatui::init();
    // Best-effort. A terminal without the protocol (every Windows console, and
    // plenty of Unix ones) is left alone and everything degrades to legacy
    // keys — disable, notify, continue.
    if matches!(supports_keyboard_enhancement(), Ok(true)) {
        match execute!(io::stdout(), PushKeyboardEnhancementFlags(KITTY_FLAGS)) {
            Ok(()) => KITTY_PUSHED.store(true, Ordering::Release),
            Err(e) => messages::global()
                .push(format!("keyboard protocol unavailable: {e}"), false),
        }
    }
    term
}

/// Leave raw mode and the alternate screen.
pub fn restore() {
    // Pop before leaving the alternate screen, mirroring the push order, so the
    // shell we hand back is in the mode it started in.
    if KITTY_PUSHED.swap(false, Ordering::AcqRel) {
        let _ = execute!(io::stdout(), PopKeyboardEnhancementFlags);
    }
    ratatui::restore();
}

/// Suspended form: drop the TUI, run the child in the foreground inheriting the
/// cooked terminal, then re-enter the TUI. The caller issues Refresh afterward.
pub fn run_suspended(argv: &[String], cwd: &std::path::Path) -> io::Result<DefaultTerminal> {
    restore();
    let status = if argv.is_empty() {
        Ok(())
    } else {
        std::process::Command::new(&argv[0])
            .args(&argv[1..])
            .current_dir(cwd)
            .status()
            .map(|_| ())
    };
    let term = init();
    // Re-entering the alternate screen makes the terminal answer the color /
    // device-attributes queries it was sent (by the editor, and by our own
    // re-init). Those replies arrive a beat *after* we are back, and if read as
    // keys they open spurious modes: the trailing `c` of a DA reply (`ESC[?…c`)
    // starts copy, and the OSC color bytes (`]10;rgb:…`) then fill its argument.
    // Drain them before the input thread resumes.
    drain_pending_input();
    status?;
    Ok(term)
}

/// Swallow the terminal-reply bytes a suspended child leaves behind. The replies
/// can land slightly after we re-enter the TUI, so we drain through crossterm —
/// its own fd and escape-sequence parser, so partial sequences are consumed
/// whole — for a short fixed window rather than stopping at the first quiet
/// moment. Safe because the input thread is paused for the entire suspended
/// span, so no genuine keystroke is discarded here.
fn drain_pending_input() {
    use std::time::{Duration, Instant};
    let deadline = Instant::now() + Duration::from_millis(500);
    while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
        match crossterm::event::poll(remaining) {
            Ok(true) => {
                if crossterm::event::read().is_err() {
                    break;
                }
            }
            // Timed out with the window elapsed, or a poll error — either way stop.
            _ => break,
        }
    }
}

/// Convert a crossterm key event into a core Key. Returns None for events that
/// are not key presses or carry no representable code.
pub fn convert_key(ev: KeyEvent) -> Option<Key> {
    let m = ev.modifiers;
    let code = match ev.code {
        CtKeyCode::Char(c) => KeyCode::Char(c),
        CtKeyCode::Enter => KeyCode::Enter,
        CtKeyCode::Esc => KeyCode::Esc,
        CtKeyCode::Tab => KeyCode::Tab,
        CtKeyCode::BackTab => KeyCode::BackTab,
        CtKeyCode::Backspace => KeyCode::Backspace,
        CtKeyCode::Delete => KeyCode::Delete,
        CtKeyCode::Insert => KeyCode::Insert,
        CtKeyCode::Left => KeyCode::Left,
        CtKeyCode::Right => KeyCode::Right,
        CtKeyCode::Up => KeyCode::Up,
        CtKeyCode::Down => KeyCode::Down,
        CtKeyCode::Home => KeyCode::Home,
        CtKeyCode::End => KeyCode::End,
        CtKeyCode::PageUp => KeyCode::PageUp,
        CtKeyCode::PageDown => KeyCode::PageDown,
        CtKeyCode::F(n) => KeyCode::F(n),
        _ => return None,
    };
    // Shift is folded into the character; only keep it for non-char keys.
    let shift = m.contains(KeyModifiers::SHIFT) && !matches!(code, KeyCode::Char(_));
    let mods = Mods {
        ctrl: m.contains(KeyModifiers::CONTROL),
        shift,
        alt: m.contains(KeyModifiers::ALT),
        sup: m.contains(KeyModifiers::SUPER),
        hyper: m.contains(KeyModifiers::HYPER),
        meta: m.contains(KeyModifiers::META),
    };
    // Releases are no longer dropped here: a PTY child that enabled the Kitty
    // protocol's event types expects them. Nothing else in the app sees one —
    // `KeymapRegistry::resolve` refuses to match a non-press.
    let kind = match ev.kind {
        KeyEventKind::Press => KeyKind::Press,
        KeyEventKind::Repeat => KeyKind::Repeat,
        KeyEventKind::Release => KeyKind::Release,
    };
    Some(Key { code, mods, kind })
}
