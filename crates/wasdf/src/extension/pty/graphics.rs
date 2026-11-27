//! Kitty graphics passthrough for pty children.
//!
//! vt100 has no image model, so the reader thread cuts every APC
//! (`ESC _ … ESC \`) out of the child's output before the screen model sees
//! it. Kitty graphics commands (`ESC _ G …`) are kept with the child's cursor
//! at that point in the stream; render hands them to the event loop, which
//! writes them to the real terminal at the panel's offset. Only the control
//! string crosses — a child using shared memory (`mpv --vo=kitty
//! --vo-kitty-use-shm=yes`) sends no pixels through the pty at all.

/// An APC longer than this is dropped (a runaway or malicious child).
const MAX_APC: usize = 64 << 20;

/// A kitty command from the child, at its cursor `(row, col)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChildGraphic {
    pub row: u16,
    pub col: u16,
    pub data: String,
}

#[derive(Debug, Default)]
enum State {
    #[default]
    Ground,
    Esc,
    Apc,
    ApcEsc,
}

/// One piece of a read: text for vt100 (borrowed from the read) or an APC body.
#[derive(Debug, PartialEq, Eq)]
pub enum Seg<'a> {
    Text(&'a [u8]),
    Apc(Vec<u8>),
}

/// Incremental splitter: text for the screen model, APCs on the side. State
/// carries across reads, so a sequence split between chunks is still whole.
#[derive(Debug, Default)]
pub struct Splitter {
    state: State,
    apc: Vec<u8>,
    overflow: bool,
}

impl Splitter {
    /// Split one read into text for vt100 and complete APC bodies (without
    /// `ESC _` / `ESC \`), in stream order. Text is not copied.
    pub fn feed<'a>(&mut self, chunk: &'a [u8]) -> Vec<Seg<'a>> {
        let mut segs = Vec::new();
        let text = |segs: &mut Vec<Seg<'a>>, t: &'a [u8]| {
            if !t.is_empty() {
                segs.push(Seg::Text(t));
            }
        };
        let mut run = 0; // start of the pending text run in `chunk`
        for (i, &b) in chunk.iter().enumerate() {
            match self.state {
                State::Ground => {
                    if b == 0x1b {
                        text(&mut segs, &chunk[run..i]);
                        self.state = State::Esc;
                    }
                }
                State::Esc => {
                    if b == b'_' {
                        self.state = State::Apc;
                        self.apc.clear();
                        self.overflow = false;
                    } else {
                        // Not ours: give vt100 the ESC back.
                        text(&mut segs, b"\x1b");
                        if b != 0x1b {
                            // This byte starts the next text run.
                            self.state = State::Ground;
                            run = i;
                        }
                    }
                }
                State::Apc => {
                    if b == 0x1b {
                        self.state = State::ApcEsc;
                    } else if self.apc.len() < MAX_APC {
                        self.apc.push(b);
                    } else {
                        self.overflow = true;
                    }
                }
                State::ApcEsc => {
                    if b == b'\\' {
                        if !self.overflow {
                            segs.push(Seg::Apc(std::mem::take(&mut self.apc)));
                        }
                        self.state = State::Ground;
                        run = i + 1;
                    } else {
                        // A stray ESC inside the string: keep both bytes.
                        self.apc.extend_from_slice(&[0x1b, b]);
                        self.state = State::Apc;
                    }
                }
            }
            if !matches!(self.state, State::Ground) {
                run = i + 1;
            }
        }
        if matches!(self.state, State::Ground) {
            text(&mut segs, &chunk[run.min(chunk.len())..]);
        }
        segs
    }
}

/// A kitty command as it should reach the terminal: replies suppressed
/// (`q=2` — an answer would arrive on *our* stdin as keystrokes), re-framed.
/// `None` for an APC that is not kitty graphics.
pub fn kitty_command(body: &[u8]) -> Option<String> {
    let body = body.strip_prefix(b"G")?;
    let (keys, payload) = match body.iter().position(|&b| b == b';') {
        Some(i) => (&body[..i], Some(&body[i + 1..])),
        None => (body, None),
    };
    let keys = std::str::from_utf8(keys).ok()?;
    let mut out = String::from("\x1b_G");
    for kv in keys.split(',').filter(|kv| !kv.is_empty() && !kv.starts_with("q=")) {
        out.push_str(kv);
        out.push(',');
    }
    out.push_str("q=2");
    if let Some(p) = payload {
        out.push(';');
        out.push_str(std::str::from_utf8(p).ok()?);
    }
    out.push_str("\x1b\\");
    Some(out)
}

fn key<'a>(cmd: &'a str, k: &str) -> Option<&'a str> {
    let keys = cmd.strip_prefix("\x1b_G")?.split(';').next()?;
    keys.split(',').find_map(|kv| kv.strip_prefix(k)?.strip_prefix('='))
}

/// Queue `g`. A shared-memory frame (`a=T,t=s`) supersedes any still-queued
/// one at the same cursor: the object behind the name already holds the newer
/// frame, so the queued command could only redraw it twice.
pub fn push(queue: &mut Vec<ChildGraphic>, g: ChildGraphic) {
    let frame = |c: &ChildGraphic| key(&c.data, "a") == Some("T") && key(&c.data, "t") == Some("s");
    if frame(&g) {
        queue.retain(|q| !(frame(q) && (q.row, q.col) == (g.row, g.col)));
    }
    queue.push(g);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn split(chunks: &[&[u8]]) -> (Vec<u8>, Vec<Vec<u8>>) {
        let mut s = Splitter::default();
        let (mut text, mut apcs) = (Vec::new(), Vec::new());
        for c in chunks {
            for seg in s.feed(c) {
                match seg {
                    Seg::Text(t) => text.extend_from_slice(t),
                    Seg::Apc(a) => apcs.push(a),
                }
            }
        }
        (text, apcs)
    }

    #[test]
    fn cuts_apcs_out_and_keeps_other_escapes() {
        let (text, apcs) = split(&[b"ab\x1b[2J\x1b_Ga=T;xyz\x1b\\cd"]);
        assert_eq!(text, b"ab\x1b[2Jcd");
        assert_eq!(apcs, vec![b"Ga=T;xyz".to_vec()]);
    }

}
