//! Generic function-panel rendering helpers: the panel title, the search prompt
//! line, the image blitter, and [`build_line`] — the single low-level line
//! painter that applies horizontal scroll, optional line numbers, and
//! search-match highlighting over a text line's pre-resolved style segments. This
//! is content-agnostic kernel infrastructure; the `PanelView` layout
//! (`ui/middle/function.rs`) lowers logical blocks into `(byte-len, Style)`
//! segments and calls `build_line`. The function panel owns the chrome.

use std::path::Path;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::core::{PanelSearch, Role, Theme, LINE_NUMBER_WIDTH};

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
pub fn prompt_line(search: &PanelSearch, theme: &Theme) -> Option<Line<'static>> {
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
            Span::styled("/", theme.style(Role::PathSeparator)),
            Span::raw(q[..caret].to_string()),
            Span::styled(under, Style::default().add_modifier(Modifier::REVERSED)),
            Span::raw(rest.collect::<String>()),
        ])
    })
}

/// Paint one text line: an optional line number, then the text from `hscroll`
/// columns on, colored by its pre-resolved style `segs` (`(byte-len, Style)`,
/// tiling the line left-to-right; a gap past the last seg is unstyled) and
/// overlaid with the search matches on this row (`highlights`: byte ranges
/// in `text`, flagged when current). The `PanelView` layout
/// lowers each logical block into `(text, segs)` and calls this. Widths are
/// counted per char to match `line_width`, so the h-scroll bound lines up.
pub fn build_line(
    text: &str,
    segs: &[(usize, Style)],
    hscroll: usize,
    line_no: usize,
    highlights: &[(usize, usize, bool)],
    show_line_numbers: bool,
    theme: &Theme,
) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    if show_line_numbers {
        spans.push(Span::styled(
            format!("{:>w$} ", line_no + 1, w = LINE_NUMBER_WIDTH as usize - 1),
            theme.style(Role::Muted),
        ));
    }
    // Match byte ranges on this line, flagged when current.

    let mut seg_idx = 0usize;
    let mut seg_base = 0usize; // first byte covered by segs[seg_idx]
    let mut cur_style = Style::default();
    let mut cur_text = String::new();
    for (col, (b, ch)) in text.char_indices().enumerate() {
        while seg_idx < segs.len() && seg_base + segs[seg_idx].0 <= b {
            seg_base += segs[seg_idx].0;
            seg_idx += 1;
        }
        let m = highlights.iter().find(|(s, e, _)| b >= *s && b < *e);
        let style = match m {
            // Search highlight overrides the segment's own colors.
            Some((_, _, true)) => theme.style(Role::MatchCurrentFg).bg(theme.role(Role::MatchCurrentBg)),
            Some(_) => theme.style(Role::MatchFg).bg(theme.role(Role::MatchBg)),
            None => segs.get(seg_idx).map(|s| s.1).unwrap_or_default(),
        };
        if col >= hscroll {
            if style != cur_style && !cur_text.is_empty() {
                spans.push(Span::styled(std::mem::take(&mut cur_text), cur_style));
            }
            cur_style = style;
            cur_text.push(ch);
        }
    }
    if !cur_text.is_empty() {
        spans.push(Span::styled(cur_text, cur_style));
    }
    Line::from(spans)
}

/// A text line's width in columns (char count) — pairs with the viewport to
/// bound horizontal scrolling, matching `build_line`'s per-char counting.
pub fn line_width(text: &str) -> usize {
    text.chars().count()
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
