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
        KeyCode::Up | KeyCode::Down | KeyCode::Right | KeyCode::Left | KeyCode::Home
        | KeyCode::End => {
            let f = cursor_final(key.code).expect("arm covers only cursor-final codes");
            if modified {
                out.extend(format!("\x1b[1;{param}{}", f as char).into_bytes());
            } else {
                out.extend([ESC, b'[', f]);
            }
        }
        KeyCode::Insert | KeyCode::Delete | KeyCode::PageUp | KeyCode::PageDown => {
            let n = tilde_number(key.code).expect("arm covers only tilde codes");
            out.extend(tilde(n, param, modified));
        }
        KeyCode::F(n @ 1..=4) => {
            // SS3 in the unmodified case, CSI once a modifier is present —
            // xterm's own split, which editors' termcap entries assume.
            let f = b"PQRS"[(n - 1) as usize];
            if modified {
                out.extend(format!("\x1b[1;{param}{}", f as char).into_bytes());
            } else {
                out.extend([ESC, b'O', f]);
            }
        }
        KeyCode::F(n) => {
            if let Some(num) = f_tilde_number(n) {
                out.extend(tilde(num, param, modified));
            }
        }
    }
    out
}

/// `CSI <n> ~`, or `CSI <n> ; <param> ~` when modified.
fn tilde(n: u16, param: u8, modified: bool) -> Vec<u8> {
    if modified {
        format!("\x1b[{n};{param}~").into_bytes()
    } else {
        format!("\x1b[{n}~").into_bytes()
    }
}

fn cursor_final(code: KeyCode) -> Option<u8> {
    Some(match code {
        KeyCode::Up => b'A',
        KeyCode::Down => b'B',
        KeyCode::Right => b'C',
        KeyCode::Left => b'D',
        KeyCode::Home => b'H',
        KeyCode::End => b'F',
        _ => return None,
    })
}

fn tilde_number(code: KeyCode) -> Option<u16> {
    Some(match code {
        KeyCode::Insert => 2,
        KeyCode::Delete => 3,
        KeyCode::PageUp => 5,
        KeyCode::PageDown => 6,
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
    let suffix = match (param > 1, report_events && key.kind != KeyKind::Press) {
        (_, true) => format!(";{param}:{}", event_type(key.kind)),
        (true, false) => format!(";{param}"),
        (false, false) => String::new(),
    };

    match key.code {
        KeyCode::Char(c) => csi_u(c as u32, &suffix),
        KeyCode::Enter => csi_u(13, &suffix),
        KeyCode::Tab => csi_u(9, &suffix),
        // Kitty has no separate back-tab: it is Tab carrying shift, and the
        // shift has to be injected because core folds it into the code.
        KeyCode::BackTab => {
            let mut m = key.mods;
            m.shift = true;
            let p = kitty_mods(m);
            let s = if report_events && key.kind != KeyKind::Press {
                format!(";{p}:{}", event_type(key.kind))
            } else {
                format!(";{p}")
            };
            csi_u(9, &s)
        }
        KeyCode::Backspace => csi_u(127, &suffix),
        KeyCode::Esc => csi_u(27, &suffix),
        KeyCode::Up | KeyCode::Down | KeyCode::Right | KeyCode::Left | KeyCode::Home
        | KeyCode::End => {
            let f = cursor_final(key.code).expect("arm covers only cursor-final codes");
            let s = if suffix.is_empty() { ";1".to_string() } else { suffix };
            format!("\x1b[1{s}{}", f as char).into_bytes()
        }
        KeyCode::Insert | KeyCode::Delete | KeyCode::PageUp | KeyCode::PageDown => {
            let n = tilde_number(key.code).expect("arm covers only tilde codes");
            format!("\x1b[{n}{suffix}~").into_bytes()
        }
        KeyCode::F(n @ 1..=4) => {
            let f = b"PQRS"[(n - 1) as usize];
            let s = if suffix.is_empty() { ";1".to_string() } else { suffix };
            format!("\x1b[1{s}{}", f as char).into_bytes()
        }
        KeyCode::F(n) => match f_tilde_number(n) {
            Some(num) => format!("\x1b[{num}{suffix}~").into_bytes(),
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
    fn a_plain_character_is_its_own_utf8() {
        assert_eq!(encode(k(KeyCode::Char('a')), 0), b"a");
        assert_eq!(encode(k(KeyCode::Char('あ')), 0), "あ".as_bytes());
    }

    #[test]
    fn ctrl_letters_fold_to_their_c0_bytes() {
        assert_eq!(encode(km(KeyCode::Char('a'), ctrl()), 0), vec![0x01]);
        assert_eq!(encode(km(KeyCode::Char('A'), ctrl()), 0), vec![0x01]);
        assert_eq!(encode(km(KeyCode::Char('c'), ctrl()), 0), vec![0x03]);
        assert_eq!(encode(km(KeyCode::Char('@'), ctrl()), 0), vec![0x00]);
        assert_eq!(encode(km(KeyCode::Char('['), ctrl()), 0), vec![0x1b]);
        assert_eq!(encode(km(KeyCode::Char('?'), ctrl()), 0), vec![0x7f]);
    }

    #[test]
    fn a_ctrl_pair_with_no_c0_encoding_is_sent_bare() {
        assert_eq!(encode(km(KeyCode::Char('1'), ctrl()), 0), b"1");
    }

    #[test]
    fn alt_prefixes_an_escape() {
        let alt = Mods { alt: true, ..Mods::NONE };
        assert_eq!(encode(km(KeyCode::Char('x'), alt), 0), vec![ESC, b'x']);
        let ctrl_alt = Mods { ctrl: true, alt: true, ..Mods::NONE };
        assert_eq!(encode(km(KeyCode::Char('x'), ctrl_alt), 0), vec![ESC, 0x18]);
    }

    #[test]
    fn enter_is_carriage_return_never_newline() {
        assert_eq!(encode(k(KeyCode::Enter), 0), b"\r");
    }

    #[test]
    fn the_simple_control_keys_have_their_classic_bytes() {
        assert_eq!(encode(k(KeyCode::Tab), 0), b"\t");
        assert_eq!(encode(k(KeyCode::BackTab), 0), b"\x1b[Z");
        assert_eq!(encode(k(KeyCode::Backspace), 0), vec![0x7f]);
        assert_eq!(encode(k(KeyCode::Esc), 0), vec![ESC]);
    }

    #[test]
    fn unmodified_cursor_keys_use_the_bare_csi_form() {
        assert_eq!(encode(k(KeyCode::Up), 0), b"\x1b[A");
        assert_eq!(encode(k(KeyCode::Down), 0), b"\x1b[B");
        assert_eq!(encode(k(KeyCode::Right), 0), b"\x1b[C");
        assert_eq!(encode(k(KeyCode::Left), 0), b"\x1b[D");
        assert_eq!(encode(k(KeyCode::Home), 0), b"\x1b[H");
        assert_eq!(encode(k(KeyCode::End), 0), b"\x1b[F");
    }

    #[test]
    fn the_xterm_modifier_parameter_weights_shift_alt_ctrl_meta_as_1_2_4_8() {
        let cases = [
            (Mods { shift: true, ..Mods::NONE }, 2),
            (Mods { alt: true, ..Mods::NONE }, 3),
            (Mods { ctrl: true, ..Mods::NONE }, 5),
            (Mods { meta: true, ..Mods::NONE }, 9),
            (Mods { ctrl: true, shift: true, ..Mods::NONE }, 6),
        ];
        for (m, want) in cases {
            assert_eq!(
                encode(km(KeyCode::Up, m), 0),
                format!("\x1b[1;{want}A").into_bytes(),
                "{m:?}"
            );
        }
    }

    #[test]
    fn super_and_hyper_are_dropped_by_the_legacy_encoding() {
        // xterm has no parameter for them; silently ignoring beats inventing one.
        let m = Mods { sup: true, hyper: true, ..Mods::NONE };
        assert_eq!(encode(km(KeyCode::Up, m), 0), b"\x1b[A");
    }

    #[test]
    fn the_tilde_keys_carry_their_xterm_numbers() {
        assert_eq!(encode(k(KeyCode::Insert), 0), b"\x1b[2~");
        assert_eq!(encode(k(KeyCode::Delete), 0), b"\x1b[3~");
        assert_eq!(encode(k(KeyCode::PageUp), 0), b"\x1b[5~");
        assert_eq!(encode(k(KeyCode::PageDown), 0), b"\x1b[6~");
        assert_eq!(encode(km(KeyCode::Delete, ctrl()), 0), b"\x1b[3;5~");
    }

    #[test]
    fn f1_to_f4_use_ss3_until_a_modifier_appears() {
        assert_eq!(encode(k(KeyCode::F(1)), 0), b"\x1bOP");
        assert_eq!(encode(k(KeyCode::F(4)), 0), b"\x1bOS");
        assert_eq!(encode(km(KeyCode::F(1), ctrl()), 0), b"\x1b[1;5P");
    }

    #[test]
    fn f5_and_above_use_the_gappy_tilde_table() {
        assert_eq!(encode(k(KeyCode::F(5)), 0), b"\x1b[15~");
        assert_eq!(encode(k(KeyCode::F(6)), 0), b"\x1b[17~", "16 is skipped");
        assert_eq!(encode(k(KeyCode::F(11)), 0), b"\x1b[23~", "22 is skipped");
        assert_eq!(encode(k(KeyCode::F(12)), 0), b"\x1b[24~");
        assert!(encode(k(KeyCode::F(30)), 0).is_empty(), "no agreed encoding past F20");
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
    fn the_kitty_modifier_parameter_puts_super_at_8_and_meta_at_32() {
        // Deliberately checked against xterm's weights, which differ: this is
        // the easiest thing in the encoder to transpose.
        let cases = [
            (Mods { shift: true, ..Mods::NONE }, 2),
            (Mods { alt: true, ..Mods::NONE }, 3),
            (Mods { ctrl: true, ..Mods::NONE }, 5),
            (Mods { sup: true, ..Mods::NONE }, 9),
            (Mods { hyper: true, ..Mods::NONE }, 17),
            (Mods { meta: true, ..Mods::NONE }, 33),
        ];
        for (m, want) in cases {
            assert_eq!(kitty_mods(m), want, "{m:?}");
        }
        assert_eq!(xterm_mods(Mods { meta: true, ..Mods::NONE }), 9, "xterm puts meta at 8");
    }

    #[test]
    fn a_modified_key_becomes_csi_u() {
        assert_eq!(encode(km(KeyCode::Char('a'), ctrl()), ALL_KEYS), b"\x1b[97;5u");
    }

    #[test]
    fn a_bare_press_stays_legacy_unless_all_keys_is_set() {
        use super::super::kbmode::DISAMBIGUATE;
        assert_eq!(encode(k(KeyCode::Char('a')), DISAMBIGUATE), b"a");
        assert_eq!(encode(k(KeyCode::Char('a')), ALL_KEYS), b"\x1b[97u");
    }

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

    #[test]
    fn back_tab_is_shifted_tab_under_kitty() {
        // Core folds shift into the BackTab code; Kitty expects it spelled out.
        assert_eq!(encode(k(KeyCode::BackTab), ALL_KEYS), b"\x1b[9;2u");
    }

    #[test]
    fn a_plain_key_encodes_identically_until_the_child_asks_for_all_keys() {
        use super::super::kbmode::DISAMBIGUATE;
        let plain = k(KeyCode::Char('a'));
        assert_eq!(encode(plain, 0), encode(plain, DISAMBIGUATE | EVENT_TYPES));
        assert_ne!(encode(plain, 0), encode(plain, ALL_KEYS));
    }

    #[test]
    fn a_modified_key_encodes_differently_under_the_two_protocols() {
        // The property that catches a mis-wired `flags` argument.
        let key = km(KeyCode::Char('a'), ctrl());
        assert_ne!(encode(key, 0), encode(key, ALL_KEYS));
    }
}
