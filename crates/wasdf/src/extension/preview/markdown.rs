//! Markdown rendering for the preview panel (`.md` / `.markdown`).
//!
//! `tui-markdown` turns the source into a styled `ratatui::text::Text`; this
//! module lowers that back into the preview's logical [`Block`] vocabulary so
//! core keeps owning layout, h-scroll, line numbers and search highlight.
//!
//! Images are the one thing `tui-markdown` cannot do: it renders every image as
//! an `[img] …` text fallback. So the source is first scanned with
//! `pulldown-cmark` for **standalone image paragraphs** (a top-level paragraph
//! whose entire content is one image — the usual `![alt](path)` on its own
//! line), the source is split around them, and each becomes either a decoded
//! [`Block::Image`] (local file) or a one-line placeholder (remote URL, or a
//! local file that would not decode). Images nested in a list, a table or a
//! sentence keep `tui-markdown`'s inline `[img]` fallback — splitting the source
//! there would tear the enclosing block apart.

use std::ops::Range;
use std::path::{Path, PathBuf};

use pulldown_cmark::{Event, Options as ParseOptions, Parser, Tag, TagEnd};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Text;

use crate::core::{Block, Emph, SpanStyle, TextSpan};

use super::decode;

/// Whether `path` should be previewed as markdown rather than as plain text.
pub fn is_markdown(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("md") || e.eq_ignore_ascii_case("markdown"))
}

/// Render `source` (the markdown file at `path`) into panel blocks. Relative
/// image paths resolve against `path`'s directory.
pub fn render(path: &Path, source: &str, tab_width: usize) -> Vec<Block> {
    let base = path.parent().unwrap_or_else(|| Path::new("."));
    let mut blocks = Vec::new();
    let mut cut = 0usize;
    for (range, url) in standalone_images(source) {
        blocks.extend(render_text(&source[cut..range.start], tab_width));
        // Each segment is rendered on its own, so the blank line tui-markdown
        // would have put between two paragraphs is missing at every seam.
        separate(&mut blocks);
        blocks.push(image_block(base, &url));
        cut = range.end;
    }
    let tail = render_text(&source[cut..], tab_width);
    if !tail.is_empty() {
        separate(&mut blocks);
    }
    blocks.extend(tail);
    blocks
}

/// Close the current run of blocks with one blank line, unless there is nothing
/// to separate from or the run already ends blank.
fn separate(blocks: &mut Vec<Block>) {
    let ends_blank = matches!(blocks.last(), Some(Block::Line(spans)) if spans.iter().all(|s| s.text.is_empty()));
    if !blocks.is_empty() && !ends_blank {
        blocks.push(Block::Line(Vec::new()));
    }
}

/// Lower one markdown segment through `tui-markdown` into logical blocks.
fn render_text(source: &str, tab_width: usize) -> Vec<Block> {
    if source.trim().is_empty() {
        return Vec::new();
    }
    text_to_blocks(tui_markdown::from_str(source), tab_width)
}

fn text_to_blocks(text: Text<'_>, tab_width: usize) -> Vec<Block> {
    text.lines
        .into_iter()
        .map(|line| {
            // A ratatui Line's own style is the base its spans patch onto.
            let base = line.style;
            Block::Line(
                line.spans
                    .into_iter()
                    .map(|s| to_text_span(&s.content, base.patch(s.style), tab_width))
                    .collect(),
            )
        })
        .collect()
}

/// Lower a ratatui span into the logical [`TextSpan`] vocabulary: color into
/// [`SpanStyle`], font modifiers into [`Emph`].
fn to_text_span(text: &str, style: Style, tab_width: usize) -> TextSpan {
    let bg = style.bg.and_then(rgb_of);
    let span_style = match (style.fg.and_then(rgb_of), bg) {
        (None, None) => SpanStyle::Default,
        // `SpanStyle::Rgb` has no "keep the default foreground" form, and
        // tui-markdown's H1 is a background-only style; white is the foreground
        // its own docs assume.
        (fg, bg) => SpanStyle::Rgb { fg: fg.unwrap_or((0xc0, 0xc0, 0xc0)), bg },
    };
    let m = style.add_modifier;
    TextSpan {
        text: sanitize(text, tab_width),
        style: span_style,
        emph: Emph {
            bold: m.contains(Modifier::BOLD),
            italic: m.contains(Modifier::ITALIC),
            underline: m.contains(Modifier::UNDERLINED),
            dim: m.contains(Modifier::DIM),
        },
    }
}

/// The RGB triple a ratatui color resolves to. The named ANSI colors are pinned
/// to the standard xterm palette because [`SpanStyle`] carries literal rgb only;
/// `Reset` and 256-palette indices have no fixed triple and stay unstyled.
fn rgb_of(c: Color) -> Option<(u8, u8, u8)> {
    let t = match c {
        Color::Rgb(r, g, b) => (r, g, b),
        Color::Black => (0x00, 0x00, 0x00),
        Color::Red => (0x80, 0x00, 0x00),
        Color::Green => (0x00, 0x80, 0x00),
        Color::Yellow => (0x80, 0x80, 0x00),
        Color::Blue => (0x00, 0x00, 0x80),
        Color::Magenta => (0x80, 0x00, 0x80),
        Color::Cyan => (0x00, 0x80, 0x80),
        Color::Gray => (0xc0, 0xc0, 0xc0),
        Color::DarkGray => (0x80, 0x80, 0x80),
        Color::LightRed => (0xff, 0x00, 0x00),
        Color::LightGreen => (0x00, 0xff, 0x00),
        Color::LightYellow => (0xff, 0xff, 0x00),
        Color::LightBlue => (0x00, 0x00, 0xff),
        Color::LightMagenta => (0xff, 0x00, 0xff),
        Color::LightCyan => (0x00, 0xff, 0xff),
        Color::White => (0xff, 0xff, 0xff),
        _ => return None,
    };
    Some(t)
}

/// Replace control characters ratatui's cell-width calculator rejects, as
/// `decode::sanitize_line` does for plain text.
fn sanitize(s: &str, tab_width: usize) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '\t' => out.push_str(&" ".repeat(tab_width.max(1))),
            c if c.is_control() => out.push('·'),
            c => out.push(c),
        }
    }
    out
}

// -- images -----------------------------------------------------------------

/// The block a standalone image renders to: the decoded pixels for a local file,
/// a placeholder line for a remote URL or an undecodable file.
fn image_block(base: &Path, url: &str) -> Block {
    let Some(path) = local_path(base, url) else {
        return placeholder(format!("[image: {url} (remote)]"));
    };
    match decode::load_rgb(&path) {
        Ok((width, height, rgb)) => Block::Image { width, height, rgb },
        Err(e) => placeholder(format!("[image: {url} — {e}]")),
    }
}

fn placeholder(text: String) -> Block {
    Block::Line(vec![TextSpan {
        text,
        style: SpanStyle::Role("muted".into()),
        emph: Emph { italic: true, ..Emph::default() },
    }])
}

/// The local file an image destination points at, or `None` when it is remote.
/// Anything carrying a URL scheme other than `file:` is remote; everything else
/// is a path, resolved against the markdown file's own directory.
fn local_path(base: &Path, url: &str) -> Option<PathBuf> {
    if url.starts_with("//") {
        return None; // protocol-relative
    }
    let rest = match url.split_once("://") {
        Some(("file", rest)) => rest.strip_prefix("localhost").unwrap_or(rest),
        Some(_) => return None,
        // A bare scheme with no authority (`data:`, `mailto:`) is never a path.
        // A Windows-style `C:\…` is not a concern on the platforms wasdf ships.
        None if url.split_once(':').is_some_and(|(s, _)| is_scheme(s)) => return None,
        None => url,
    };
    let decoded = percent_decode(rest.split(['?', '#']).next().unwrap_or(rest));
    if decoded.is_empty() {
        return None;
    }
    let path = Path::new(&decoded);
    Some(if path.is_absolute() { path.to_path_buf() } else { base.join(path) })
}

fn is_scheme(s: &str) -> bool {
    !s.is_empty()
        && s.starts_with(|c: char| c.is_ascii_alphabetic())
        && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

/// Decode `%XX` escapes; markdown writers percent-encode spaces in image paths.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match (bytes[i], s.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(h, 16).ok())) {
            (b'%', Some(byte)) => {
                out.push(byte);
                i += 3;
            }
            (b, _) => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Scan `source` for top-level paragraphs whose entire content is a single
/// image, returning each paragraph's byte range and the image destination, in
/// source order. The parse options mirror `tui_markdown::from_str` so both
/// halves see the same document.
fn standalone_images(source: &str) -> Vec<(Range<usize>, String)> {
    let mut out = Vec::new();
    // Depth of open tags: a top-level paragraph opens at 0, and its own inline
    // content sits at exactly 1 — which is what makes "the paragraph holds
    // nothing but this image" checkable without a tag stack.
    let mut depth = 0usize;
    let mut para: Option<Range<usize>> = None;
    let mut url: Option<String> = None;
    let mut extra = false;

    for (ev, range) in Parser::new_ext(source, parse_options()).into_offset_iter() {
        let inline = para.is_some() && depth == 1;
        match &ev {
            Event::Start(Tag::Paragraph) if depth == 0 => {
                para = Some(range.clone());
                url = None;
                extra = false;
            }
            Event::Start(Tag::Image { dest_url, .. }) if inline => {
                extra |= url.is_some(); // a second image: no longer standalone
                url = Some(dest_url.to_string());
            }
            Event::End(TagEnd::Paragraph) if depth == 1 => {
                if let (Some(p), Some(u), false) = (para.take(), url.take(), extra) {
                    out.push((p, u));
                }
            }
            Event::Text(t) if inline => extra |= !t.trim().is_empty(),
            // Any other inline content — a link, emphasis, code, raw html.
            Event::Start(_) | Event::Code(_) | Event::InlineHtml(_) | Event::InlineMath(_)
                if inline =>
            {
                extra = true;
            }
            _ => {}
        }
        match ev {
            Event::Start(_) => depth += 1,
            Event::End(_) => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    out
}

/// The parse options `tui_markdown::from_str` uses; the scan must agree with the
/// renderer about what the document contains.
fn parse_options() -> ParseOptions {
    ParseOptions::ENABLE_STRIKETHROUGH
        | ParseOptions::ENABLE_TASKLISTS
        | ParseOptions::ENABLE_HEADING_ATTRIBUTES
        | ParseOptions::ENABLE_YAML_STYLE_METADATA_BLOCKS
        | ParseOptions::ENABLE_SUPERSCRIPT
        | ParseOptions::ENABLE_SUBSCRIPT
        | ParseOptions::ENABLE_MATH
        | ParseOptions::ENABLE_FOOTNOTES
        | ParseOptions::ENABLE_DEFINITION_LIST
        | ParseOptions::ENABLE_GFM
        | ParseOptions::ENABLE_TABLES
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(blocks: &[Block]) -> Vec<String> {
        blocks.iter().map(|b| b.text().unwrap_or_else(|| "<image>".into())).collect()
    }

    #[test]
    fn local_image_is_decoded_into_an_image_block() {
        let dir = std::env::temp_dir().join("wasdf-md-image-test");
        std::fs::create_dir_all(&dir).expect("temp dir");
        let png = dir.join("dot.png");
        image::RgbImage::from_pixel(4, 2, image::Rgb([1, 2, 3])).save(&png).expect("write png");

        let blocks = render(&dir.join("a.md"), "text\n\n![dot](dot.png)\n", 4);
        let img = blocks.iter().find(|b| matches!(b, Block::Image { .. })).expect("an image block");
        let Block::Image { width, height, rgb } = img else { unreachable!() };
        assert_eq!((*width, *height), (4, 2));
        assert_eq!(rgb.len(), 4 * 2 * 3);
        let _ = std::fs::remove_dir_all(&dir);
    }

}
