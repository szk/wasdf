//! Decoding raw read results into the preview extension's own content
//! representation. Runs on the main thread (in `accept_content`); the bytes were
//! read off-thread by core. This is the preview extension's private content type
//! — core never sees it.

use std::path::Path;

use crate::core::{Block, Entry, TextSpan};

use super::highlight::highlight_lines;
use super::markdown;

const IMAGE_MAX: u32 = 320;

/// Preview's decoded content, held inside the extension and rendered by its
/// render hook. Text/hex/dir become styled lines; images become a bounded RGB
/// buffer. Text and hex grow chunk by chunk as the viewport scrolls (the
/// "more remains" marker is driven by the load's `eof`, not stored here).
#[derive(Debug, Clone, PartialEq)]
pub enum Decoded {
    /// Text lines with their per-line syntax-highlight spans (`spans[i]` covers
    /// `lines[i]`; may be shorter/empty when no syntax matched — then plain).
    Text { lines: Vec<String>, spans: Vec<Vec<TextSpan>> },
    /// A markdown document: the accumulated source plus the blocks it renders
    /// to. Markdown is a whole-document format (a fence, a list, a table only
    /// mean anything complete), so every chunk re-renders `source` rather than
    /// appending to `blocks`.
    Markdown { source: String, blocks: Vec<Block> },
    Image { width: u32, height: u32, rgb: Vec<u8> },
    Binary { lines: Vec<String> },
    Dir { entries: Vec<Entry> },
}

/// Which streamable representation the first chunk's MIME class selected. Images
/// and directories are not streamed (decoded whole), so they have no `Kind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Text,
    Markdown,
    Hex,
}

/// Classify the first chunk: an image (decoded whole from `path`), or a streamed
/// `Kind` whose empty content the caller will grow via [`append`].
pub fn classify_first(path: &Path, bytes: &[u8]) -> (Decoded, Option<Kind>) {
    use wasdf_sdk::{detect_mime, MimeClass};
    match detect_mime(bytes) {
        MimeClass::Image => (decode_image(path), None),
        MimeClass::Text if markdown::is_markdown(path) => {
            (Decoded::Markdown { source: String::new(), blocks: Vec::new() }, Some(Kind::Markdown))
        }
        MimeClass::Text => (Decoded::Text { lines: Vec::new(), spans: Vec::new() }, Some(Kind::Text)),
        MimeClass::Binary => (Decoded::Binary { lines: Vec::new() }, Some(Kind::Hex)),
    }
}

/// Append a chunk into the streamed content, decoding only the newly completed
/// units and returning the undecoded trailing bytes (`tail`) to prepend to the
/// next chunk. `buf` is the previous tail followed by the new chunk's bytes.
/// With `eof`, everything is flushed and the tail is empty. `tab_width` controls
/// how many spaces a `\t` character expands to.
pub fn append(
    path: &Path,
    kind: Kind,
    content: &mut Decoded,
    buf: &[u8],
    eof: bool,
    tab_width: usize,
) -> Vec<u8> {
    match (kind, content) {
        (Kind::Text, Decoded::Text { lines, spans }) => {
            let (new_lines, tail) = split_lines(buf, eof, tab_width);
            let new_spans = highlight_lines(path, &new_lines);
            lines.extend(new_lines);
            spans.extend(new_spans);
            tail
        }
        (Kind::Markdown, Decoded::Markdown { source, blocks }) => {
            let (text, tail) = split_source(buf, eof);
            source.push_str(&text);
            // Re-rendering is whole-document (and re-decodes every inline
            // image), so only pay for it twice: once on the first chunk, so a
            // large file shows something immediately, and once at eof for the
            // final document. Intermediate chunks just accumulate.
            if eof || blocks.is_empty() {
                *blocks = markdown::render(path, source, tab_width);
            }
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

/// Split `buf` at the last newline into decodable source text and the trailing
/// remainder carried to the next chunk, so the lossy UTF-8 decode never cuts a
/// multibyte char. Markdown keeps its source verbatim — line breaks and
/// indentation are syntax.
fn split_source(buf: &[u8], eof: bool) -> (String, Vec<u8>) {
    let cut = if eof {
        buf.len()
    } else {
        buf.iter().rposition(|&b| b == b'\n').map_or(0, |nl| nl + 1)
    };
    (String::from_utf8_lossy(&buf[..cut]).into_owned(), buf[cut..].to_vec())
}

/// Replace control characters that ratatui's cell-width calculator rejects.
/// `\t` expands to `tab_width` spaces; other control chars become `·` (U+00B7).
/// `\n` and `\r` are already stripped by `.lines()` so they never arrive here.
fn sanitize_line(s: &str, tab_width: usize) -> String {
    let tab = " ".repeat(tab_width.max(1));
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        if ch == '\t' {
            out.push_str(&tab);
        } else if ch.is_control() {
            out.push('·');
        } else {
            out.push(ch);
        }
    }
    out
}

/// Split `buf` into complete lines up to the last newline plus the trailing
/// remainder (a partial line carried to the next chunk). Cutting at `\n` keeps
/// the boundary off any multibyte char, so the lossy decode stays valid. With
/// `eof`, the whole buffer is decoded and the tail is empty.
fn split_lines(buf: &[u8], eof: bool, tab_width: usize) -> (Vec<String>, Vec<u8>) {
    if eof {
        let lines = String::from_utf8_lossy(buf)
            .lines()
            .map(|l| sanitize_line(l, tab_width))
            .collect();
        return (lines, Vec::new());
    }
    match buf.iter().rposition(|&b| b == b'\n') {
        Some(nl) => {
            let lines = String::from_utf8_lossy(&buf[..nl])
                .lines()
                .map(|l| sanitize_line(l, tab_width))
                .collect();
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

/// Decode an image file to a bounded RGB8 buffer `(width, height, rgb)`, or the
/// decoder's error message. Shared by the whole-file image preview and by
/// markdown's inline images ([`super::markdown`]).
pub fn load_rgb(path: &Path) -> Result<(u32, u32, Vec<u8>), String> {
    let img = image::open(path).map_err(|e| e.to_string())?;
    let img = if img.width() > IMAGE_MAX || img.height() > IMAGE_MAX {
        img.thumbnail(IMAGE_MAX, IMAGE_MAX)
    } else {
        img
    };
    let rgb = img.to_rgb8();
    let (width, height) = rgb.dimensions();
    Ok((width, height, rgb.into_raw()))
}

/// Decode an image to a bounded RGB8 buffer; on any error fall back to a note.
fn decode_image(path: &Path) -> Decoded {
    match load_rgb(path) {
        Ok((width, height, rgb)) => Decoded::Image { width, height, rgb },
        Err(e) => Decoded::Text { lines: vec![format!("[image: {e}]")], spans: Vec::new() },
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
