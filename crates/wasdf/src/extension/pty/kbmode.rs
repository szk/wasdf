//! What the child has told the terminal about **input**, recovered by watching
//! its output: the Kitty keyboard-protocol stack, and whether its line editor is
//! currently accepting a line (bracketed paste).
//!
//! A PTY child negotiates these by writing to its terminal — us. `vt100` 0.16
//! parses the screen but models neither, and hands us no hook to observe them,
//! so the reader thread feeds the same bytes here in parallel. This never
//! modifies the stream; `vt100` still sees every byte.
//!
//! Without the keyboard stack, [`super::encode`] would have to guess, and
//! guessing wrong is not cosmetic: sending CSI-u to a child that never asked
//! for it turns every keystroke into visible garbage. Without the paste state
//! there is no reliable way to tell a shell sitting at its prompt from one
//! running a command — see [`Stack::at_prompt`].

/// `DISAMBIGUATE_ESCAPE_CODES` — report ambiguous keys (`Ctrl-i` vs Tab) in CSI-u form.
pub const DISAMBIGUATE: u8 = 0b0000_0001;
/// `REPORT_EVENT_TYPES` — also send repeat and release events.
pub const EVENT_TYPES: u8 = 0b0000_0010;
/// `REPORT_ALL_KEYS_AS_ESCAPE_CODES` — even plain printable keys become CSI-u.
pub const ALL_KEYS: u8 = 0b0000_1000;

/// A CSI sequence longer than this is not one of ours; give up on it rather
/// than buffering unbounded garbage from a child writing binary.
const MAX_PARAMS: usize = 32;

/// DEC private mode 2004 — bracketed paste. A line editor turns it on while it
/// is reading a line and off while the command it read is running, so its state
/// doubles as "is the shell at its prompt?".
const BRACKETED_PASTE: &str = "?2004";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scan {
    Ground,
    /// Saw `ESC`, waiting for `[`.
    Escape,
    /// Inside `CSI …`, accumulating parameter bytes.
    Params,
}

/// The child's keyboard-mode stack, innermost last. Empty means the child has
/// asked for nothing, i.e. the legacy encoding.
#[derive(Debug)]
pub struct Stack {
    levels: Vec<u8>,
    scan: Scan,
    params: Vec<u8>,
    /// `None` until the child says anything about bracketed paste; `Some` after.
    paste: Option<bool>,
}

impl Default for Stack {
    fn default() -> Self {
        Stack {
            levels: Vec::new(),
            scan: Scan::Ground,
            params: Vec::with_capacity(MAX_PARAMS),
            paste: None,
        }
    }
}

impl Stack {
    /// The flags in force right now; 0 (legacy) when the child has pushed nothing.
    pub fn current(&self) -> u8 {
        self.levels.last().copied().unwrap_or(0)
    }

    /// Whether the child's line editor is reading a line, as far as bracketed
    /// paste reveals. `None` means it has never mentioned it — an old `bash`, a
    /// `dash`, or anything with no line editor — and the caller must fall back
    /// to the tty's foreground process group.
    ///
    /// zsh, fish and nushell all toggle this around every command, which is what
    /// makes it a usable prompt detector; the process-group test alone is not,
    /// because zsh leaves job control off when wasdf spawns it.
    pub fn at_prompt(&self) -> Option<bool> {
        self.paste
    }

    /// Whether pasted text must be wrapped in `ESC[200~` … `ESC[201~`.
    pub fn bracketed_paste(&self) -> bool {
        self.paste == Some(true)
    }

    /// Feed one chunk of the child's output. Returns bytes to write back into
    /// the child's *input* — non-empty only for the `CSI ? u` query, which the
    /// protocol requires us to answer.
    ///
    /// The scanner's state persists across calls on purpose: the reader
    /// delivers fixed-size chunks with no regard for sequence boundaries, so a
    /// sequence split down the middle is the ordinary case, not an edge case.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<u8> {
        let mut reply = Vec::new();
        for &b in bytes {
            match self.scan {
                Scan::Ground => {
                    if b == 0x1b {
                        self.scan = Scan::Escape;
                    }
                }
                Scan::Escape => {
                    self.scan = if b == b'[' { Scan::Params } else { Scan::Ground };
                    self.params.clear();
                    // An ESC immediately followed by ESC restarts the sequence
                    // rather than being swallowed as its own introducer.
                    if b == 0x1b {
                        self.scan = Scan::Escape;
                    }
                }
                Scan::Params => {
                    // Final bytes are 0x40..=0x7e; everything before them is
                    // parameter or intermediate.
                    if (0x40..=0x7e).contains(&b) {
                        match b {
                            b'u' => reply.extend(self.apply()),
                            b'h' | b'l' => self.set_mode(b == b'h'),
                            _ => {}
                        }
                        self.scan = Scan::Ground;
                        self.params.clear();
                    } else if self.params.len() < MAX_PARAMS {
                        self.params.push(b);
                    } else {
                        self.scan = Scan::Ground;
                        self.params.clear();
                    }
                }
            }
        }
        reply
    }

    /// Interpret an accumulated `CSI <params> h|l` — only mode 2004 matters; a
    /// terminal sets and resets dozens of others we have no use for.
    fn set_mode(&mut self, on: bool) {
        if self.params.as_slice() == BRACKETED_PASTE.as_bytes() {
            self.paste = Some(on);
        }
    }

    /// Interpret an accumulated `CSI <params> u`. Anything that is not one of
    /// the four keyboard forms is left alone — plenty of unrelated sequences
    /// end in `u`.
    fn apply(&mut self) -> Vec<u8> {
        let Ok(s) = std::str::from_utf8(&self.params) else { return Vec::new() };
        match s.as_bytes().first() {
            Some(b'>') => {
                self.levels.push(parse_first(&s[1..]).unwrap_or(0));
                Vec::new()
            }
            Some(b'<') => {
                // A bare `CSI < u` pops one level.
                let n = parse_first(&s[1..]).unwrap_or(1).max(1) as usize;
                let keep = self.levels.len().saturating_sub(n);
                self.levels.truncate(keep);
                Vec::new()
            }
            Some(b'=') => {
                let rest = &s[1..];
                let mut parts = rest.split(';');
                let flags = parse_first(parts.next().unwrap_or("")).unwrap_or(0);
                // mode 1 = set, 2 = or, 3 = and-not; default 1.
                let mode = parse_first(parts.next().unwrap_or("")).unwrap_or(1);
                if self.levels.is_empty() {
                    self.levels.push(0);
                }
                let cur = self.levels.last_mut().expect("just ensured non-empty");
                *cur = match mode {
                    2 => *cur | flags,
                    3 => *cur & !flags,
                    _ => flags,
                };
                Vec::new()
            }
            Some(b'?') if s.len() == 1 => {
                format!("\x1b[?{}u", self.current()).into_bytes()
            }
            _ => Vec::new(),
        }
    }
}

/// The leading decimal number of a parameter string, as a `u8`. `None` when it
/// is empty or not a number; out-of-range values saturate, since every flag we
/// care about lives in the low byte.
fn parse_first(s: &str) -> Option<u8> {
    let digits: String = s.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return None;
    }
    Some(digits.parse::<u32>().unwrap_or(0).min(u8::MAX as u32) as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_and_pop_track_the_childs_nesting() {
        let mut s = Stack::default();
        s.feed(b"\x1b[>5u");
        assert_eq!(s.current(), 5);
        s.feed(b"\x1b[>15u");
        assert_eq!(s.current(), 15);
        s.feed(b"\x1b[<1u");
        assert_eq!(s.current(), 5);
        s.feed(b"\x1b[<u");
        assert_eq!(s.current(), 0, "a bare pop takes one level");
    }

    #[test]
    fn a_sequence_split_across_two_chunks_is_still_recognised() {
        // The reader hands over fixed-size chunks, so this is the common case.
        let mut s = Stack::default();
        for cut in 1..6 {
            let mut s2 = Stack::default();
            let seq = b"\x1b[>5u";
            s2.feed(&seq[..cut]);
            s2.feed(&seq[cut..]);
            assert_eq!(s2.current(), 5, "split at {cut}");
        }
        s.feed(b"\x1b");
        s.feed(b"[");
        s.feed(b">");
        s.feed(b"7");
        s.feed(b"u");
        assert_eq!(s.current(), 7, "byte at a time");
    }

}
