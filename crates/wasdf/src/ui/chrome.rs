//! Panel chrome: one connected single-line border drawn from all panel rects,
//! titles embedded in the top border, and a ratatui Scrollbar in place of each
//! scrollable panel's right border (no begin/end arrows). See doc/UI.md
//! (Borders and Scrollbars, Screen chrome) for the intended look.
//!
//! ratatui's per-widget `Block` borders cannot merge where panels meet (they
//! double up and form no junctions), so the border is rendered here as a single
//! layer: each panel rect contributes its outline to a per-cell connection mask,
//! and the box-drawing glyph for each cell follows from the directions that meet
//! there — yielding `┬ ┴ ├ ┤ ┼` automatically at shared edges.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::widgets::{Scrollbar, ScrollbarOrientation, ScrollbarState};

use crate::core::{Role, Theme};

/// The "this one" / "done" marker (the active theme, the Confirm row). ASCII
/// on purpose: ✓/✔ are missing from small bitmap fonts (the CardputerZero's
/// PCF), and ●/◆/☑ are East Asian ambiguous width — two cells on a terminal
/// set to ambiguous-wide, one in ratatui's count, which breaks the row.
pub const CHECK_MARK: char = '*';

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

/// The glyphs used to draw a panel's right-border scrollbar, configurable under
/// `@/settings/Display`.
#[derive(Clone)]
pub struct ScrollbarChars {
    /// The moving block.
    pub thumb: String,
    /// The track behind the thumb (when the panel is scrollable).
    pub rail: String,
    /// What fills the column when scrolling is unnecessary (the panel fully fits).
    pub disabled: String,
}

impl Default for ScrollbarChars {
    fn default() -> Self {
        ScrollbarChars { thumb: "█".into(), rail: "│".into(), disabled: "│".into() }
    }
}

impl ScrollbarChars {
    /// First char of a configured glyph, falling back to `default` when empty.
    fn glyph(s: &str, default: char) -> char {
        s.chars().next().unwrap_or(default)
    }
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
    /// with no closing line or corners — see doc/UI.md, where the file
    /// and function panels open directly onto the unboxed help row below.
    pub open_bottom: bool,
    /// When true, the panel has no left border: its content runs flush to the
    /// screen's left column — see doc/UI.md, where the main (file/select)
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

/// Text laid on a border line that belongs to a panel other than the one whose
/// frame drew the line: the file panel's path and the function panel's file
/// name on the file panel's top border (doc/UI.md). Painted after every title,
/// from column `x`, never past column `right`; themed with its owner's focus.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BorderLabel {
    pub x: u16,
    pub y: u16,
    pub right: u16,
    pub text: String,
    pub focused: bool,
}

/// Where a panel sits in the solved layout: its rect and which of its edges
/// are left open (doc/UI.md, Screen chrome). A panel draws its frame and finds
/// its content rect through this, never by assuming its position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PanelArea {
    pub rect: Rect,
    pub open_left: bool,
    pub open_bottom: bool,
}

impl PanelArea {
    pub fn closed(rect: Rect) -> Self {
        PanelArea { rect, open_left: false, open_bottom: false }
    }

    pub fn inner(&self) -> Rect {
        if self.open_bottom { inner_open_bottom(self.rect) } else { inner(self.rect) }
    }

    pub fn frame(&self) -> PanelFrame {
        PanelFrame::new(self.rect).open_left(self.open_left).open_bottom(self.open_bottom)
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

/// The horizontal rule glyph for a border style — the only character the
/// bottom-border painter should treat as border padding (so a literal `-` inside
/// a path/name is rendered as label text, not mistaken for the rule).
fn rule_char(style: &str) -> char {
    match style {
        "double" => '═',
        "ascii" => '-',
        _ => '─', // single, rounded, and any unrecognised value
    }
}

fn glyph(mask: u8, style: &str) -> char {
    // bits: N=1 S=2 E=4 W=8
    match style {
        "rounded" => match mask {
            6 => '╭', 10 => '╮', 5 => '╰', 9 => '╯',
            1..=3 => '│', 12 | 4 | 8 => '─',
            7 => '├', 11 => '┤', 14 => '┬', 13 => '┴', 15 => '┼',
            _ => ' ',
        },
        "double" => match mask {
            1..=3 => '║', 12 | 4 | 8 => '═',
            6 => '╔', 10 => '╗', 5 => '╚', 9 => '╝',
            7 => '╠', 11 => '╣', 14 => '╦', 13 => '╩', 15 => '╬',
            _ => ' ',
        },
        "ascii" => match mask {
            3 | 1 | 2 | 7 | 11 => '|',
            12 | 4 | 8 | 14 | 13 => '-',
            6 | 10 | 5 | 9 | 15 => '+',
            _ => ' ',
        },
        _ => match mask { // "single" and any unrecognised value
            1..=3 => '│', 12 | 4 | 8 => '─',
            6 => '┌', 10 => '┐', 5 => '└', 9 => '┘',
            7 => '├', 11 => '┤', 14 => '┬', 13 => '┴', 15 => '┼',
            _ => ' ',
        },
    }
}

/// Render the connected border, titles, and scrollbars for all panels.
pub fn render(
    frame: &mut Frame,
    panels: &[PanelFrame],
    labels: &[BorderLabel],
    border_style: &str,
    scrollbar: &ScrollbarChars,
    theme: &Theme,
) {
    let area = frame.area();
    let (w, h) = (area.width as usize, area.height as usize);
    if w == 0 || h == 0 {
        return;
    }
    let mut mask = vec![0u8; w * h];

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
                let style = theme.style(Role::Border);
                let x = area.x + cx as u16;
                let y = area.y + cy as u16;
                if let Some(cell) = buf.cell_mut((x, y)) {
                    cell.set_char(glyph(m, border_style));
                    cell.set_style(style);
                }
            }
        }

        // Titles embedded in the top border: ─[ title ]─. The `[ ` / ` ]`
        // brackets carry their own role so they can be themed apart from the
        // title text (banners are unbracketed and use the title role only).
        for p in panels {
            let Some(title) = &p.title else { continue };
            if p.rect.width < 6 {
                continue;
            }
            // A banner (unbracketed title, e.g. the top-left `/ wasdf … /`) is the
            // nameplate; bracketed panel titles use the title / title-bracket roles.
            let text_style = if !p.bracket {
                theme.style(Role::Nameplate)
            } else if p.focused {
                theme.style(Role::TitleFocused)
            } else {
                theme.style(Role::Title)
            };
            let bracket_style = if p.focused {
                theme.style(Role::TitleBracketFocused)
            } else {
                theme.style(Role::TitleBracket)
            };
            let label = if p.bracket { format!("[ {title} ]") } else { title.clone() };
            let full_len = label.chars().count();
            let max = p.rect.width.saturating_sub(4);
            let label = crate::ui::text::take_width(&label, max as usize);
            let y = p.rect.y;
            let end = p.rect.x + 2 + max;
            let mut x = p.rect.x + 2;
            for (i, ch) in label.chars().enumerate() {
                // The first two (`[ `) and last two (` ]`) chars are brackets.
                let is_bracket = p.bracket && (i < 2 || i >= full_len.saturating_sub(2));
                let style = if is_bracket { bracket_style } else { text_style };
                let cw = crate::ui::text::char_width(ch) as u16;
                // Only skip cells where a vertical border continues southward
                // into this panel's content (S bit set). An N-only junction (a
                // divider from an adjacent panel above terminating here) is safe
                // to overwrite — the title should remain readable. A wide
                // character is skipped whole if either of its cells is one.
                let junction = (x..x + cw).any(|cx| {
                    let (cx, cy) = ((cx - area.x) as usize, (y - area.y) as usize);
                    cx < w && cy < h && mask[cy * w + cx] & S != 0
                });
                if !junction {
                    crate::ui::text::put_char(buf, x, y, ch, style, end - x);
                }
                x += cw;
            }
        }

        // Info embedded in the top border, right-aligned (e.g. the prop panel's
        // load and clock readout). It is one or more bracketed labels `[…]`
        // joined and trailed by border rule (`─`): the brackets take the
        // title-bracket role, the text inside them the title role, and whatever
        // lies outside them the border color. (By bracket depth, so a `-` inside
        // a date is never mistaken for an ascii rule.)
        for p in panels {
            let Some(info) = &p.info else { continue };
            if p.rect.width < 6 {
                continue;
            }
            let title_style = if p.focused {
                theme.style(Role::TitleFocused)
            } else {
                theme.style(Role::Title)
            };
            let bracket_style = if p.focused {
                theme.style(Role::TitleBracketFocused)
            } else {
                theme.style(Role::TitleBracket)
            };
            let border_c = theme.style(Role::Border);
            let max = p.rect.width.saturating_sub(4) as usize;
            let info = crate::ui::text::take_width(info, max);
            let len = crate::ui::text::width(&info) as u16;
            let y = p.rect.y;
            let start = (p.rect.x + p.rect.width).saturating_sub(1 + len).max(p.rect.x + 2);
            let end = start + len;
            let mut inside = false;
            let mut x = start;
            for ch in info.chars() {
                let cw = crate::ui::text::char_width(ch) as u16;
                let style = match ch {
                    '[' => {
                        inside = true;
                        bracket_style
                    }
                    ']' => {
                        inside = false;
                        bracket_style
                    }
                    _ if inside => title_style,
                    _ => border_c,
                };
                let junction = (x..x + cw).any(|cx| {
                    let (cx, cy) = ((cx - area.x) as usize, (y - area.y) as usize);
                    cx < w && cy < h && mask[cy * w + cx] & S != 0
                });
                if !junction {
                    crate::ui::text::put_char(buf, x, y, ch, style, end - x);
                }
                x += cw;
            }
        }

        // Border labels (the path, and the file name after it).
        //
        // The line a label sits on is usually shared — the top panel's bottom
        // border is the main panels' top border — so a function-panel title may
        // already have been painted here.
        // Three outcomes per cell, in priority order:
        //
        //   1. A real label character (anything but a horizontal rule — brackets
        //      and the path/name text) always wins, in the Nameplate color. It
        //      overwrites the border glyph and any bled title, so the path and
        //      filename stay intact even where a divider or a title char sits.
        //   2. A rule/padding character (`═` etc.) is part of the border line, so
        //      it takes the **Border** color, not the Nameplate color — whether it
        //      is the `─` padding/separators woven between `]…[` labels or a
        //      junction stem. Over a *junction* cell (any vertical stem, N or S)
        //      it repaints the connected junction glyph (`┴`/`╦`), which also
        //      erases any function title that bled onto this shared row.
        //
        // A label is clamped to its own right edge (the end of the line it sits on).
        let paint_bottom = |buf: &mut ratatui::buffer::Buffer, mut x: u16, y: u16, frame_right: u16, text: &str, focused: bool| {
            // The bracketed path/name labels are themed like panel titles: the
            // `[`/`]` brackets take the title-bracket role, the inner text the
            // title role, and the rule/padding the border color. They follow the
            // owning panel's focus the same way top-border titles do, so the path
            // shows the focused colors when the file panel holds focus.
            let title_style = theme.style(if focused {
                Role::TitleFocused
            } else {
                Role::Title
            });
            let bracket_style = theme.style(if focused {
                Role::TitleBracketFocused
            } else {
                Role::TitleBracket
            });
            let border_style_fg = theme.style(Role::Border);
            let rule = rule_char(border_style);
            for ch in text.chars() {
                let cw = crate::ui::text::char_width(ch) as u16;
                if x + cw > frame_right {
                    x += cw;
                    continue;
                }
                let (cx, cy) = ((x - area.x) as usize, (y - area.y) as usize);
                let m = if cx < w && cy < h { mask[cy * w + cx] } else { 0 };
                // Only the active border style's rule glyph is padding; a literal
                // `-` in a directory name is label text, not a border rule.
                let is_rule = ch == rule;
                let stem = m & (N | S) != 0;
                if let Some(cell) = buf.cell_mut((x, y)) {
                    if is_rule {
                        // Border-line rule/padding: Border color. Restore the
                        // connected junction glyph where a stem crosses here.
                        cell.set_char(if stem { glyph(m, border_style) } else { ch });
                        cell.set_style(border_style_fg);
                    } else if ch == '[' || ch == ']' {
                        cell.set_char(ch);
                        cell.set_style(bracket_style);
                    } else {
                        crate::ui::text::put_char(buf, x, y, ch, title_style, frame_right - x);
                    }
                }
                x += cw;
            }
        };
        for l in labels {
            paint_bottom(buf, l.x, l.y, l.right, &l.text, l.focused);
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
        let style = theme.style(Role::Border);

        // Disabled: the panel fully fits, so a scrollbar is meaningless — fill the
        // column with the configured disabled glyph (by default a plain border).
        if s.total <= s.viewport {
            let ch = ScrollbarChars::glyph(&scrollbar.disabled, '│');
            let buf = frame.buffer_mut();
            for y in track.y..track.y + track.height {
                if let Some(cell) = buf.cell_mut((track.x, y)) {
                    cell.set_char(ch);
                    cell.set_style(style);
                }
            }
            continue;
        }

        // ratatui maps `position` over `0..content_length`, reaching the track
        // bottom only when the *last row* is at the viewport top. Our `position`
        // is the scroll offset (the top visible row), which maxes out at
        // total - viewport (the last row at the viewport *bottom*). So feed it
        // the number of distinct offsets as the content length: then offset 0
        // puts the thumb at the top, the max offset puts it flush at the bottom,
        // and the thumb length stays proportional to viewport / total.
        let positions = s.total.saturating_sub(s.viewport) + 1;
        let mut state = ScrollbarState::new(positions)
            .viewport_content_length(s.viewport)
            .position(s.position.min(positions - 1));
        let thumb = scrollbar.thumb.chars().next().map(|_| scrollbar.thumb.as_str()).unwrap_or("█");
        let rail = if scrollbar.rail.is_empty() { "│" } else { scrollbar.rail.as_str() };
        let bar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(None)
            .end_symbol(None)
            .track_symbol(Some(rail))
            .thumb_symbol(thumb)
            .style(style);
        frame.render_stateful_widget(bar, track, &mut state);
    }
}
