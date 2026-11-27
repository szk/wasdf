//! The main (central) area: the file and select layouts and every panel they
//! host — the file list, the select input and candidate list, and the function
//! panel with its extension content / exec / command-summary frames (see
//! doc/UI.md). Panels are stateless: they read the read-only AppState, the
//! render caches, and the focused panel id. Renderer selection is native Rust
//! dispatch — no Scheme round trip at render time.
//!
//! Panels are drawn as borderless content into their inner rects; the connected
//! single-line border, titles, and right-edge scrollbars are overlaid by the
//! chrome layer (see chrome.rs). Main-area panels open onto the unboxed help row
//! below them, so they use `inner_open_bottom` and set `open_bottom`.

use ratatui::Frame;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, ListState, Paragraph};

use crate::core::{AppState, Role, SelectPhase, Theme, PANEL_FUNCTION};
use crate::extension::ExtensionRegistry;
use crate::ui::chrome::{PanelArea, PanelFrame, ScrollInfo};

mod function;

pub use function::render_function;
use function::is_command_select;

/// Persistent top-of-window scroll offsets for the cursor-driven lists (the file
/// list and the select candidate list). Kept across frames so a list scrolls
/// only when the cursor would leave the visible window — never on every cursor
/// step (doc/UI.md). Render-only state owned by UiManager; the reducer never
/// reads it.
pub struct ScrollMemory {
    file: usize,
    candidates: usize,
    /// The function panel's maximum scroll offset measured at the last render
    /// (content rows − viewport rows). The function-panel scroll is kernel-owned
    /// AppState the reducer advances blindly (it cannot see the viewport); the
    /// kernel clamps `function.scroll` to this after each draw so over-scroll
    /// input is discarded rather than left to drift. `usize::MAX` means "not a
    /// scrollable frame this render — do not clamp".
    function_max: usize,
    /// The function panel's maximum *horizontal* scroll offset at the last render
    /// (content columns − viewport columns); same contract as `function_max`.
    function_hmax: usize,
    /// The file-list geometry measured at the last render (column count × visual
    /// rows). The kernel copies this into `AppState.list_geom` after each draw so
    /// the pure reducer can move the cursor in 2-D matching the screen.
    list_cols: usize,
    list_rows: usize,
    /// The function panel's title as of the last render — whatever actually
    /// owns the panel this frame (a preview's file name, a form extension's own
    /// title). The top area draws this on the border it shares with the panel
    /// (see `ui/top.rs`), and it has no other way to know what the panel became:
    /// `AppState` records *who* owns the panel, never what they chose to call
    /// it. `None` when nothing is drawing a titled function panel.
    function_title: Option<String>,
    /// The panels the last frame laid out — how the kernel learns whether a
    /// panel with a data source of its own (the monitor) is on screen.
    shown: Vec<String>,
    /// The options generation this frame renders at (the theme follows it).
    generation: u64,
    /// The function panel's lowered content (see `function::Lowered`).
    lowered: Option<function::Lowered>,
    /// The scroll this draw used to reveal the current search match, for the
    /// kernel to keep (the reducer cannot place a block on a row).
    revealed: Option<usize>,
}

impl Default for ScrollMemory {
    fn default() -> Self {
        ScrollMemory {
            file: 0,
            candidates: 0,
            function_max: usize::MAX,
            function_hmax: usize::MAX,
            list_cols: 1,
            list_rows: 0,
            function_title: None,
            shown: Vec::new(),
            generation: 0,
            lowered: None,
            revealed: None,
        }
    }
}

impl ScrollMemory {
    /// The file-list geometry measured at the last render, for the kernel's
    /// post-draw refresh of `AppState.list_geom`.
    pub fn list_geom(&self) -> crate::core::ListGeom {
        crate::core::ListGeom { cols: self.list_cols, rows: self.list_rows }
    }

    /// The function panel's max vertical scroll offset measured at the last render.
    pub fn function_max(&self) -> usize {
        self.function_max
    }

    /// The function panel's max horizontal scroll offset measured at the last render.
    pub fn function_hmax(&self) -> usize {
        self.function_hmax
    }

    /// Mark this render as having no function-panel scroll bounds yet (set before
    /// drawing; a scrollable frame overwrites them via the function render path).
    pub fn reset_function_bounds(&mut self) {
        self.function_max = usize::MAX;
        self.function_hmax = usize::MAX;
        self.function_title = None;
    }

    /// Record the title the function panel is drawn with this frame. Every
    /// producer of a function-panel `PanelFrame` calls this, so the border label
    /// the top area draws cannot drift from the panel's own title.
    pub fn set_function_title(&mut self, title: &str) {
        self.function_title = Some(title.to_string());
    }

    pub fn function_title(&self) -> Option<&str> {
        self.function_title.as_deref()
    }

    pub fn take_revealed(&mut self) -> Option<usize> {
        self.revealed.take()
    }

    pub fn set_generation(&mut self, generation: u64) {
        self.generation = generation;
    }

    pub fn set_shown(&mut self, panels: Vec<String>) {
        self.shown = panels;
    }

    pub fn shows(&self, panel: &str) -> bool {
        self.shown.iter().any(|p| p == panel)
    }
}

/// The maximum scroll offset that keeps content on screen: the last row rests at
/// the bottom of the viewport, never scrolled up into blank space (no over-scroll).
fn max_scroll(total: usize, viewport: usize) -> usize {
    total.saturating_sub(viewport)
}

/// Adjust a stored top-of-window offset so `cursor` stays visible in a window of
/// `viewport` rows over `total` items, scrolling only when the cursor crosses an
/// edge and never past the last page. The cursor sits still within the window
/// until it reaches the top or bottom edge, at which point the window follows it.
fn follow_cursor(prev: usize, cursor: usize, viewport: usize, total: usize) -> usize {
    if viewport == 0 || total == 0 {
        return 0;
    }
    let max_off = total.saturating_sub(viewport);
    let mut off = prev.min(max_off);
    if cursor < off {
        off = cursor;
    } else if cursor >= off + viewport {
        off = cursor + 1 - viewport;
    }
    off.min(max_off)
}

// --- Active/inactive recognition --------------------------------------------
//
// Keyboard focus within the middle area is *controlled* by the reducer, which
// writes the focused panel id into `AppState.focused_panel` (the reducer's
// `intent_focus` rule routes it from the dispatched intent; mode changes and
// focus ops also set it). The middle area only *recognizes* that state at render
// time. So that every middle renderer reads it the same way — instead of
// comparing `focused_panel` strings inline — recognition is funneled through
// `focus()` below. The panel ids themselves are the shared `core::PANEL_*`
// constants, the single source of truth for both the control and the recognition.

/// The active (focused) panel within the middle area, as recognized from the
/// AppState. The middle module's single source of truth for active/inactive.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Focus {
    File,
    Function,
}

impl Focus {
    pub(super) fn is_file(self) -> bool {
        self == Focus::File
    }
    pub(super) fn is_function(self) -> bool {
        self == Focus::Function
    }
}

/// Recognize which middle panel currently holds focus. The file panel is the
/// default holder; only an explicit `function` focus moves it to the right.
pub(super) fn focus(state: &AppState) -> Focus {
    if state.focused_panel == PANEL_FUNCTION {
        Focus::Function
    } else {
        Focus::File
    }
}

/// Gap between adjacent file-list columns.
const LIST_GAP: usize = 2;
/// Minimum cell width for the Grid layout.
const GRID_MIN_CELL: usize = 10;

/// The display text of an entry's cell: `{mark}{name}{dir-slash}`.
fn cell_text(e: &crate::core::Entry, selection: &std::collections::HashSet<std::path::PathBuf>) -> String {
    let mark = if selection.contains(&e.path) { "*" } else { " " };
    let icon = if e.is_dir { "/" } else { " " };
    format!("{mark}{}{icon}", e.name)
}

/// Truncate (by chars) or right-pad `s` to exactly `width` columns.
fn fit(s: &str, width: usize) -> String {
    let count = s.chars().count();
    if count > width {
        s.chars().take(width).collect()
    } else {
        let mut t = s.to_string();
        t.extend(std::iter::repeat_n(' ', width - count));
        t
    }
}

/// Compute the file-list geometry for the active layout: the column count, the
/// visual-row count, and each column's width. The single source of `(cols, rows)`
/// — the kernel feeds it back to the reducer so cursor movement matches the screen
/// (see [`ScrollMemory::list_geom`]). Column-major throughout.
fn list_geometry(
    entries: &[crate::core::Entry],
    layout: crate::core::ListLayout,
    selection: &std::collections::HashSet<std::path::PathBuf>,
    w: usize,
) -> (usize, usize, Vec<usize>) {
    let n = entries.len();
    if n == 0 || w == 0 {
        return (1, 0, vec![w.max(1)]);
    }
    let cell_w = |idx: usize| entries.get(idx).map(|e| cell_text(e, selection).chars().count()).unwrap_or(0);
    match layout {
        crate::core::ListLayout::Rows => (1, n, vec![w]),
        crate::core::ListLayout::Columns => {
            // True `ls -C`: the most columns that fit, each sized to its own
            // entries (column-major), gaps between columns.
            let upper = n.min((w + LIST_GAP) / (1 + LIST_GAP)).max(1);
            for c in (1..=upper).rev() {
                let rows = n.div_ceil(c);
                let mut widths = Vec::with_capacity(c);
                let mut total = 0usize;
                for j in 0..c {
                    let start = j * rows;
                    let end = ((j + 1) * rows).min(n);
                    let cw = (start..end).map(cell_w).max().unwrap_or(0);
                    widths.push(cw);
                    total += cw;
                }
                total += LIST_GAP * c.saturating_sub(1);
                if total <= w {
                    return (c, rows, widths);
                }
            }
            // Nothing fits: a single, truncated column.
            (1, n, vec![w])
        }
        crate::core::ListLayout::Grid => {
            let max_fit = (w / GRID_MIN_CELL).max(1);
            let cols = (n as f64).sqrt().ceil() as usize;
            let cols = cols.clamp(1, max_fit).min(n);
            let rows = n.div_ceil(cols);
            let cell = (w / cols).max(1);
            (cols, rows, vec![cell; cols])
        }
    }
}

/// The base style for entry `idx` (dir/symlink/extension override), with the
/// cursor highlight layered on top when `idx == cursor`. Shared by every
/// file-list renderer (Columns/Grid's per-cell fit and Rows' name + props).
fn entry_cell_style(
    e: &crate::core::Entry,
    idx: usize,
    cursor: usize,
    focused: bool,
    extensions: &ExtensionRegistry,
    theme: &Theme,
) -> Style {
    let mut style = Style::default();
    if let Some(ext_style) = extensions.entry_style(e) {
        style = ext_style;
    } else if e.is_dir {
        style = theme.style(Role::EntryDir);
    } else if e.is_symlink {
        style = theme.style(Role::EntrySymlink);
    }
    if idx == cursor {
        // Cursor colours follow the panel's focus: bright when focused,
        // subdued otherwise (doc/UI.md). Bold from the entry kind above
        // (e.g. a bold directory) is kept; the cursor role can add bold
        // of its own without clearing it.
        let (fg, bg) = if focused {
            (Role::CursorActiveFg, Role::CursorActiveBg)
        } else {
            (Role::CursorFg, Role::CursorBg)
        };
        style = style.fg(theme.role(fg)).bg(theme.role(bg));
        if theme.bold(fg) {
            style = style.add_modifier(Modifier::BOLD);
        }
    }
    style
}

/// What a list row shows about the entries' state: where the cursor is, which
/// entries are marked, and whether the list has focus.
#[derive(Clone, Copy)]
struct Marks<'a> {
    cursor: usize,
    selection: &'a std::collections::HashSet<std::path::PathBuf>,
    focused: bool,
}

/// A column-major grid: `cols` columns of `rows` rows, each `widths[c]` wide.
struct Grid<'a> {
    cols: usize,
    rows: usize,
    widths: &'a [usize],
}

/// Build the visual rows (column-major) as styled lines: cell `idx = c*rows + r`,
/// each fit to its column width, joined by the gap; the cursor cell is highlighted.
fn build_list_lines(
    entries: &[crate::core::Entry],
    window: std::ops::Range<usize>,
    marks: &Marks,
    grid: &Grid,
    extensions: &ExtensionRegistry,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let n = entries.len();
    let (cols, rows, widths) = (grid.cols, grid.rows, grid.widths);
    let mut lines = Vec::with_capacity(window.len());
    for r in window {
        let mut spans: Vec<Span<'static>> = Vec::new();
        for c in 0..cols {
            let idx = c * rows + r;
            if idx >= n {
                continue;
            }
            if !spans.is_empty() {
                spans.push(Span::raw(" ".repeat(LIST_GAP)));
            }
            let e = &entries[idx];
            let cw = widths.get(c).copied().unwrap_or(0);
            let style = entry_cell_style(e, idx, marks.cursor, marks.focused, extensions, theme);
            spans.push(Span::styled(fit(&cell_text(e, marks.selection), cw), style));
        }
        lines.push(Line::from(spans));
    }
    lines
}

// ── Rows layout: right-aligned PERM/OWNR/SIZE/TIME columns ─────────────────────
//
// PANEL_FILE's Rows layout only (not Columns/Grid, not the file-search Select
// grid): past the longest name in the current listing, whatever horizontal
// clearance remains shows as many of PERM/OWNR/SIZE/TIME as fit, always
// dropped in this order as clearance shrinks — OWNR (widest, most variable)
// first, then PERM, then SIZE; TIME survives alone at the narrowest fit
// (most compact, most broadly useful — sorted-by-recency browsing). All
// entries share one set of field widths (the max over the whole listing), so
// the columns line up like `ls -l`, not just the name column.

/// `PERM` is always exactly 10 columns (`wasdf_sdk::format_permissions`);
/// `TIME` the short `MM-DD HH:MM` form (`fmt_time(_, false)`), 11 columns.
const PROP_PERM_W: usize = 10;
const PROP_TIME_W: usize = 11;
/// Minimum gap between the name column and the prop columns, and between
/// adjacent prop fields.
const PROP_GAP: usize = 2;
const PROP_FIELD_GAP: usize = 1;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum PropLevel {
    None,
    Small,  // TIME
    Medium, // SIZE TIME
    Large,  // PERM SIZE TIME
    Max,    // PERM OWNR SIZE TIME
}

/// Per-listing dynamic field widths (PERM/TIME are fixed; OWNR/SIZE depend on
/// the actual values in the current directory, like any other column here).
struct PropWidths {
    ownr: usize,
    size: usize,
}

impl PropWidths {
    fn measure(entries: &[crate::core::Entry]) -> Self {
        let ownr = entries.iter().map(|e| owner_short(e).chars().count()).max().unwrap_or(0);
        let size = entries.iter().map(|e| wasdf_sdk::format_size(e.size).chars().count()).max().unwrap_or(0);
        PropWidths { ownr, size }
    }

    /// Total width of the prop segment at `level` (fields plus the 1-column
    /// gaps between them) — what `pick_level` measures against the clearance.
    fn segment_width(&self, level: PropLevel) -> usize {
        let fields = self.field_widths(level);
        if fields.is_empty() {
            return 0;
        }
        fields.iter().sum::<usize>() + PROP_FIELD_GAP * (fields.len() - 1)
    }

    /// The field widths making up `level`, left to right.
    fn field_widths(&self, level: PropLevel) -> Vec<usize> {
        match level {
            PropLevel::None => vec![],
            PropLevel::Small => vec![PROP_TIME_W],
            PropLevel::Medium => vec![self.size, PROP_TIME_W],
            PropLevel::Large => vec![PROP_PERM_W, self.size, PROP_TIME_W],
            PropLevel::Max => vec![PROP_PERM_W, self.ownr, self.size, PROP_TIME_W],
        }
    }

    /// The widest level whose segment fits in `clearance`.
    fn pick_level(&self, clearance: usize) -> PropLevel {
        for level in [PropLevel::Max, PropLevel::Large, PropLevel::Medium, PropLevel::Small] {
            if self.segment_width(level) <= clearance {
                return level;
            }
        }
        PropLevel::None
    }
}

/// The compact OWNR value for the Rows prop column: the owner's name, or the
/// uid when it has none.
fn owner_short(e: &crate::core::Entry) -> String {
    if e.owner.is_empty() { e.uid.to_string() } else { e.owner.to_string() }
}

/// Right-align (left-pad) `s` to exactly `width` columns; truncate from the
/// left if it somehow overflows (widths are pre-measured against the same
/// listing, so this should never trigger in practice).
fn pad_left(s: &str, width: usize) -> String {
    let count = s.chars().count();
    if count >= width {
        s.chars().skip(count - width).collect()
    } else {
        let mut t = " ".repeat(width - count);
        t.push_str(s);
        t
    }
}

/// One entry's prop values at `level`, paired with the theme role each one
/// carries, left to right — the same field set `PropWidths::field_widths`
/// sizes, so the two never disagree about what's shown. All four fields
/// share `Role::FileRowProps`: one themeable color for the whole column,
/// distinct from the top prop panel's own per-field `PropXxxValue` roles.
fn prop_values(e: &crate::core::Entry, level: PropLevel) -> Vec<(String, Role)> {
    let mut out = Vec::new();
    if matches!(level, PropLevel::Large | PropLevel::Max) {
        out.push((wasdf_sdk::format_permissions(e.mode), Role::FileRowProps));
    }
    if level == PropLevel::Max {
        out.push((owner_short(e), Role::FileRowProps));
    }
    if matches!(level, PropLevel::Medium | PropLevel::Large | PropLevel::Max) {
        out.push((wasdf_sdk::format_size(e.size), Role::FileRowProps));
    }
    if level != PropLevel::None {
        out.push((crate::ui::top::fmt_time(e.modified, false), Role::FileRowProps));
    }
    out
}

// ── The file-size bar: a themed gradient overlay across the prop segment ───────
//
// A logarithmic bar (base: the largest *file* in the current listing —
// directories don't carry a meaningful size, so they never set the scale and
// never draw a bar of their own) drawn by painting the trailing columns of
// the prop segment with a smooth color gradient (`Role::FileRowSizeGraph`:
// fg the gradient's left end, bg its right end), so it reads as a filled bar
// sitting behind the PERM/OWNR/SIZE/TIME text rather than a separate widget.
// Anchored at the segment's right edge: `reversed_from` — not a length from
// the left — is what every row shares, so a file's bar always grows *left*
// from that fixed zero point as its size grows, however many columns the
// segment has. The gradient itself is fixed to *each row's own drawn bar*
// (its own left edge at `reversed_from` through the zero point), not the
// whole prop segment — a short bar still shows the gradient's full range,
// left end to right end, just compressed into fewer columns, rather than
// only a sliver of whichever color the segment's fixed 0% happens to be.
// `@/settings/Display/file-row-size-graph` toggles the whole feature off;
// either gradient endpoint left "nil" (`Color::Reset`) does the same, since
// there is no meaningful color to interpolate to/from.

/// How many of the segment's trailing columns should read as "inside the
/// bar" for a file of `size` bytes, given the listing's largest file is
/// `max_file_size` bytes and the whole segment is `segment_width` columns
/// wide. Logarithmic: doubling a small file's size moves the bar much more
/// than doubling an already-huge one. `0` for directories' non-callers, an
/// empty listing (`max_file_size == 0`), or a zero-width segment.
fn bar_width_log(size: u64, max_file_size: u64, segment_width: usize) -> usize {
    if max_file_size == 0 || segment_width == 0 {
        return 0;
    }
    let frac = ((size as f64) + 1.0).ln() / ((max_file_size as f64) + 1.0).ln();
    (frac.clamp(0.0, 1.0) * segment_width as f64).round() as usize
}

/// Linear-interpolate `left` towards `right` at `t` (clamped to `0.0..=1.0`),
/// per RGB channel.
fn lerp_rgb(left: (u8, u8, u8), right: (u8, u8, u8), t: f64) -> Color {
    let t = t.clamp(0.0, 1.0);
    let ch = |a: u8, b: u8| (a as f64 + (b as f64 - a as f64) * t).round() as u8;
    Color::Rgb(ch(left.0, right.0), ch(left.1, right.1), ch(left.2, right.2))
}

/// The gradient's color at absolute column `col`, within a bar that spans
/// `bar_width` columns starting at `bar_start` (`bar_start` = the bar's own
/// left edge — `left` — through `bar_start + bar_width - 1`, the zero point —
/// `right`). Fixed to *this row's own drawn bar*, not the prop segment as a
/// whole, per the module doc above.
fn gradient_at(col: usize, bar_start: usize, bar_width: usize, left: (u8, u8, u8), right: (u8, u8, u8)) -> Color {
    if bar_width <= 1 {
        return Color::Rgb(right.0, right.1, right.2);
    }
    lerp_rgb(left, right, (col - bar_start) as f64 / (bar_width - 1) as f64)
}

/// Push `text` styled with `style` into `spans`, splitting it at the bar
/// boundary if needed: the portion before `bar_start` (measured from the
/// segment's own start, tracked by `seg_pos`) keeps `style` untouched as one
/// span; the portion at or past it is inside the bar and gets one span per
/// character, each with its own gradient `bg` (`gradient_at`'s per-column
/// color, scaled to this row's own `bar_width`) layered on top of `style` —
/// a single `Span` can only carry one style, so a smooth gradient needs one
/// span per cell. Advances `*seg_pos` by `text`'s length either way.
#[allow(clippy::too_many_arguments)]
fn push_gradient_aware(
    spans: &mut Vec<Span<'static>>,
    seg_pos: &mut usize,
    text: &str,
    style: Style,
    bar_start: usize,
    bar_width: usize,
    left: (u8, u8, u8),
    right: (u8, u8, u8),
) {
    let chars: Vec<char> = text.chars().collect();
    let start = *seg_pos;
    let outside_len = bar_start.saturating_sub(start).min(chars.len());
    if outside_len > 0 {
        let prefix: String = chars[..outside_len].iter().collect();
        spans.push(Span::styled(prefix, style));
    }
    for (i, ch) in chars[outside_len..].iter().enumerate() {
        let col = start + outside_len + i;
        spans.push(Span::styled(ch.to_string(), style.bg(gradient_at(col, bar_start, bar_width, left, right))));
    }
    *seg_pos = start + chars.len();
}

/// Build Rows-layout lines with right-aligned PERM/OWNR/SIZE/TIME columns:
/// the name column is fixed to the longest name in `entries` (so every prop
/// column lines up down the listing), and the prop level is chosen once for
/// the whole listing from the clearance left over. Falls back to a plain
/// name-only line (see `build_list_lines`'s Rows case) when nothing fits.
/// Past the fields themselves, each file's row overlays the size bar
/// described above when `file_row_size_graph` is on and both of
/// `Role::FileRowSizeGraph`'s fg/bg are set; directories never do.
fn build_rows_lines_with_props(
    entries: &[crate::core::Entry],
    window: std::ops::Range<usize>,
    marks: &Marks,
    width: usize,
    extensions: &ExtensionRegistry,
    theme: &Theme,
    file_row_size_graph: bool,
) -> Vec<Line<'static>> {
    let Marks { cursor, selection, focused } = *marks;
    let name_w = entries.iter().map(|e| cell_text(e, selection).chars().count()).max().unwrap_or(0);
    let prop_widths = PropWidths::measure(entries);
    let clearance = width.saturating_sub(name_w + PROP_GAP);
    let level = prop_widths.pick_level(clearance);
    let segment_width = prop_widths.segment_width(level);
    let max_file_size = entries.iter().filter(|e| !e.is_dir).map(|e| e.size).max().unwrap_or(0);

    // Both gradient endpoints must be real colors — nil ("no color") leaves
    // nothing sensible to interpolate, so the bar just doesn't draw.
    let gradient = file_row_size_graph.then(|| {
        let (left_c, right_c) = (theme.role(Role::FileRowSizeGraph), theme.bg(Role::FileRowSizeGraph));
        (left_c != Color::Reset && right_c != Color::Reset)
            .then(|| (crate::core::color_to_rgb(left_c), crate::core::color_to_rgb(right_c)))
    }).flatten();

    entries
        .iter()
        .enumerate()
        .skip(window.start)
        .take(window.len())
        .map(|(idx, e)| {
            let style = entry_cell_style(e, idx, cursor, focused, extensions, theme);
            let name_w_here = if level == PropLevel::None { width } else { name_w };
            let mut spans = vec![Span::styled(fit(&cell_text(e, selection), name_w_here), style)];
            if level != PropLevel::None {
                spans.push(Span::raw(" ".repeat(PROP_GAP)));
                let field_widths = prop_widths.field_widths(level);
                match gradient {
                    Some((left, right)) => {
                        let bar_width = if e.is_dir { 0 } else { bar_width_log(e.size, max_file_size, segment_width) };
                        let reversed_from = segment_width.saturating_sub(bar_width);
                        let mut seg_pos = 0usize;
                        for (i, ((value, role), fw)) in prop_values(e, level).into_iter().zip(field_widths).enumerate() {
                            if i > 0 {
                                push_gradient_aware(&mut spans, &mut seg_pos, &" ".repeat(PROP_FIELD_GAP), Style::default(), reversed_from, bar_width, left, right);
                            }
                            push_gradient_aware(&mut spans, &mut seg_pos, &pad_left(&value, fw), theme.style(role), reversed_from, bar_width, left, right);
                        }
                    }
                    None => {
                        for (i, ((value, role), fw)) in prop_values(e, level).into_iter().zip(field_widths).enumerate() {
                            if i > 0 {
                                spans.push(Span::raw(" ".repeat(PROP_FIELD_GAP)));
                            }
                            spans.push(Span::styled(pad_left(&value, fw), theme.style(role)));
                        }
                    }
                }
            }
            Line::from(spans)
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
pub fn render_file_list(
    frame: &mut Frame,
    area: PanelArea,
    state: &AppState,
    extensions: &ExtensionRegistry,
    scroll: &mut ScrollMemory,
    panels: &mut Vec<PanelFrame>,
    theme: &Theme,
    file_row_size_graph: bool,
) {
    let focused = focus(state).is_file();
    // A committed file-search query swaps `entries` for its matches (see
    // `commit_file_search` in the reducer) — the title says so, since Esc's
    // meaning changes while a filter is active (steps back one level instead
    // of clearing the selection).
    let title = match &state.filter_query {
        Some(q) if q.is_empty() => format!("{} — filtered", state.cwd.display()),
        Some(q) => format!("{} — filter: {q}", state.cwd.display()),
        None => state.cwd.display().to_string(),
    };
    render_entry_grid(
        frame,
        area,
        &state.entries,
        state.cursor,
        state.list_layout,
        &state.selection,
        focused,
        title,
        scroll,
        true,
        panels,
        extensions,
        theme,
        file_row_size_graph,
    );
}

/// Render a navigable entry list as the file-panel grid: the layout geometry,
/// the column-major styled rows, the cursor highlight, and the scrollbar. Shared
/// by the file panel and any path-valued Select (file-search results), so both
/// inherit the same layouts, cursor rendering, and 2-D navigation. `is_file`
/// selects which `ScrollMemory` row offset to persist (the file vs the candidate
/// slot); the geometry it measures is fed back through `list_cols/list_rows` for
/// the reducer's cursor movement.
#[allow(clippy::too_many_arguments)]
fn render_entry_grid(
    frame: &mut Frame,
    area: PanelArea,
    entries: &[crate::core::Entry],
    cursor: usize,
    layout: crate::core::ListLayout,
    selection: &std::collections::HashSet<std::path::PathBuf>,
    focused: bool,
    title: String,
    scroll: &mut ScrollMemory,
    is_file: bool,
    panels: &mut Vec<PanelFrame>,
    extensions: &ExtensionRegistry,
    theme: &Theme,
    file_row_size_graph: bool,
) {
    let inner = area.inner();
    let n = entries.len();
    let h = inner.height as usize;

    let (cols, rows, widths) = list_geometry(entries, layout, selection, inner.width as usize);
    scroll.list_cols = cols;
    scroll.list_rows = rows;

    // PANEL_FILE's Rows layout gets right-aligned PERM/OWNR/SIZE/TIME columns;
    // Columns/Grid and the file-search Select grid (is_file = false) keep the
    // plain name-fit cells.
    // Window over visual rows: the cursor's row drives the persistent offset,
    // and only the rows in the window are built.
    let off = if is_file { &mut scroll.file } else { &mut scroll.candidates };
    if rows > 0 && n > 0 {
        let cursor_row = cursor.min(n - 1) % rows;
        *off = follow_cursor(*off, cursor_row, h, rows);
    } else {
        *off = 0;
    }
    let position = *off;
    let window = position..(position + h).min(rows);
    let marks = Marks { cursor, selection, focused };
    let visible = if layout == crate::core::ListLayout::Rows && is_file {
        build_rows_lines_with_props(entries, window, &marks, inner.width as usize, extensions, theme, file_row_size_graph)
    } else {
        build_list_lines(entries, window, &marks, &Grid { cols, rows, widths: &widths }, extensions, theme)
    };
    frame.render_widget(Paragraph::new(visible), inner);

    // Scrollbar reflects the visible window (offset), in visual-row units.
    panels.push(
        area.frame()
            .title(title)
            .focused(focused)
            .scroll(ScrollInfo { total: rows, viewport: h, position }),
    );
}

pub fn render_select_input(frame: &mut Frame, area: PanelArea, state: &AppState, panels: &mut Vec<PanelFrame>, theme: &Theme) {
    let (query, caret, phase) = match &state.select {
        Some(s) => (s.query.clone(), s.caret.min(s.query.len()), s.phase),
        None => return,
    };
    let inner = area.inner();
    let marker = match phase {
        SelectPhase::Input => "› ",
        SelectPhase::Navigate => "  ",
    };
    // While the form is focused (Input phase), draw a reverse-video block at the
    // real caret. Once Enter leaves the form (Navigate phase), show plain text —
    // no cursor marker — though the caret position itself is kept in state.
    let line = if phase == SelectPhase::Input {
        let caret = if query.is_char_boundary(caret) { caret } else { query.len() };
        let before = query[..caret].to_string();
        let mut rest = query[caret..].chars();
        let under = rest.next().map(|c| c.to_string()).unwrap_or_else(|| " ".into());
        let after: String = rest.collect();
        Line::from(vec![
            Span::styled(marker, theme.style(Role::SelectionMarker)),
            Span::raw(before),
            Span::styled(under, Style::default().add_modifier(Modifier::REVERSED)),
            Span::raw(after),
        ])
    } else {
        Line::from(vec![
            Span::styled(marker, theme.style(Role::SelectionMarker)),
            Span::raw(query.clone()),
        ])
    };
    frame.render_widget(Paragraph::new(line), inner);
    let title = if is_command_select(state) { "Argument" } else { "Search" };
    panels.push(
        area.frame().title(title).focused(phase == SelectPhase::Input),
    );
}

pub fn render_select_candidates(
    frame: &mut Frame,
    area: PanelArea,
    state: &AppState,
    extensions: &ExtensionRegistry,
    scroll: &mut ScrollMemory,
    panels: &mut Vec<PanelFrame>,
    theme: &Theme,
) {
    let Some(sel) = &state.select else { return };
    let navigate = sel.phase == SelectPhase::Navigate;
    // Path-valued results (file-search) render as the file-panel entry grid, so
    // they inherit its layouts, cursor rendering, and 2-D navigation.
    if let Some(view) = &sel.view {
        let title = format!("Results ({}){}", view.len(), if navigate { " [nav]" } else { "" });
        render_entry_grid(
            frame,
            area,
            &view.entries,
            view.cursor,
            state.list_layout,
            &state.selection,
            navigate,
            title,
            scroll,
            false,
            panels,
            extensions,
            theme,
            false, // is_file is false here too, so the size bar never draws regardless
        );
        return;
    }
    let inner = area.inner();
    let items: Vec<ListItem> = sel
        .results
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let cur = i == sel.selected;
            let mark = if sel.marks.contains(&c.label) { "*" } else { " " };
            let mut style = Style::default();
            if cur {
                // The list is focused in the Navigate phase → reverse; else underline.
                style = style.add_modifier(if navigate {
                    Modifier::REVERSED
                } else {
                    Modifier::UNDERLINED
                });
            }
            ListItem::new(Line::from(Span::styled(format!("{mark}{}", c.label), style)))
        })
        .collect();
    let total = sel.results.len();
    let mut ls = ListState::default();
    if total > 0 {
        let cursor = sel.selected.min(total - 1);
        scroll.candidates = follow_cursor(scroll.candidates, cursor, inner.height as usize, total);
        *ls.offset_mut() = scroll.candidates;
        ls.select(Some(cursor));
    } else {
        scroll.candidates = 0;
    }
    frame.render_stateful_widget(List::new(items), inner, &mut ls);
    // A candidate/entry-picking panel needs no "Results (N)" label — it reads
    // as a plain list to browse (like the file panel itself), not a search
    // result count. The command Select keeps its distinct "Options (N)" title
    // (the option checkboxes, a genuinely separate concept from the results).
    let mut pf = area.frame()
        .focused(navigate)
        .scroll(ScrollInfo {
            total,
            viewport: inner.height as usize,
            position: scroll.candidates,
        });
    if is_command_select(state) {
        pf = pf.title(format!("Options ({total})"));
    }
    panels.push(pf);
}

// --- helpers ---------------------------------------------------------------

fn scrolled(lines: Vec<Line<'static>>, scroll: usize) -> Paragraph<'static> {
    let scroll = scroll.min(lines.len().saturating_sub(1)) as u16;
    Paragraph::new(lines).scroll((scroll, 0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::path::PathBuf;

    fn entry(name: &str, size: u64) -> crate::core::Entry {
        crate::core::Entry {
            path: PathBuf::from("/d").join(name),
            name: name.into(),
            is_dir: false,
            is_symlink: false,
            size,
            mode: 0o644,
            uid: 0,
            gid: 0, owner: "".into(), group: "".into(),
            modified: Some(std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000)),
            created: None,
            accessed: None,
            symlink_target: None,
            activation: None,
        }
    }

    fn line_text(l: &Line) -> String {
        l.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn pad_left_right_aligns_and_truncates_from_the_left() {
        assert_eq!(pad_left("42", 5), "   42");
        assert_eq!(pad_left("hello", 5), "hello");
        assert_eq!(pad_left("toolong", 4), "long", "overflow truncates from the left, keeping the tail");
    }

    #[test]
    fn prop_widths_pick_level_drops_ownr_first_then_perm_then_size() {
        let widths = PropWidths { ownr: 8, size: 6 };
        let max_w = widths.segment_width(PropLevel::Max);
        let large_w = widths.segment_width(PropLevel::Large);
        let medium_w = widths.segment_width(PropLevel::Medium);
        let small_w = widths.segment_width(PropLevel::Small);
        assert!(max_w > large_w && large_w > medium_w && medium_w > small_w);

        assert_eq!(widths.pick_level(max_w), PropLevel::Max);
        assert_eq!(widths.pick_level(max_w - 1), PropLevel::Large, "just short of Max drops to Large (OWNR first)");
        assert_eq!(widths.pick_level(large_w), PropLevel::Large);
        assert_eq!(widths.pick_level(large_w - 1), PropLevel::Medium, "next drops PERM");
        assert_eq!(widths.pick_level(medium_w), PropLevel::Medium);
        assert_eq!(widths.pick_level(medium_w - 1), PropLevel::Small, "next drops SIZE, TIME survives alone");
        assert_eq!(widths.pick_level(small_w), PropLevel::Small);
        assert_eq!(widths.pick_level(small_w - 1), PropLevel::None, "not even TIME fits");
    }

    #[test]
    fn prop_values_field_count_matches_field_widths_for_every_level() {
        let e = entry("a.txt", 1024);
        let widths = PropWidths { ownr: 4, size: 5 };
        for level in [PropLevel::None, PropLevel::Small, PropLevel::Medium, PropLevel::Large, PropLevel::Max] {
            assert_eq!(
                prop_values(&e, level).len(),
                widths.field_widths(level).len(),
                "level {level:?}: prop_values and field_widths must agree on field count"
            );
        }
    }

    #[test]
    fn rows_with_ample_width_shows_the_max_level() {
        let entries = vec![entry("short.txt", 10), entry("a-much-longer-filename.txt", 1234)];
        let selection = HashSet::new();
        let extensions = ExtensionRegistry::new();
        let theme = Theme::default();
        let lines = build_rows_lines_with_props(&entries, 0..entries.len(), &Marks { cursor: 0, selection: &selection, focused: true }, 120, &extensions, &theme, true);
        assert_eq!(lines.len(), 2);
        let text = line_text(&lines[0]);
        assert!(text.contains("rw-r--r--"), "PERM shown at ample width: {text:?}");
        assert!(text.trim_end().ends_with(char::is_numeric) || text.contains(':'), "TIME shown: {text:?}");
    }

    #[test]
    fn rows_with_no_clearance_falls_back_to_name_only() {
        let entries = vec![entry("a-very-long-filename-that-eats-the-whole-row.txt", 10)];
        let selection = HashSet::new();
        let extensions = ExtensionRegistry::new();
        let theme = Theme::default();
        // Width barely wider than the name itself — no room for PROP_GAP + TIME.
        let name_w = "a-very-long-filename-that-eats-the-whole-row.txt".len() + 2;
        let lines = build_rows_lines_with_props(&entries, 0..entries.len(), &Marks { cursor: 0, selection: &selection, focused: true }, name_w, &extensions, &theme, true);
        assert_eq!(lines.len(), 1);
        let text = line_text(&lines[0]);
        assert!(!text.contains("rw-r"), "no room for props: {text:?}");
    }

    #[test]
    fn rows_columns_line_up_to_the_longest_name_regardless_of_each_rows_own_length() {
        let entries = vec![entry("a.txt", 10), entry("a-longer-name.txt", 20)];
        let selection = HashSet::new();
        let extensions = ExtensionRegistry::new();
        let theme = Theme::default();
        let lines = build_rows_lines_with_props(&entries, 0..entries.len(), &Marks { cursor: 0, selection: &selection, focused: true }, 120, &extensions, &theme, true);
        // Both rows' prop segments must start at the same column (padded to
        // the longest name), so the TIME field lands in the same place.
        let t0 = line_text(&lines[0]);
        let t1 = line_text(&lines[1]);
        let time_col = |s: &str| s.rfind(':').map(|i| i - 2); // "HH:MM" -> the HH start
        assert_eq!(time_col(&t0), time_col(&t1), "TIME column aligns across rows: {t0:?} vs {t1:?}");
    }

    fn dir_entry(name: &str, size: u64) -> crate::core::Entry {
        let mut e = entry(name, size);
        e.is_dir = true;
        e
    }

    /// Any span the gradient touched — `Theme::default()`'s Prop*Value roles
    /// all default to a nil (`Color::Reset`) bg, so a set, non-reset bg is
    /// unambiguously "inside the bar" in these tests.
    fn has_gradient_bg(s: &Span) -> bool {
        matches!(s.style.bg, Some(c) if c != Color::Reset)
    }

    #[test]
    fn bar_width_log_is_zero_at_zero_scale_and_full_at_the_largest_file() {
        assert_eq!(bar_width_log(500, 0, 20), 0, "an empty/all-zero listing has no scale to bar against");
        assert_eq!(bar_width_log(500, 1000, 0), 0, "a zero-width segment has nowhere to draw");
        assert_eq!(bar_width_log(1000, 1000, 20), 20, "the largest file fills the whole segment");
        assert_eq!(bar_width_log(0, 1000, 20), 0, "a zero-byte file draws no bar (ln(1) == 0)");
    }

    #[test]
    fn bar_width_log_grows_monotonically_with_size_but_sublinearly() {
        let w = |size| bar_width_log(size, 1_000_000, 40);
        assert!(w(10) < w(1_000), "bigger files must never draw a shorter bar");
        assert!(w(1_000) < w(100_000));
        // Logarithmic: the first order-of-magnitude jump (10 -> 1_000) moves the
        // bar by more columns than the same-ratio jump near the top of the scale
        // (100_000 -> 1_000_000), since the bar is capped at `segment_width`.
        assert!(w(1_000) - w(10) >= w(1_000_000) - w(100_000));
    }

    #[test]
    fn gradient_at_interpolates_linearly_across_the_bars_own_span() {
        let left = (0, 0, 0);
        let right = (100, 200, 250);
        // A bar starting at column 0, 6 columns wide (0..=5).
        assert_eq!(gradient_at(0, 0, 6, left, right), Color::Rgb(0, 0, 0), "the bar's own start is exactly the left endpoint");
        assert_eq!(gradient_at(5, 0, 6, left, right), Color::Rgb(100, 200, 250), "the bar's own end (zero point) is exactly the right endpoint");
        assert_eq!(gradient_at(1, 0, 6, left, right), lerp_rgb(left, right, 0.2));
    }

    #[test]
    fn gradient_at_is_scaled_to_the_bar_not_the_whole_segment() {
        // A short, 2-column bar starting at column 4 (e.g. within a much wider
        // prop segment) must still show the full left..right range across its
        // own 2 columns, not just a sliver near whatever the segment's fixed
        // right end happens to be.
        let left = (0, 0, 0);
        let right = (255, 255, 255);
        assert_eq!(gradient_at(4, 4, 2, left, right), Color::Rgb(0, 0, 0), "the bar's own left edge");
        assert_eq!(gradient_at(5, 4, 2, left, right), Color::Rgb(255, 255, 255), "the bar's own right edge (zero point)");
    }

    #[test]
    fn push_gradient_aware_splits_a_span_at_the_bar_boundary_one_span_per_bar_column() {
        let mut spans = Vec::new();
        let mut pos = 0usize;
        let (left, right) = ((0, 0, 0), (255, 255, 255));
        // Bar starts at column 4, is 2 columns wide (covering "e" and "f").
        push_gradient_aware(&mut spans, &mut pos, "abcdef", Style::default(), 4, 2, left, right);
        // "abcd" (outside the bar) stays one span; "ef" (inside) is one span
        // per character since each column has its own interpolated color.
        assert_eq!(spans.len(), 3, "{spans:?}");
        assert_eq!(spans[0].content.as_ref(), "abcd");
        assert!(!has_gradient_bg(&spans[0]));
        assert_eq!(spans[1].content.as_ref(), "e");
        assert_eq!(spans[1].style.bg, Some(Color::Rgb(0, 0, 0)), "the bar's own left edge -> the left endpoint exactly");
        assert_eq!(spans[2].content.as_ref(), "f");
        assert_eq!(spans[2].style.bg, Some(Color::Rgb(255, 255, 255)), "the bar's own right edge -> the right endpoint exactly");
        assert_eq!(pos, 6);
    }

    #[test]
    fn the_largest_files_bar_reaches_the_segments_right_edge_in_the_right_endpoint_color() {
        // One huge file sets the scale to its own max — its bar fills the
        // entire segment, so the rightmost span (TIME) lands exactly on the
        // gradient's right endpoint (Role::FileRowSizeGraph's bg).
        let entries = vec![entry("big.bin", 10_000_000)];
        let selection = HashSet::new();
        let extensions = ExtensionRegistry::new();
        let theme = Theme::default();
        let lines = build_rows_lines_with_props(&entries, 0..entries.len(), &Marks { cursor: entries.len(), selection: &selection, focused: true }, 120, &extensions, &theme, true);
        let last = lines[0].spans.last().expect("prop segment has at least one span");
        let right = crate::core::color_to_rgb(theme.bg(Role::FileRowSizeGraph));
        assert_eq!(last.style.bg, Some(Color::Rgb(right.0, right.1, right.2)));
    }

    #[test]
    fn even_a_short_bar_reaches_the_left_endpoint_color_at_its_own_start() {
        // A small file next to a much larger one draws a short bar — but that
        // bar must still touch the gradient's left endpoint at its own start,
        // not just fade partway there, since the gradient is scaled to each
        // row's own bar, not the whole (much wider) prop segment. Sizes picked
        // so the shorter bar is several columns wide (not the 1-column
        // degenerate case, which by definition can only show one color).
        let entries = vec![entry("smaller.txt", 100), entry("bigger.bin", 1_000_000)];
        let selection = HashSet::new();
        let extensions = ExtensionRegistry::new();
        let theme = Theme::default();
        let lines = build_rows_lines_with_props(&entries, 0..entries.len(), &Marks { cursor: entries.len(), selection: &selection, focused: true }, 120, &extensions, &theme, true);
        let left = crate::core::color_to_rgb(theme.role(Role::FileRowSizeGraph));
        let first_gradient_span = lines[0].spans.iter().find(|s| has_gradient_bg(s)).expect("tiny.txt still draws some bar");
        assert_eq!(first_gradient_span.style.bg, Some(Color::Rgb(left.0, left.1, left.2)));
    }

    #[test]
    fn a_tiny_file_draws_a_much_shorter_bar_than_a_huge_neighbor() {
        let entries = vec![entry("tiny.txt", 1), entry("huge.bin", 100_000_000)];
        let selection = HashSet::new();
        let extensions = ExtensionRegistry::new();
        let theme = Theme::default();
        // No entry under the cursor (out of range): the cursor highlight also
        // sets a non-reset bg on the name span, which would otherwise be
        // indistinguishable from the gradient in these `has_gradient_bg` checks.
        let lines = build_rows_lines_with_props(&entries, 0..entries.len(), &Marks { cursor: entries.len(), selection: &selection, focused: true }, 120, &extensions, &theme, true);
        let bar_chars = |l: &Line| -> usize {
            l.spans.iter().filter(|s| has_gradient_bg(s)).map(|s| s.content.chars().count()).sum()
        };
        let tiny_bar = bar_chars(&lines[0]);
        let huge_bar = bar_chars(&lines[1]);
        assert!(huge_bar > tiny_bar, "huge.bin ({huge_bar} gradient cols) must draw a longer bar than tiny.txt ({tiny_bar})");
    }

    #[test]
    fn directories_never_draw_a_size_bar_even_when_huge() {
        let entries = vec![dir_entry("etc", 10_000_000_000)];
        let selection = HashSet::new();
        let extensions = ExtensionRegistry::new();
        let theme = Theme::default();
        let lines = build_rows_lines_with_props(&entries, 0..entries.len(), &Marks { cursor: entries.len(), selection: &selection, focused: true }, 120, &extensions, &theme, true);
        assert!(!lines[0].spans.iter().any(has_gradient_bg), "directories are excluded from the size-bar scale entirely");
    }

    #[test]
    fn the_toggle_off_disables_the_size_bar_entirely() {
        let entries = vec![entry("huge.bin", 100_000_000)];
        let selection = HashSet::new();
        let extensions = ExtensionRegistry::new();
        let theme = Theme::default();
        let lines = build_rows_lines_with_props(&entries, 0..entries.len(), &Marks { cursor: entries.len(), selection: &selection, focused: true }, 120, &extensions, &theme, false);
        assert!(!lines[0].spans.iter().any(has_gradient_bg), "file-row-size-graph off draws no bar even for a huge file");
    }

    #[test]
    fn a_nil_gradient_endpoint_disables_the_bar_too() {
        let entries = vec![entry("huge.bin", 100_000_000)];
        let selection = HashSet::new();
        let extensions = ExtensionRegistry::new();
        let mut theme = Theme::default();
        theme.set_bg(Role::FileRowSizeGraph, Color::Reset); // right endpoint left nil
        let lines = build_rows_lines_with_props(&entries, 0..entries.len(), &Marks { cursor: entries.len(), selection: &selection, focused: true }, 120, &extensions, &theme, true);
        assert!(!lines[0].spans.iter().any(has_gradient_bg), "a nil gradient endpoint has nothing sensible to interpolate to/from");
    }

    #[test]
    fn only_the_window_is_built_and_keeps_the_listing_wide_column_widths() {
        let entries: Vec<_> = (0..100).map(|i| entry(&format!("file-{i:03}"), i)).collect();
        let selection = HashSet::new();
        let extensions = ExtensionRegistry::new();
        let theme = Theme::default();
        let all = build_rows_lines_with_props(&entries, 0..100, &Marks { cursor: 0, selection: &selection, focused: true }, 120, &extensions, &theme, false);
        let window = build_rows_lines_with_props(&entries, 40..45, &Marks { cursor: 0, selection: &selection, focused: true }, 120, &extensions, &theme, false);
        assert_eq!(window.len(), 5);
        assert_eq!(window, all[40..45], "a windowed row is the same row");
    }
}
