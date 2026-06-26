//! Panel chrome: one connected single-line border drawn from all panel rects,
//! titles embedded in the top border, and a ratatui Scrollbar in place of each
//! scrollable panel's right border (no begin/end arrows). See doc/UI.md
//! (Borders and Scrollbars) and doc/SCREEN.md for the intended look.
//!
//! ratatui's per-widget `Block` borders cannot merge where panels meet (they
//! double up and form no junctions), so the border is rendered here as a single
//! layer: each panel rect contributes its outline to a per-cell connection mask,
//! and the box-drawing glyph for each cell follows from the directions that meet
//! there — yielding `┬ ┴ ├ ┤ ┼` automatically at shared edges.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::widgets::{Scrollbar, ScrollbarOrientation, ScrollbarState};

const N: u8 = 1;
const S: u8 = 2;
const E: u8 = 4;
const W: u8 = 8;

/// Scroll state for a panel's right-border scrollbar.
#[derive(Clone, Copy)]
pub struct ScrollInfo {
    pub total: usize,
    pub viewport: usize,
    pub position: usize,
}

/// A panel to frame: its outer rect, optional title (left of the top border),
/// optional info (right of the top border), focus, and scroll state.
pub struct PanelFrame {
    pub rect: Rect,
    pub title: Option<String>,
    /// When false, the title is drawn verbatim (a banner); otherwise it is
    /// wrapped as `[ title ]`.
    pub bracket: bool,
    pub info: Option<String>,
    pub focused: bool,
    pub scroll: Option<ScrollInfo>,
    /// When true, the panel has no bottom border: the side borders run down
    /// through its last row (a content row, not a separator) and stop there
    /// with no closing line or corners — see doc/SCREEN.md, where the file
    /// and function panels open directly onto the unboxed help row below.
    pub open_bottom: bool,
    /// When true, the panel has no left border: its content runs flush to the
    /// screen's left column — see doc/SCREEN.md, where the main (file/select)
    /// area has no left edge, so the top panel's bottom-left corner is `└`
    /// rather than `├`.
    pub open_left: bool,
}

impl PanelFrame {
    pub fn new(rect: Rect) -> Self {
        PanelFrame {
            rect,
            title: None,
            bracket: true,
            info: None,
            focused: false,
            scroll: None,
            open_bottom: false,
            open_left: false,
        }
    }
    pub fn title(mut self, t: impl Into<String>) -> Self {
        self.title = Some(t.into());
        self
    }
    /// A verbatim (unbracketed) title, e.g. the app banner.
    pub fn banner(mut self, t: impl Into<String>) -> Self {
        self.title = Some(t.into());
        self.bracket = false;
        self
    }
    pub fn info(mut self, t: impl Into<String>) -> Self {
        self.info = Some(t.into());
        self
    }
    pub fn focused(mut self, f: bool) -> Self {
        self.focused = f;
        self
    }
    pub fn scroll(mut self, s: ScrollInfo) -> Self {
        self.scroll = Some(s);
        self
    }
    pub fn open_bottom(mut self, v: bool) -> Self {
        self.open_bottom = v;
        self
    }
    pub fn open_left(mut self, v: bool) -> Self {
        self.open_left = v;
        self
    }
}

/// The inner content rect of a panel (one cell inset on every side).
pub fn inner(rect: Rect) -> Rect {
    Rect {
        x: rect.x + 1,
        y: rect.y + 1,
        width: rect.width.saturating_sub(2),
        height: rect.height.saturating_sub(2),
    }
}

/// The inner content rect of an open-bottom panel: inset on top and sides,
/// but flush with the panel's last row since there is no bottom border to
/// leave room for.
pub fn inner_open_bottom(rect: Rect) -> Rect {
    Rect {
        x: rect.x + 1,
        y: rect.y + 1,
        width: rect.width.saturating_sub(2),
        height: rect.height.saturating_sub(1),
    }
}

fn glyph(mask: u8) -> char {
    // bits: N=1 S=2 E=4 W=8
    match mask {
        3 => '│',
        12 => '─',
        6 => '┌',
        10 => '┐',
        5 => '└',
        9 => '┘',
        7 => '├',
        11 => '┤',
        14 => '┬',
        13 => '┴',
        15 => '┼',
        1 | 2 => '│',
        4 | 8 => '─',
        _ => ' ',
    }
}

/// Render the connected border, titles, and scrollbars for all panels.
pub fn render(frame: &mut Frame, panels: &[PanelFrame]) {
    let area = frame.area();
    let (w, h) = (area.width as usize, area.height as usize);
    if w == 0 || h == 0 {
        return;
    }
    let mut mask = vec![0u8; w * h];
    let mut focus = vec![false; w * h];

    // Accumulate each panel's outline into the connection mask.
    for p in panels {
        let r = p.rect;
        if r.width < 2 || r.height < 2 {
            continue;
        }
        let (x0, y0) = (r.x, r.y);
        let (x1, y1) = (r.x + r.width - 1, r.y + r.height - 1);
        let mut put = |x: u16, y: u16, bits: u8| {
            if x >= area.x && y >= area.y {
                let (cx, cy) = ((x - area.x) as usize, (y - area.y) as usize);
                if cx < w && cy < h {
                    mask[cy * w + cx] |= bits;
                    if p.focused {
                        focus[cy * w + cx] = true;
                    }
                }
            }
        };
        // Interior edges only (corners handled separately so they don't pick
        // up the perpendicular edge bits and become spurious junctions). An
        // open-bottom panel omits the bottom edge and its corners; its side
        // borders run down through y1 instead, ending there as plain `│`. An
        // open-left panel omits the left edge and its corners entirely; the top
        // edge then carries only its eastward arm at x0 so the line still meets
        // a neighbour above (e.g. the top panel) without a southward stem.
        for x in (x0 + 1)..x1 {
            put(x, y0, E | W);
            if !p.open_bottom {
                put(x, y1, E | W);
            }
        }
        let side_end = if p.open_bottom { y1 } else { y1.saturating_sub(1) };
        for y in (y0 + 1)..=side_end {
            if !p.open_left {
                put(x0, y, N | S);
            }
            put(x1, y, N | S);
        }
        if p.open_left {
            put(x0, y0, E);
        } else {
            put(x0, y0, S | E);
        }
        put(x1, y0, S | W);
        if !p.open_bottom {
            if !p.open_left {
                put(x0, y1, N | E);
            }
            put(x1, y1, N | W);
        }
    }

    {
        let buf = frame.buffer_mut();
        for cy in 0..h {
            for cx in 0..w {
                let m = mask[cy * w + cx];
                if m == 0 {
                    continue;
                }
                let style = if focus[cy * w + cx] {
                    Style::default().fg(Color::Cyan)
                } else {
                    Style::default().fg(Color::DarkGray)
                };
                let x = area.x + cx as u16;
                let y = area.y + cy as u16;
                if let Some(cell) = buf.cell_mut((x, y)) {
                    cell.set_char(glyph(m));
                    cell.set_style(style);
                }
            }
        }

        // Titles embedded in the top border: ─[ title ]─.
        for p in panels {
            let Some(title) = &p.title else { continue };
            if p.rect.width < 6 {
                continue;
            }
            let style = if p.focused {
                Style::default().fg(Color::Cyan)
            } else {
                Style::default().fg(Color::Gray)
            };
            let label = if p.bracket { format!("[ {title} ]") } else { title.clone() };
            let max = p.rect.width.saturating_sub(4) as usize;
            let label: String = label.chars().take(max).collect();
            let mut x = p.rect.x + 2;
            let y = p.rect.y;
            for ch in label.chars() {
                let (cx, cy) = ((x - area.x) as usize, (y - area.y) as usize);
                // Leave junction cells (a divider meeting this row from above or
                // below) intact so a long title flows around them.
                let junction = cx < w && cy < h && mask[cy * w + cx] & (N | S) != 0;
                if !junction {
                    if let Some(cell) = buf.cell_mut((x, y)) {
                        cell.set_char(ch);
                        cell.set_style(style);
                    }
                }
                x += 1;
            }
        }

        // Info embedded in the top border, right-aligned (e.g. load + clock).
        for p in panels {
            let Some(info) = &p.info else { continue };
            if p.rect.width < 6 {
                continue;
            }
            let max = p.rect.width.saturating_sub(4) as usize;
            let label: String = info.chars().take(max).collect();
            let len = label.chars().count() as u16;
            let y = p.rect.y;
            let mut x = (p.rect.x + p.rect.width).saturating_sub(1 + len).max(p.rect.x + 2);
            let style = Style::default().fg(Color::Gray);
            for ch in label.chars() {
                let (cx, cy) = ((x - area.x) as usize, (y - area.y) as usize);
                let junction = cx < w && cy < h && mask[cy * w + cx] & (N | S) != 0;
                if !junction {
                    if let Some(cell) = buf.cell_mut((x, y)) {
                        cell.set_char(ch);
                        cell.set_style(style);
                    }
                }
                x += 1;
            }
        }
    }

    // Right-border scrollbars (track + thumb only; no arrows).
    for p in panels {
        let Some(s) = p.scroll else { continue };
        if p.rect.width < 2 || p.rect.height < 3 {
            continue;
        }
        let track_h = if p.open_bottom { p.rect.height - 1 } else { p.rect.height - 2 };
        let track = Rect {
            x: p.rect.x + p.rect.width - 1,
            y: p.rect.y + 1,
            width: 1,
            height: track_h,
        };
        let style = if p.focused {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        // ratatui maps `position` over `0..content_length`, reaching the track
        // bottom only when the *last row* is at the viewport top. Our `position`
        // is the scroll offset (the top visible row), which maxes out at
        // total - viewport (the last row at the viewport *bottom*). So feed it
        // the number of distinct offsets as the content length: then offset 0
        // puts the thumb at the top, the max offset puts it flush at the bottom,
        // and the thumb length stays proportional to viewport / total. Content
        // that fits (total <= viewport) gives one position and a full-height thumb.
        let positions = s.total.saturating_sub(s.viewport) + 1;
        let mut state = ScrollbarState::new(positions)
            .viewport_content_length(s.viewport)
            .position(s.position.min(positions - 1));
        let bar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(None)
            .end_symbol(None)
            .track_symbol(Some("│"))
            .thumb_symbol("█")
            .style(style);
        frame.render_stateful_widget(bar, track, &mut state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    /// Render one framed panel and return the glyphs down its right-border
    /// (scrollbar) column, top to bottom.
    fn scrollbar_column(scroll: ScrollInfo) -> Vec<char> {
        let rect = Rect { x: 0, y: 0, width: 10, height: 12 };
        let mut t = Terminal::new(TestBackend::new(10, 12)).unwrap();
        t.draw(|f| render(f, &[PanelFrame::new(rect).scroll(scroll)])).unwrap();
        let buf = t.backend().buffer();
        let x = rect.x + rect.width - 1;
        (1..rect.height - 1).map(|y| buf[(x, y)].symbol().chars().next().unwrap()).collect()
    }

    // doc/UI.md, point 1: when the bottom of the scroll range is at the bottom of
    // the viewport (offset == total - viewport), the thumb sits flush at the
    // bottom of the track.
    #[test]
    fn thumb_reaches_the_bottom_at_max_offset() {
        let col = scrollbar_column(ScrollInfo { total: 100, viewport: 10, position: 90 });
        assert_eq!(*col.last().unwrap(), '█', "thumb at the very bottom: {col:?}");
        assert_eq!(col[0], '│', "track (not thumb) at the top: {col:?}");
    }

    #[test]
    fn thumb_sits_at_the_top_at_offset_zero() {
        let col = scrollbar_column(ScrollInfo { total: 100, viewport: 10, position: 0 });
        assert_eq!(col[0], '█', "thumb at the top: {col:?}");
        assert_eq!(*col.last().unwrap(), '│', "track (not thumb) at the bottom: {col:?}");
    }

    #[test]
    fn content_that_fits_is_a_full_thumb() {
        let col = scrollbar_column(ScrollInfo { total: 5, viewport: 10, position: 0 });
        assert!(col.iter().all(|&c| c == '█'), "full-height thumb when content fits: {col:?}");
    }
}
