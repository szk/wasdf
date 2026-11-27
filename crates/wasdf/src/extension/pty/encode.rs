//! Turning a core [`Key`] into the bytes a PTY child expects.
//!
//! Two encodings, picked by the flags the child negotiated (see
//! [`super::kbmode`]): the xterm form every TUI understands, and the Kitty
//! CSI-u form for children that asked for it. The two use *different* modifier
//! weights — xterm has no super/hyper and puts meta at 8, Kitty puts super
//! there — which is the single easiest thing here to get wrong, so they are
//! separate functions rather than one with a branch inside.

use crate::core::{Key, KeyCode, KeyKind, Mods};

use super::kbmode::{ALL_KEYS, EVENT_TYPES};

const ESC: u8 = 0x1b;

/// The bytes to write to the child for `key`, given the keyboard `flags` it has
/// negotiated. `flags == 0` selects the legacy encoding. An empty result means
/// "send nothing" — a release the child never asked for, say.
pub fn encode(key: Key, flags: u8) -> Vec<u8> {
    if flags == 0 { legacy(key) } else { kitty(key, flags) }
}

// ── Legacy (xterm) ────────────────────────────────────────────────────────────

/// The xterm modifier parameter: `1 + shift + 2·alt + 4·ctrl + 8·meta`. Super
/// and hyper have no xterm encoding and are dropped.
fn xterm_mods(m: Mods) -> u8 {
    1 + u8::from(m.shift) + 2 * u8::from(m.alt) + 4 * u8::from(m.ctrl) + 8 * u8::from(m.meta)
}

fn legacy(key: Key) -> Vec<u8> {
    // A legacy terminal has no concept of a key release, so a child on this
    // encoding must never see one — it would read as a second press.
    if key.kind == KeyKind::Release {
        return Vec::new();
    }
    let m = key.mods;
    let param = xterm_mods(m);
    let modified = param > 1;

    let mut out = Vec::new();
    match key.code {
        KeyCode::Char(c) => {
            if m.alt {
                out.push(ESC);
            }
            match ctrl_byte(c) {
                Some(b) if m.ctrl => out.push(b),
                _ => out.extend(c.to_string().into_bytes()),
            }
        }
        // `\r`, not `\n`: the child's line discipline turns CR into NL, and a
        // raw NL leaves most shells waiting for more input.
        KeyCode::Enter => out.push(b'\r'),
        KeyCode::Tab => out.push(b'\t'),
        KeyCode::BackTab => out.extend(b"\x1b[Z"),
        KeyCode::Backspace => out.push(0x7f),
        KeyCode::Esc => out.push(ESC),
        code => match shape(code) {
            Some(Shape::Final { intro, f }) if !modified => out.extend([ESC, intro, f]),
            Some(Shape::Final { f, .. }) => out.extend(format!("\x1b[1;{param}{}", f as char).into_bytes()),
            Some(Shape::Tilde(n)) if !modified => out.extend(format!("\x1b[{n}~").into_bytes()),
            Some(Shape::Tilde(n)) => out.extend(format!("\x1b[{n};{param}~").into_bytes()),
            None => {}
        },
    }
    out
}

/// How a non-text key is spelled, shared by both encodings.
enum Shape {
    /// `ESC <intro> <f>` unmodified, `CSI 1 ; <mods> <f>` modified. F1–F4
    /// use SS3 (`O`) as the unmodified introducer — xterm's own split, which
    /// editors' termcap entries assume.
    Final { intro: u8, f: u8 },
    /// `CSI <n> ~`, with `; <mods>` when modified.
    Tilde(u16),
}

fn shape(code: KeyCode) -> Option<Shape> {
    let csi = |f| Shape::Final { intro: b'[', f };
    Some(match code {
        KeyCode::Up => csi(b'A'),
        KeyCode::Down => csi(b'B'),
        KeyCode::Right => csi(b'C'),
        KeyCode::Left => csi(b'D'),
        KeyCode::Home => csi(b'H'),
        KeyCode::End => csi(b'F'),
        KeyCode::F(n @ 1..=4) => Shape::Final { intro: b'O', f: b"PQRS"[(n - 1) as usize] },
        KeyCode::Insert => Shape::Tilde(2),
        KeyCode::Delete => Shape::Tilde(3),
        KeyCode::PageUp => Shape::Tilde(5),
        KeyCode::PageDown => Shape::Tilde(6),
        KeyCode::F(n) => Shape::Tilde(f_tilde_number(n)?),
        _ => return None,
    })
}

/// F5–F20's `CSI <n> ~` numbers. The sequence is deliberately gappy (no 16, no
/// 22) — that is xterm's table, not a typo. Beyond F20 there is no agreed
/// encoding, so those keys are dropped.
fn f_tilde_number(n: u8) -> Option<u16> {
    Some(match n {
        5 => 15,
        6 => 17,
        7 => 18,
        8 => 19,
        9 => 20,
        10 => 21,
        11 => 23,
        12 => 24,
        13 => 25,
        14 => 26,
        15 => 28,
        16 => 29,
        17 => 31,
        18 => 32,
        19 => 33,
        20 => 34,
        _ => return None,
    })
}

/// The control byte for `Ctrl-<c>`, or `None` when the pair has no C0 encoding
/// (`Ctrl-1`, say) and the character should be sent bare.
fn ctrl_byte(c: char) -> Option<u8> {
    match c {
        'a'..='z' => Some(c as u8 - b'a' + 1),
        'A'..='Z' => Some(c as u8 - b'A' + 1),
        '@' | ' ' => Some(0),
        '[' => Some(0x1b),
        '\\' => Some(0x1c),
        ']' => Some(0x1d),
        '^' => Some(0x1e),
        '_' => Some(0x1f),
        '?' => Some(0x7f),
        _ => None,
    }
}

// ── Kitty (CSI-u) ─────────────────────────────────────────────────────────────

/// The Kitty modifier parameter: `1 + shift + 2·alt + 4·ctrl + 8·super +
/// 16·hyper + 32·meta`. Note meta is 32 here and 8 in xterm.
fn kitty_mods(m: Mods) -> u8 {
    1 + u8::from(m.shift)
        + 2 * u8::from(m.alt)
        + 4 * u8::from(m.ctrl)
        + 8 * u8::from(m.sup)
        + 16 * u8::from(m.hyper)
        + 32 * u8::from(m.meta)
}

fn kitty(key: Key, flags: u8) -> Vec<u8> {
    let report_events = flags & EVENT_TYPES != 0;
    if key.kind != KeyKind::Press && !report_events {
        return Vec::new();
    }
    let all_keys = flags & ALL_KEYS != 0;
    let param = kitty_mods(key.mods);

    // Unmodified presses keep their legacy shape unless the child asked for
    // everything as escape codes. Sending CSI-u for a bare `a` to a child that
    // only set DISAMBIGUATE would put literal escape text on its screen.
    if !all_keys && param == 1 && key.kind == KeyKind::Press && !needs_csi_u(key.code) {
        return legacy(key);
    }

    // `;mods` is omitted when unmodified, but must be present as a placeholder
    // whenever an event type follows it.
    let event = report_events && key.kind != KeyKind::Press;
    let suffix = |param: u8| match (param > 1, event) {
        (_, true) => format!(";{param}:{}", event_type(key.kind)),
        (true, false) => format!(";{param}"),
        (false, false) => String::new(),
    };

    match key.code {
        KeyCode::Char(c) => csi_u(c as u32, &suffix(param)),
        KeyCode::Enter => csi_u(13, &suffix(param)),
        KeyCode::Tab => csi_u(9, &suffix(param)),
        // Kitty has no separate back-tab: it is Tab carrying shift, and the
        // shift has to be injected because core folds it into the code.
        KeyCode::BackTab => csi_u(9, &suffix(kitty_mods(Mods { shift: true, ..key.mods }))),
        KeyCode::Backspace => csi_u(127, &suffix(param)),
        KeyCode::Esc => csi_u(27, &suffix(param)),
        code => match shape(code) {
            // Unmodified, these still carry an explicit `;1`: `CSI 1 ; 1 <f>`.
            Some(Shape::Final { f, .. }) => {
                let s = suffix(param);
                let s = if s.is_empty() { ";1".to_string() } else { s };
                format!("\x1b[1{s}{}", f as char).into_bytes()
            }
            Some(Shape::Tilde(n)) => format!("\x1b[{n}{}~", suffix(param)).into_bytes(),
            None => Vec::new(),
        },
    }
}

fn csi_u(code: u32, suffix: &str) -> Vec<u8> {
    format!("\x1b[{code}{suffix}u").into_bytes()
}

fn event_type(kind: KeyKind) -> u8 {
    match kind {
        KeyKind::Press => 1,
        KeyKind::Repeat => 2,
        KeyKind::Release => 3,
    }
}

/// Codes with no legacy byte of their own, which therefore must go out as
/// CSI-u even for a bare unmodified press.
fn needs_csi_u(_code: KeyCode) -> bool {
    // Every code core can represent has a legacy encoding today. The hook stays
    // because the moment `KeyCode` gains a media or keypad key, forgetting it
    // here would silently send nothing.
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(code: KeyCode) -> Key {
        Key::plain(code)
    }
    fn km(code: KeyCode, m: Mods) -> Key {
        Key::new(code, m)
    }
    fn ctrl() -> Mods {
        Mods { ctrl: true, ..Mods::NONE }
    }

    // ── legacy ────────────────────────────────────────────────────────────

    #[test]
    fn f1_to_f4_use_ss3_until_a_modifier_appears() {
        assert_eq!(encode(k(KeyCode::F(1)), 0), b"\x1bOP");
        assert_eq!(encode(k(KeyCode::F(4)), 0), b"\x1bOS");
        assert_eq!(encode(km(KeyCode::F(1), ctrl()), 0), b"\x1b[1;5P");
    }

    #[test]
    fn a_release_produces_nothing_on_the_legacy_encoding() {
        for code in [KeyCode::Char('a'), KeyCode::Enter, KeyCode::Up, KeyCode::F(5)] {
            let key = Key { kind: KeyKind::Release, ..k(code) };
            assert!(encode(key, 0).is_empty(), "{code:?}");
        }
    }

    // ── kitty ─────────────────────────────────────────────────────────────

    #[test]
    fn the_event_type_appears_only_when_the_child_asked_for_it() {
        let rel = Key { kind: KeyKind::Release, ..k(KeyCode::Char('a')) };
        assert!(encode(rel, ALL_KEYS).is_empty(), "no REPORT_EVENT_TYPES, no release");
        assert_eq!(encode(rel, ALL_KEYS | EVENT_TYPES), b"\x1b[97;1:3u");
        let rep = Key { kind: KeyKind::Repeat, ..k(KeyCode::Char('a')) };
        assert_eq!(encode(rep, ALL_KEYS | EVENT_TYPES), b"\x1b[97;1:2u");
        assert_eq!(encode(k(KeyCode::Char('a')), ALL_KEYS | EVENT_TYPES), b"\x1b[97u",
            "a press needs no event suffix");
    }

    #[test]
    fn kitty_functional_keys_keep_their_csi_shapes() {
        assert_eq!(encode(k(KeyCode::Enter), ALL_KEYS), b"\x1b[13u");
        assert_eq!(encode(k(KeyCode::Esc), ALL_KEYS), b"\x1b[27u");
        assert_eq!(encode(k(KeyCode::Backspace), ALL_KEYS), b"\x1b[127u");
        assert_eq!(encode(k(KeyCode::Up), ALL_KEYS), b"\x1b[1;1A");
        assert_eq!(encode(k(KeyCode::Delete), ALL_KEYS), b"\x1b[3~");
    }

}
