//! Syntax highlighting for text previews (syntect). Off the render path: this is
//! called by the blocking Preview task, which stores the resulting per-line
//! logical spans alongside the raw text in `Decoded::Text`. Colors are emitted as
//! literal `SpanStyle::Rgb` (syntax colors are not theme roles); core lays the
//! spans out.

use std::path::Path;
use std::sync::OnceLock;

use syntect::easy::HighlightLines;
use syntect::highlighting::{Theme, ThemeSet};
use syntect::parsing::SyntaxSet;

use crate::core::{SpanStyle, TextSpan};

/// The bundled syntect syntax set (newline variants), loaded once on first text
/// preview so startup pays nothing.
fn syntaxes() -> &'static SyntaxSet {
    static SET: OnceLock<SyntaxSet> = OnceLock::new();
    SET.get_or_init(SyntaxSet::load_defaults_newlines)
}

fn theme() -> &'static Theme {
    static THEME: OnceLock<Theme> = OnceLock::new();
    THEME.get_or_init(|| ThemeSet::load_defaults().themes["base16-ocean.dark"].clone())
}

/// Syntax-highlight `lines` (the raw text of a preview) into per-line logical
/// spans keyed by file extension. Each inner Vec fully covers its line. Returns an
/// empty outer Vec when no syntax matches the file, so the caller falls back to a
/// single plain span per line.
pub fn highlight_lines(path: &Path, lines: &[String]) -> Vec<Vec<TextSpan>> {
    let ss = syntaxes();
    let syntax = path
        .extension()
        .and_then(|e| e.to_str())
        .and_then(|ext| ss.find_syntax_by_extension(ext));
    let Some(syntax) = syntax else {
        return Vec::new();
    };
    let mut hl = HighlightLines::new(syntax, theme());
    let mut out = Vec::with_capacity(lines.len());
    for line in lines {
        // syntect's newline syntaxes expect a trailing '\n'; add one for parsing
        // and cap by the original length so span lengths sum to the line length.
        let with_nl = format!("{line}\n");
        let Ok(ranges) = hl.highlight_line(&with_nl, ss) else {
            out.push(vec![TextSpan::plain(line.clone())]);
            continue;
        };
        let mut spans = Vec::new();
        let mut pos = 0usize;
        for (style, piece) in ranges {
            if pos >= line.len() {
                break;
            }
            let end = (pos + piece.len()).min(line.len());
            let Some(text) = line.get(pos..end) else { break };
            let c = style.foreground;
            spans.push(TextSpan::styled(text, SpanStyle::Rgb { fg: (c.r, c.g, c.b), bg: None }));
            pos = end;
        }
        if pos < line.len()
            && let Some(rest) = line.get(pos..) {
                spans.push(TextSpan::plain(rest.to_string()));
            }
        out.push(spans);
    }
    out
}
