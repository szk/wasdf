//! Decoding raw read results into the preview extension's own content
//! representation. Runs on the main thread (in `accept_content`); the bytes were
//! read off-thread by core. This is the preview extension's private content type
//! — core never sees it.

use std::path::Path;

use crate::core::{Entry, StyleRun};

use super::highlight::highlight_lines;

const IMAGE_MAX: u32 = 320;

/// Preview's decoded content, held inside the extension and rendered by its
/// render hook. Text/hex/dir become styled lines; images become a bounded RGB
/// buffer. Text and hex grow chunk by chunk as the viewport scrolls (the
/// "more remains" marker is driven by the load's `eof`, not stored here).
#[derive(Debug, Clone, PartialEq)]
pub enum Decoded {
    Text { lines: Vec<String>, styles: Vec<Vec<StyleRun>> },
    Image { width: u32, height: u32, rgb: Vec<u8> },
    Binary { lines: Vec<String> },
    Dir { entries: Vec<Entry> },
}

/// Which streamable representation the first chunk's MIME class selected. Images
/// and directories are not streamed (decoded whole), so they have no `Kind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Text,
    Hex,
}

/// Classify the first chunk: an image (decoded whole from `path`), or a streamed
/// `Kind` whose empty content the caller will grow via [`append`].
pub fn classify_first(path: &Path, bytes: &[u8]) -> (Decoded, Option<Kind>) {
    use wasdf_sdk::{detect_mime, MimeClass};
    match detect_mime(bytes) {
        MimeClass::Image => (decode_image(path), None),
        MimeClass::Text => (Decoded::Text { lines: Vec::new(), styles: Vec::new() }, Some(Kind::Text)),
        MimeClass::Binary => (Decoded::Binary { lines: Vec::new() }, Some(Kind::Hex)),
    }
}

/// Append a chunk into the streamed content, decoding only the newly completed
/// units and returning the undecoded trailing bytes (`tail`) to prepend to the
/// next chunk. `buf` is the previous tail followed by the new chunk's bytes.
/// With `eof`, everything is flushed and the tail is empty.
pub fn append(path: &Path, kind: Kind, content: &mut Decoded, buf: &[u8], eof: bool) -> Vec<u8> {
    match (kind, content) {
        (Kind::Text, Decoded::Text { lines, styles }) => {
            let (new_lines, tail) = split_lines(buf, eof);
            let new_styles = highlight_lines(path, &new_lines);
            lines.extend(new_lines);
            styles.extend(new_styles);
            tail
        }
        (Kind::Hex, Decoded::Binary { lines }) => {
            let (rows, tail) = hex_rows(buf, lines.len(), eof);
            lines.extend(rows);
            tail
        }
        _ => Vec::new(),
    }
}

/// Split `buf` into complete lines up to the last newline plus the trailing
/// remainder (a partial line carried to the next chunk). Cutting at `\n` keeps
/// the boundary off any multibyte char, so the lossy decode stays valid. With
/// `eof`, the whole buffer is decoded and the tail is empty.
fn split_lines(buf: &[u8], eof: bool) -> (Vec<String>, Vec<u8>) {
    if eof {
        let lines = String::from_utf8_lossy(buf).lines().map(|l| l.to_string()).collect();
        return (lines, Vec::new());
    }
    match buf.iter().rposition(|&b| b == b'\n') {
        Some(nl) => {
            let lines = String::from_utf8_lossy(&buf[..nl]).lines().map(|l| l.to_string()).collect();
            (lines, buf[nl + 1..].to_vec())
        }
        None => (Vec::new(), buf.to_vec()),
    }
}

/// Format `buf` into full 16-byte hexdump rows addressed from row `start_row`,
/// returning the rows and the (<16-byte) remainder carried to the next chunk.
/// With `eof`, the remainder is emitted as a final short row.
fn hex_rows(buf: &[u8], start_row: usize, eof: bool) -> (Vec<String>, Vec<u8>) {
    let full = if eof { buf.len() } else { buf.len() - buf.len() % 16 };
    let rows = buf[..full]
        .chunks(16)
        .enumerate()
        .map(|(i, chunk)| hex_row((start_row + i) * 16, chunk))
        .collect();
    (rows, buf[full..].to_vec())
}

/// Decode an image to a bounded RGB8 buffer; on any error fall back to a note.
fn decode_image(path: &Path) -> Decoded {
    match image::open(path) {
        Ok(img) => {
            let img = if img.width() > IMAGE_MAX || img.height() > IMAGE_MAX {
                img.thumbnail(IMAGE_MAX, IMAGE_MAX)
            } else {
                img
            };
            let rgb = img.to_rgb8();
            let (width, height) = rgb.dimensions();
            Decoded::Image { width, height, rgb: rgb.into_raw() }
        }
        Err(e) => Decoded::Text { lines: vec![format!("[image: {e}]")], styles: Vec::new() },
    }
}

/// One hexdump row: `{addr}  {hex bytes}  {ascii}` for up to 16 bytes at `addr`.
fn hex_row(addr: usize, chunk: &[u8]) -> String {
    let hex: Vec<String> = chunk.iter().map(|b| format!("{b:02x}")).collect();
    let ascii: String = chunk
        .iter()
        .map(|&b| if (0x20..0x7f).contains(&b) { b as char } else { '.' })
        .collect();
    format!("{addr:08x}  {:<48}  {ascii}", hex.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_image_to_bounded_rgb() {
        let mut path = std::env::temp_dir();
        path.push(format!("wasdf-img-{}.png", std::process::id()));
        let img = image::RgbImage::from_fn(4, 4, |x, _| image::Rgb([(x * 60) as u8, 0, 0]));
        img.save(&path).unwrap();
        match classify_first(&path, &std::fs::read(&path).unwrap()) {
            (Decoded::Image { width, height, rgb }, None) => {
                assert_eq!((width, height), (4, 4));
                assert_eq!(rgb.len(), 4 * 4 * 3);
            }
            other => panic!("expected image, got {other:?}"),
        }
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn text_classifies_then_appends_with_highlight() {
        let (mut content, kind) = classify_first(Path::new("x.rs"), b"fn main() {}\n");
        assert_eq!(kind, Some(Kind::Text));
        let tail = append(Path::new("x.rs"), Kind::Text, &mut content, b"fn main() {}\n", true);
        assert!(tail.is_empty());
        match content {
            Decoded::Text { lines, styles } => {
                assert_eq!(lines, vec!["fn main() {}".to_string()]);
                assert!(!styles[0].is_empty(), "rust got highlight runs");
            }
            other => panic!("expected text, got {other:?}"),
        }
    }

    #[test]
    fn text_append_carries_a_partial_line_across_chunks() {
        let mut content = Decoded::Text { lines: Vec::new(), styles: Vec::new() };
        // First chunk ends mid-line: only the complete line decodes; the rest tails.
        let tail = append(Path::new("a.txt"), Kind::Text, &mut content, b"alpha\nbe", false);
        assert_eq!(tail, b"be");
        // The tail is prepended to the next chunk.
        let mut buf = tail;
        buf.extend_from_slice(b"ta\ngamma");
        let tail = append(Path::new("a.txt"), Kind::Text, &mut content, &buf, true);
        assert!(tail.is_empty());
        match content {
            Decoded::Text { lines, .. } => assert_eq!(lines, vec!["alpha", "beta", "gamma"]),
            other => panic!("expected text, got {other:?}"),
        }
    }

    #[test]
    fn hex_append_rows_have_global_addresses_and_carry_a_remainder() {
        let mut content = Decoded::Binary { lines: Vec::new() };
        // 18 bytes, not eof: one full 16-byte row, 2 bytes tail.
        let chunk: Vec<u8> = (0..18u8).collect();
        let tail = append(Path::new("b.bin"), Kind::Hex, &mut content, &chunk, false);
        assert_eq!(tail.len(), 2);
        // Next chunk (tail + 16 bytes), eof: row addressed at 0x10, then 0x20 remainder.
        let mut buf = tail;
        buf.extend((18..34u8).collect::<Vec<_>>());
        append(Path::new("b.bin"), Kind::Hex, &mut content, &buf, true);
        match content {
            Decoded::Binary { lines } => {
                assert_eq!(lines.len(), 3);
                assert!(lines[0].starts_with("00000000"));
                assert!(lines[1].starts_with("00000010"));
                assert!(lines[2].starts_with("00000020"), "final short row at the global offset");
            }
            other => panic!("expected hex, got {other:?}"),
        }
    }
}
