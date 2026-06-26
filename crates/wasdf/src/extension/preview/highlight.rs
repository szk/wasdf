//! Syntax highlighting for text previews (syntect). Off the render path: this is
//! called by the blocking Preview task, which stores the resulting per-line
//! style runs alongside the raw text in PreviewContent::Text.

use std::path::Path;
use std::sync::OnceLock;

use syntect::easy::HighlightLines;
use syntect::highlighting::{Theme, ThemeSet};
use syntect::parsing::SyntaxSet;

use crate::core::StyleRun;

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

/// Syntax-highlight `lines` (the raw text of a preview) into per-line style runs
/// keyed by file extension. Returns an empty Vec when no syntax matches the
/// file, so the renderer falls back to plain text.
pub fn highlight_lines(path: &Path, lines: &[String]) -> Vec<Vec<StyleRun>> {
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
        // and cap by the original length so run lengths sum to the line length.
        let mut runs = Vec::new();
        let with_nl = format!("{line}\n");
        let Ok(ranges) = hl.highlight_line(&with_nl, ss) else {
            out.push(Vec::new());
            continue;
        };
        let mut remaining = line.len();
        for (style, piece) in ranges {
            if remaining == 0 {
                break;
            }
            let len = piece.len().min(remaining);
            remaining -= len;
            let c = style.foreground;
            runs.push(StyleRun { len, fg: (c.r, c.g, c.b), bg: None });
        }
        out.push(runs);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn highlights_a_known_extension() {
        let lines = vec!["fn main() {}".to_string()];
        let styles = highlight_lines(Path::new("example.rs"), &lines);
        assert_eq!(styles.len(), 1);
        assert!(!styles[0].is_empty(), "rust source got highlight runs");
        // Run lengths tile the line exactly (kept in sync with the raw text).
        let total: usize = styles[0].iter().map(|r| r.len).sum();
        assert_eq!(total, lines[0].len());
    }

    #[test]
    fn unknown_extension_is_unstyled() {
        let styles = highlight_lines(Path::new("data.unknownext"), &["x".to_string()]);
        assert!(styles.is_empty(), "no syntax → plain text");
    }
}
