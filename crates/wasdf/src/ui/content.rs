//! The generic function-panel content blitter: turn `PanelContent`-shaped data
//! (styled text lines, directory listings, images) into ratatui `Line`s, with
//! search-match highlighting, horizontal scroll, and optional line numbers. This
//! is content-agnostic kernel infrastructure — any content extension's pushed
//! content and any provider's pulled content both render through it (it names no
//! particular extension). The function panel (`ui/middle.rs`) owns the chrome
//! around the returned lines.

use std::path::Path;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::core::{Entry, PanelSearch, StyleRun};

/// The panel title for the displayed file: its file name (not a fixed label), with
/// the match position appended while a search is active.
pub fn title(target: Option<&Path>, search: &PanelSearch) -> String {
    let base = target
        .and_then(|p| p.file_name())
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "content".into());
    if search.matches.is_empty() {
        base
    } else {
        format!("{base}  [{}/{}]", search.current + 1, search.matches.len())
    }
}

/// The `/` search prompt line, shown on the panel's bottom row while the input
/// is open; `None` when it is not.
pub fn prompt_line(search: &PanelSearch) -> Option<Line<'static>> {
    search.input_active.then(|| {
        // Block cursor at the real caret (the search input is the focused form →
        // reverse video).
        let q = &search.query;
        let caret = if q.is_char_boundary(search.caret.min(q.len())) {
            search.caret.min(q.len())
        } else {
            q.len()
        };
        let mut rest = q[caret..].chars();
        let under = rest.next().map(|c| c.to_string()).unwrap_or_else(|| " ".into());
        Line::from(vec![
            Span::styled("/", Style::default().fg(Color::Yellow)),
            Span::raw(q[..caret].to_string()),
            Span::styled(under, Style::default().add_modifier(Modifier::REVERSED)),
            Span::raw(rest.collect::<String>()),
        ])
    })
}

/// Build the styled content lines for a text/hex view: each line colored by its
/// `styles` runs (empty for hex/unstyled), scrolled horizontally by `hscroll`,
/// with search matches highlighted and optional line numbers. A `truncated`
/// marker line is appended when set.
pub fn text_lines(
    lines: &[String],
    styles: &[Vec<StyleRun>],
    search: &PanelSearch,
    hscroll: usize,
    show_line_numbers: bool,
    truncated: bool,
) -> Vec<Line<'static>> {
    let mut out: Vec<Line> = lines
        .iter()
        .enumerate()
        .map(|(i, text)| {
            let runs = styles.get(i).map(|v| v.as_slice()).unwrap_or(&[]);
            build_line(text, runs, hscroll, i, search, show_line_numbers)
        })
        .collect();
    if truncated {
        out.push(Line::from(Span::styled("… truncated", Style::default().fg(Color::DarkGray))));
    }
    out
}

/// The content's intrinsic width in columns: the longest line's char count,
/// measured on the raw (pre-h-scroll) lines. Callers pair this with the viewport
/// width to bound horizontal scrolling. Matches `build_line`'s per-char column
/// counting, so the bound lines up with what gets dropped on h-scroll.
pub fn content_width(lines: &[String]) -> usize {
    lines.iter().map(|l| l.chars().count()).max().unwrap_or(0)
}

/// Styled lines for a directory listing (directories blue, with a trailing `/`).
pub fn dir_lines(entries: &[Entry]) -> Vec<Line<'static>> {
    entries
        .iter()
        .map(|e| {
            let style = if e.is_dir { Style::default().fg(Color::Blue) } else { Style::default() };
            Line::from(Span::styled(format!("{}{}", e.name, if e.is_dir { "/" } else { "" }), style))
        })
        .collect()
}

/// One line: line number (optional), then the text from `hscroll` columns on,
/// colored by `runs` and overlaid with search-match highlighting.
fn build_line(
    text: &str,
    runs: &[StyleRun],
    hscroll: usize,
    line_idx: usize,
    search: &PanelSearch,
    show_line_numbers: bool,
) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    if show_line_numbers {
        spans.push(Span::styled(
            format!("{:>4} ", line_idx + 1),
            Style::default().fg(Color::DarkGray),
        ));
    }
    // Match byte ranges on this line, flagged when current.
    let line_matches: Vec<(usize, usize, bool)> = search
        .matches
        .iter()
        .enumerate()
        .filter_map(|(i, &(l, s, e))| (l == line_idx).then_some((s, e, i == search.current)))
        .collect();

    let mut run_idx = 0usize;
    let mut run_base = 0usize; // first byte covered by runs[run_idx]
    let mut col = 0usize;
    let mut cur_style = Style::default();
    let mut cur_text = String::new();
    for (b, ch) in text.char_indices() {
        while run_idx < runs.len() && run_base + runs[run_idx].len <= b {
            run_base += runs[run_idx].len;
            run_idx += 1;
        }
        let m = line_matches.iter().find(|(s, e, _)| b >= *s && b < *e);
        let style = match m {
            // Search highlight overrides the run's own colors.
            Some((_, _, true)) => Style::default().fg(Color::Black).bg(Color::LightCyan),
            Some(_) => Style::default().fg(Color::Black).bg(Color::Yellow),
            None => match runs.get(run_idx) {
                Some(r) => {
                    let mut st = Style::default().fg(Color::Rgb(r.fg.0, r.fg.1, r.fg.2));
                    if let Some((br, bg, bb)) = r.bg {
                        st = st.bg(Color::Rgb(br, bg, bb));
                    }
                    st
                }
                None => Style::default(),
            },
        };
        if col >= hscroll {
            if style != cur_style && !cur_text.is_empty() {
                spans.push(Span::styled(std::mem::take(&mut cur_text), cur_style));
            }
            cur_style = style;
            cur_text.push(ch);
        }
        col += 1;
    }
    if !cur_text.is_empty() {
        spans.push(Span::styled(cur_text, cur_style));
    }
    Line::from(spans)
}

/// Render a decoded RGB image into a `cols` × `rows` grid of terminal cells
/// using Chafa's symbol-rendering core (chafa-syms-rs): each cell picks the
/// Unicode symbol + fg/bg colors that best reconstruct its pixels. Returns one
/// styled Line per row.
pub fn image_cells(width: u32, height: u32, rgb: &[u8], cols: u16, rows: u16) -> Vec<Line<'static>> {
    if cols == 0 || rows == 0 || width == 0 || height == 0 || rgb.len() < (width * height * 3) as usize {
        return Vec::new();
    }
    use chafa_syms_rs::{Canvas, CanvasConfig, CanvasMode, PixelType};
    let cfg = CanvasConfig::new(cols as usize, rows as usize).mode(CanvasMode::Truecolor);
    let mut canvas = Canvas::new(cfg);
    canvas.draw_all_pixels(PixelType::Rgb8, rgb, width as usize, height as usize, (width * 3) as usize);
    let cells = canvas.cells();
    let (cols, rows) = (cols as usize, rows as usize);
    let mut lines = Vec::with_capacity(rows);
    for r in 0..rows {
        let mut spans = Vec::with_capacity(cols);
        for c in 0..cols {
            let Some(cell) = cells.get(r * cols + c) else { continue };
            let ch = char::from_u32(cell.c).filter(|c| *c != '\0').unwrap_or(' ');
            let fg = Color::Rgb((cell.fg >> 16) as u8, (cell.fg >> 8) as u8, cell.fg as u8);
            let bg = Color::Rgb((cell.bg >> 16) as u8, (cell.bg >> 8) as u8, cell.bg as u8);
            spans.push(Span::styled(ch.to_string(), Style::default().fg(fg).bg(bg)));
        }
        lines.push(Line::from(spans));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_image_to_chafa_cells() {
        let mut rgb = Vec::new();
        for y in 0..8u32 {
            for _ in 0..8u32 {
                rgb.extend_from_slice(&[(y * 30) as u8, 0, 255 - (y * 30) as u8]);
            }
        }
        let lines = image_cells(8, 8, &rgb, 6, 4);
        assert_eq!(lines.len(), 4, "one Line per row");
        assert_eq!(lines[0].spans.len(), 6, "one cell per column");
    }

    #[test]
    fn degenerate_input_is_empty() {
        assert!(image_cells(0, 0, &[], 4, 4).is_empty());
        assert!(image_cells(2, 2, &[0; 12], 0, 0).is_empty());
    }

    #[test]
    fn title_is_the_file_name_with_match_position() {
        let mut search = PanelSearch::default();
        let path = std::path::PathBuf::from("/a/b/main.rs");
        assert_eq!(title(Some(&path), &search), "main.rs");
        search.matches = vec![(0, 0, 1), (2, 0, 1)];
        search.current = 1;
        assert_eq!(title(Some(&path), &search), "main.rs  [2/2]");
    }

    #[test]
    fn content_width_is_the_longest_line() {
        assert_eq!(content_width(&["ab".into(), "abcd".into(), "a".into()]), 4);
        assert_eq!(content_width(&[]), 0);
    }

    #[test]
    fn horizontal_scroll_drops_leading_columns() {
        let search = PanelSearch::default();
        let l = text_lines(&["hello".to_string()], &[], &search, 2, false, false);
        let text: String = l[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "llo");
    }
}
