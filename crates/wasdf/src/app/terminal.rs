//! Terminal init/restore and the Suspended execution form (the user's editor).
//! Converts crossterm key events into the core Key type at the boundary.

use std::io;

use crossterm::event::{KeyCode as CtKeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::DefaultTerminal;

use crate::core::{Key, KeyCode, Mods};

/// Enter the alternate screen and raw mode, returning the ratatui terminal.
pub fn init() -> DefaultTerminal {
    ratatui::init()
}

/// Leave raw mode and the alternate screen.
pub fn restore() {
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
    if ev.kind == KeyEventKind::Release {
        return None;
    }
    let m = ev.modifiers;
    let code = match ev.code {
        CtKeyCode::Char(c) => KeyCode::Char(c),
        CtKeyCode::Enter => KeyCode::Enter,
        CtKeyCode::Esc => KeyCode::Esc,
        CtKeyCode::Tab => KeyCode::Tab,
        CtKeyCode::BackTab => KeyCode::BackTab,
        CtKeyCode::Backspace => KeyCode::Backspace,
        CtKeyCode::Delete => KeyCode::Delete,
        CtKeyCode::Left => KeyCode::Left,
        CtKeyCode::Right => KeyCode::Right,
        CtKeyCode::Up => KeyCode::Up,
        CtKeyCode::Down => KeyCode::Down,
        CtKeyCode::Home => KeyCode::Home,
        CtKeyCode::End => KeyCode::End,
        CtKeyCode::PageUp => KeyCode::PageUp,
        CtKeyCode::PageDown => KeyCode::PageDown,
        _ => return None,
    };
    // Shift is folded into the character; only keep it for non-char keys.
    let shift = m.contains(KeyModifiers::SHIFT) && !matches!(code, KeyCode::Char(_));
    let mods = Mods {
        ctrl: m.contains(KeyModifiers::CONTROL),
        shift,
        alt: m.contains(KeyModifiers::ALT),
    };
    Some(Key { code, mods })
}
