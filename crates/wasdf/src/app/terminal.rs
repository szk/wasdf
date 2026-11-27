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
    // Once, in raw mode, before crossterm's reader owns stdin.
    GRAPHICS.get_or_init(probe_graphics);
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

static GRAPHICS: std::sync::OnceLock<crate::ui::graphics::Support> = std::sync::OnceLock::new();

/// Which image protocols this terminal draws, probed once by [`init`].
pub fn graphics_support() -> crate::ui::graphics::Support {
    GRAPHICS.get().copied().unwrap_or_default()
}

/// Ask the terminal: a kitty graphics query (answered `…;OK` only by a
/// terminal that implements it) followed by DA1 (answered by every terminal;
/// a `4` among its parameters means sixel). DA1's reply ends the wait, so a
/// terminal that ignores the kitty query costs no timeout; one that answers
/// nothing at all costs at most the deadline. Keys typed in that instant are
/// lost — the probe runs once, at start.
fn probe_graphics() -> crate::ui::graphics::Support {
    use std::io::Write;
    use std::time::{Duration, Instant};
    let none = crate::ui::graphics::Support::default();
    // SAFETY: isatty has no preconditions.
    if unsafe { libc::isatty(0) != 1 || libc::isatty(1) != 1 } {
        return none;
    }
    let mut out = io::stdout();
    if out.write_all(b"\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\\x1b[c").and_then(|_| out.flush()).is_err() {
        return none;
    }
    let deadline = Instant::now() + Duration::from_millis(500);
    let mut buf = Vec::new();
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        let mut pfd = libc::pollfd { fd: 0, events: libc::POLLIN, revents: 0 };
        // SAFETY: one valid pollfd; read into a stack buffer of its length.
        let n = unsafe {
            if libc::poll(&mut pfd, 1, left.as_millis() as libc::c_int) <= 0 {
                break;
            }
            let mut chunk = [0u8; 256];
            let n = libc::read(0, chunk.as_mut_ptr() as *mut libc::c_void, chunk.len());
            if n > 0 {
                buf.extend_from_slice(&chunk[..n as usize]);
            }
            n
        };
        if n <= 0 || da1_params(&buf).is_some() {
            break;
        }
    }
    parse_graphics_replies(&buf)
}

/// The parameters of a DA1 reply (`ESC [ ? p ; p … c`) in `buf`, if complete.
fn da1_params(buf: &[u8]) -> Option<&[u8]> {
    let start = buf.windows(3).position(|w| w == b"\x1b[?")? + 3;
    let len = buf[start..].iter().position(|&b| b == b'c')?;
    Some(&buf[start..start + len])
}

fn parse_graphics_replies(buf: &[u8]) -> crate::ui::graphics::Support {
    let kitty = buf.windows(9).any(|w| w == b"_Gi=31;OK");
    let sixel = da1_params(buf).is_some_and(|p| p.split(|&b| b == b';').any(|v| v == b"4"));
    crate::ui::graphics::Support { kitty, sixel }
}

/// The terminal's cell size in pixels, from the window size it reports
/// (TIOCGWINSZ). `None` when it reports no pixels — graphics then stay off.
pub fn cell_px(term: &mut DefaultTerminal) -> Option<(u16, u16)> {
    use ratatui::backend::Backend;
    let ws = term.backend_mut().window_size().ok()?;
    let (c, r) = (ws.columns_rows.width, ws.columns_rows.height);
    let (w, h) = (ws.pixels.width, ws.pixels.height);
    (c > 0 && r > 0 && w >= c && h >= r).then(|| (w / c, h / r))
}

/// Whether the terminal runs elsewhere (ssh), so it cannot read our shared
/// memory and kitty pixels must travel inline.
pub fn remote() -> bool {
    std::env::var_os("SSH_CONNECTION").is_some() || std::env::var_os("SSH_TTY").is_some()
}

/// Write graphics control strings (ui/graphics.rs) after the draw — the only
/// bytes images cost on the tty; kitty pixels travel through shared memory.
pub fn write_graphics(term: &mut DefaultTerminal, cmds: &[crate::ui::graphics::Cmd]) -> io::Result<()> {
    use crossterm::cursor::MoveTo;
    use crossterm::queue;
    use crossterm::style::Print;
    use std::io::Write;
    if cmds.is_empty() {
        return Ok(());
    }
    let out = term.backend_mut();
    for c in cmds {
        if let Some((x, y)) = c.at {
            queue!(out, MoveTo(x, y))?;
        }
        queue!(out, Print(&c.data))?;
    }
    out.flush()
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
    // Releases are kept here: a PTY child that enabled the Kitty
    // protocol's event types expects them. `App::handle_key` drops them for
    // everything but an input capture.
    let kind = match ev.kind {
        KeyEventKind::Press => KeyKind::Press,
        KeyEventKind::Repeat => KeyKind::Repeat,
        KeyEventKind::Release => KeyKind::Release,
    };
    Some(Key { code, mods, kind })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replies_tell_kitty_and_sixel_apart() {
        let s = parse_graphics_replies(b"\x1b_Gi=31;OK\x1b\\\x1b[?62;4;22c");
        assert!(s.kitty && s.sixel);
        let s = parse_graphics_replies(b"\x1b[?62;22c");
        assert!(!s.kitty && !s.sixel, "a DA1 without 4, and no kitty answer");
        let s = parse_graphics_replies(b"\x1b_Gi=31;EINVAL:bad\x1b\\\x1b[?1;2c");
        assert!(!s.kitty, "an error reply is not support");
        assert!(da1_params(b"\x1b[?62;4").is_none(), "an unfinished reply keeps waiting");
        assert!(!parse_graphics_replies(b"\x1b[?14;2c").sixel, "14 is not 4");
    }
}
