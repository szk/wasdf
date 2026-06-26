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
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, ListState, Paragraph, Wrap};

use crate::core::{AppState, FunctionHint, Mode, PanelContent, SelectInput, SelectPhase, SubLayout};
use crate::extension::{ExtensionRegistry, FunctionDraw, FunctionRenderCtx};
use crate::ui::chrome::{self, PanelFrame, ScrollInfo};
use crate::ui::content;
use crate::ui::render::{ratio_left, split_h, split_v};

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

/// Render the file layout's main area: the file list, plus the function panel at
/// ratio-right while it is visible.
pub fn render_file_main(
    frame: &mut Frame,
    area: Rect,
    state: &AppState,
    extensions: &ExtensionRegistry,
    scroll: &mut ScrollMemory,
    panels: &mut Vec<PanelFrame>,
) {
    if state.function.visible {
        let (lw, rw) = state.function.ratio.weights();
        let (file, func) = split_h(area, ratio_left(area.width, lw, rw));
        render_file_list(frame, file, state, scroll, panels);
        render_function(frame, func, state, extensions, None, scroll, panels);
    } else {
        render_file_list(frame, area, state, scroll, panels);
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

/// Build the visual rows (column-major) as styled lines: cell `idx = c*rows + r`,
/// each fit to its column width, joined by the gap; the cursor cell is highlighted.
fn build_list_lines(
    entries: &[crate::core::Entry],
    cursor: usize,
    selection: &std::collections::HashSet<std::path::PathBuf>,
    focused: bool,
    cols: usize,
    rows: usize,
    widths: &[usize],
) -> Vec<Line<'static>> {
    let n = entries.len();
    let mut lines = Vec::with_capacity(rows);
    for r in 0..rows {
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
            let mut style = Style::default();
            if e.is_dir {
                style = style.fg(Color::Blue).add_modifier(Modifier::BOLD);
            } else if e.is_symlink {
                style = style.fg(Color::Magenta);
            }
            if idx == cursor {
                // Focused cursor → reverse video; unfocused → underline.
                style = style.add_modifier(if focused {
                    Modifier::REVERSED
                } else {
                    Modifier::UNDERLINED
                });
            }
            spans.push(Span::styled(fit(&cell_text(e, selection), cw), style));
        }
        lines.push(Line::from(spans));
    }
    lines
}

fn render_file_list(
    frame: &mut Frame,
    area: Rect,
    state: &AppState,
    scroll: &mut ScrollMemory,
    panels: &mut Vec<PanelFrame>,
) {
    let focused = state.focused_panel == "file";
    render_entry_grid(
        frame,
        area,
        &state.entries,
        state.cursor,
        state.list_layout,
        &state.selection,
        focused,
        state.cwd.display().to_string(),
        scroll,
        true,
        panels,
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
    area: Rect,
    entries: &[crate::core::Entry],
    cursor: usize,
    layout: crate::core::ListLayout,
    selection: &std::collections::HashSet<std::path::PathBuf>,
    focused: bool,
    title: String,
    scroll: &mut ScrollMemory,
    is_file: bool,
    panels: &mut Vec<PanelFrame>,
) {
    let inner = chrome::inner_open_bottom(area);
    let n = entries.len();
    let h = inner.height as usize;

    let (cols, rows, widths) = list_geometry(entries, layout, selection, inner.width as usize);
    scroll.list_cols = cols;
    scroll.list_rows = rows;

    let lines = build_list_lines(entries, cursor, selection, focused, cols, rows, &widths);

    // Window over visual rows: the cursor's row drives the persistent offset.
    let off = if is_file { &mut scroll.file } else { &mut scroll.candidates };
    if rows > 0 && n > 0 {
        let cursor_row = cursor.min(n - 1) % rows;
        *off = follow_cursor(*off, cursor_row, h, rows);
    } else {
        *off = 0;
    }
    let position = *off;
    let visible: Vec<Line> = lines.into_iter().skip(position).take(h).collect();
    frame.render_widget(Paragraph::new(visible), inner);

    // Scrollbar reflects the visible window (offset), in visual-row units.
    panels.push(
        PanelFrame::new(area)
            .title(title)
            .focused(focused)
            .open_bottom(true)
            .open_left(true)
            .scroll(ScrollInfo { total: rows, viewport: h, position }),
    );
}

/// Render the select layout's main area: the select input (collapsed when the
/// spec has no input) above the candidate list, plus the function panel at
/// ratio-right while the spec sets a function hint.
pub fn render_select_main(
    frame: &mut Frame,
    area: Rect,
    state: &AppState,
    extensions: &ExtensionRegistry,
    scroll: &mut ScrollMemory,
    panels: &mut Vec<PanelFrame>,
) {
    let Mode::Select(spec) = state.mode() else { return };
    let (left, right) = if spec.function_hint.is_some() {
        let (lw, rw) = state.function.ratio.weights();
        let (l, r) = split_h(area, ratio_left(area.width, lw, rw));
        (l, Some(r))
    } else {
        (area, None)
    };

    if !matches!(spec.input, SelectInput::None) {
        let (input, cands) = split_v(left, 3);
        render_select_input(frame, input, state, panels);
        render_select_candidates(frame, cands, state, scroll, panels);
    } else {
        render_select_candidates(frame, left, state, scroll, panels);
    }
    if let Some(r) = right {
        render_function(frame, r, state, extensions, spec.function_hint.clone(), scroll, panels);
    }
}

fn render_select_input(frame: &mut Frame, area: Rect, state: &AppState, panels: &mut Vec<PanelFrame>) {
    let (query, caret, phase) = match &state.select {
        Some(s) => (s.query.clone(), s.caret.min(s.query.len()), s.phase),
        None => return,
    };
    let inner = chrome::inner(area);
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
            Span::styled(marker, Style::default().fg(Color::Green)),
            Span::raw(before),
            Span::styled(under, Style::default().add_modifier(Modifier::REVERSED)),
            Span::raw(after),
        ])
    } else {
        Line::from(vec![
            Span::styled(marker, Style::default().fg(Color::Green)),
            Span::raw(query.clone()),
        ])
    };
    frame.render_widget(Paragraph::new(line), inner);
    let title = if is_command_select(state) { "Argument" } else { "Search" };
    panels.push(
        PanelFrame::new(area).title(title).focused(phase == SelectPhase::Input).open_left(true),
    );
}

fn render_select_candidates(
    frame: &mut Frame,
    area: Rect,
    state: &AppState,
    scroll: &mut ScrollMemory,
    panels: &mut Vec<PanelFrame>,
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
        );
        return;
    }
    let inner = chrome::inner_open_bottom(area);
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
    let title = if is_command_select(state) {
        format!("Options ({total})")
    } else {
        format!("Results ({}){}", total, if navigate { " [nav]" } else { "" })
    };
    panels.push(
        PanelFrame::new(area)
            .title(title)
            .focused(navigate)
            .open_bottom(true)
            .open_left(true)
            .scroll(ScrollInfo {
                total,
                viewport: inner.height as usize,
                position: scroll.candidates,
            }),
    );
}

fn render_function(
    frame: &mut Frame,
    area: Rect,
    state: &AppState,
    extensions: &ExtensionRegistry,
    hint: Option<FunctionHint>,
    mem: &mut ScrollMemory,
    panels: &mut Vec<PanelFrame>,
) {
    let inner = chrome::inner_open_bottom(area);
    let focused = state.focused_panel == "function";
    let scroll = state.function.scroll;

    // Exec frame. The vertical scroll is shared with the Content frame and clamped
    // to the last row at the viewport bottom (no over-scroll); push_function records
    // the bound so the kernel discards over-scroll input after the draw.
    if state.function.sublayout == SubLayout::Exec {
        let mut lines: Vec<Line> =
            state.function.exec.lines.iter().map(|l| Line::from(l.clone())).collect();
        if !state.function.exec.finished {
            lines.push(Line::from(Span::styled("… running", Style::default().fg(Color::Yellow))));
        } else if let Some(code) = state.function.exec.exit {
            lines.push(Line::from(Span::styled(
                format!("[exit {code}]"),
                Style::default().fg(if code == 0 { Color::Green } else { Color::Red }),
            )));
        }
        let total = lines.len();
        let scroll = scroll.min(max_scroll(total, inner.height as usize));
        // The exec frame does not h-scroll, so its horizontal bound is zero.
        mem.function_hmax = 0;
        frame.render_widget(scrolled(lines, scroll), inner);
        push_function(panels, area, "exec", focused, total, inner.height, scroll, mem);
        return;
    }

    // Content frame: dispatch by hint (Select) or by the content provider (File).
    match hint {
        Some(FunctionHint::CommandSummary) => render_command_summary(frame, area, inner, state, focused, panels),
        Some(FunctionHint::DirListing) => render_dir_listing(frame, area, inner, state, extensions, focused, mem, panels),
        Some(FunctionHint::Extension(_)) | None => {
            let ctx = FunctionRenderCtx { width: inner.width, height: inner.height, focused, func: &state.function };
            if let Some(content) = &state.function.content {
                // Pushed content (ShowFunctionContent) takes precedence over the
                // provider's own pull render.
                render_panel_content(frame, area, inner, state, content, focused, mem, panels);
            } else if let Some(draw) = extensions.provider().and_then(|p| p.render_function(&ctx)) {
                // The owning extension renders its content; core only blits it.
                draw_function(frame, area, inner, draw, focused, mem, panels);
            } else {
                let msg = if extensions.has_function_content() {
                    "(loading…)"
                } else {
                    "(no content)"
                };
                frame.render_widget(
                    Paragraph::new(Span::styled(msg, Style::default().fg(Color::DarkGray))),
                    inner,
                );
                panels.push(PanelFrame::new(area).title("content").focused(focused).open_bottom(true));
            }
        }
    }
}

fn push_function(
    panels: &mut Vec<PanelFrame>,
    area: Rect,
    title: &str,
    focused: bool,
    total: usize,
    viewport: u16,
    position: usize,
    mem: &mut ScrollMemory,
) {
    // Record the scroll bound for the kernel's post-draw clamp (kernel-owned
    // scroll; the reducer can't see the viewport).
    mem.function_max = max_scroll(total, viewport as usize);
    let mut f = PanelFrame::new(area).title(title).focused(focused).open_bottom(true);
    if total > viewport as usize {
        f = f.scroll(ScrollInfo { total, viewport: viewport as usize, position });
    }
    panels.push(f);
}

/// Blit a [`FunctionDraw`] an extension produced into the function panel: scroll
/// `lines` vertically, draw the optional bottom-row prompt, and push the panel
/// frame (title + scrollbar). This is core's only content-drawing code; it has
/// no notion of search / scroll / highlight — those are baked into what the
/// extension returned.
fn draw_function(
    frame: &mut Frame,
    area: Rect,
    inner: Rect,
    draw: FunctionDraw,
    focused: bool,
    mem: &mut ScrollMemory,
    panels: &mut Vec<PanelFrame>,
) {
    let has_prompt = draw.prompt.is_some();
    let content_h = inner.height.saturating_sub(if has_prompt { 1 } else { 0 });
    let content = Rect { height: content_h, ..inner };
    // Clamp so the last row stops at the viewport bottom — no over-scroll into blank.
    let scroll = draw.scroll.min(max_scroll(draw.total, content_h as usize));
    // Record the horizontal bound (the extension already h-scrolled its lines and
    // reported the content width); the kernel clamps the stored hscroll to it.
    mem.function_hmax = max_scroll(draw.width, inner.width as usize);
    frame.render_widget(Paragraph::new(draw.lines).scroll((scroll as u16, 0)), content);
    if let Some(line) = draw.prompt {
        let row = Rect { x: inner.x, y: inner.y + content_h, width: inner.width, height: 1 };
        frame.render_widget(Paragraph::new(line), row);
    }
    push_function(panels, area, &draw.title, focused, draw.total, content_h, scroll, mem);
}

/// Render generic extension `PanelContent` (the Phase-B push path, for extensions
/// without a render hook yet — e.g. dynamic ones). Builds a `FunctionDraw` with
/// the shared line builder and blits it.
fn render_panel_content(
    frame: &mut Frame,
    area: Rect,
    inner: Rect,
    state: &AppState,
    content: &PanelContent,
    focused: bool,
    mem: &mut ScrollMemory,
    panels: &mut Vec<PanelFrame>,
) {
    let f = &state.function;
    let title = f.content_owner.clone().unwrap_or_else(|| "content".into());
    let draw = match content {
        PanelContent::Lines { lines, styles } => {
            // Clamp h-scroll to the content width (no over-scroll into blank).
            let width = content::content_width(lines);
            let hscroll = f.hscroll.min(width.saturating_sub(inner.width as usize));
            let out = content::text_lines(lines, styles, &f.search, hscroll, f.show_line_numbers, false);
            let total = out.len();
            FunctionDraw {
                lines: out,
                title,
                scroll: f.scroll,
                total,
                width,
                prompt: content::prompt_line(&f.search),
            }
        }
        PanelContent::Image { width, height, rgb } => {
            let img = content::image_cells(*width, *height, rgb, inner.width, inner.height);
            let total = img.len();
            FunctionDraw { lines: img, title, scroll: 0, total, width: 0, prompt: None }
        }
    };
    draw_function(frame, area, inner, draw, focused, mem, panels);
}

fn render_dir_listing(
    frame: &mut Frame,
    area: Rect,
    inner: Rect,
    state: &AppState,
    extensions: &ExtensionRegistry,
    focused: bool,
    mem: &mut ScrollMemory,
    panels: &mut Vec<PanelFrame>,
) {
    let ctx = FunctionRenderCtx { width: inner.width, height: inner.height, focused, func: &state.function };
    if let Some(draw) = extensions.provider().and_then(|p| p.render_function(&ctx)) {
        draw_function(frame, area, inner, draw, focused, mem, panels);
        return;
    }
    let sel = state.select.as_ref().and_then(|s| s.results.get(s.selected));
    let lines = match sel {
        Some(c) => vec![Line::from(c.label.clone())],
        None => vec![Line::from("(no selection)")],
    };
    frame.render_widget(Paragraph::new(lines), inner);
    panels.push(PanelFrame::new(area).title("content").focused(focused).open_bottom(true));
}

/// Whether the active Select is the command flow (Argument input + Options list).
fn is_command_select(state: &AppState) -> bool {
    matches!(state.mode(), Mode::Select(s) if s.id == "command")
}

fn render_command_summary(frame: &mut Frame, area: Rect, inner: Rect, state: &AppState, focused: bool, panels: &mut Vec<PanelFrame>) {
    let mut lines: Vec<Line> = Vec::new();
    if let Mode::Select(spec) = state.mode() {
        if let crate::core::OnConfirm::Resolve { template, fill } = &spec.on_confirm {
            let sel = state.select.as_ref();
            let input = sel.map(|s| s.query.clone()).unwrap_or_default();
            // The chosen options are the marked candidates (toggled with Space).
            let opts: Vec<String> = sel
                .map(|s| {
                    s.results
                        .iter()
                        .filter(|c| s.marks.contains(&c.label))
                        .filter_map(|c| c.value.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();
            // The typed name placed in the current directory (the live target).
            let target = if input.is_empty() {
                match fill {
                    crate::core::ResolveFill::Dst => "<dest>".to_string(),
                    crate::core::ResolveFill::Path => "<name>".to_string(),
                }
            } else if std::path::Path::new(&input).is_absolute() {
                input.clone()
            } else {
                state.cwd.join(&input).display().to_string()
            };
            // Assemble the live command line. With a resolved skeleton, walk it so
            // the panel shows the actual external command (`mv …`, `cp -R …`),
            // filling placeholders with the live opts / paths / target. Without one
            // (extension ctx / tests), fall back to the op key plus the same args.
            let disp = |p: &std::path::PathBuf| p.display().to_string();
            let mut parts: Vec<String> = Vec::new();
            if spec.command_line.is_empty() {
                parts.push(template.op.clone());
                parts.extend(opts.clone());
                parts.extend(template.src.iter().map(disp));
                parts.extend(template.paths.iter().map(disp));
                parts.push(target);
            } else {
                use crate::core::CmdToken;
                for tok in &spec.command_line {
                    match tok {
                        CmdToken::Lit(s) => parts.push(s.clone()),
                        CmdToken::Opts => parts.extend(opts.clone()),
                        CmdToken::Src => parts.extend(template.src.iter().map(disp)),
                        CmdToken::Paths => parts.extend(template.paths.iter().map(disp)),
                        CmdToken::Dst | CmdToken::Path => parts.push(target.clone()),
                    }
                }
            }

            // The command line itself (no in-panel "command" label — the panel
            // title says it).
            lines.push(Line::from(format!("$ {}", parts.join(" "))));
            lines.push(Line::from(""));
            let has_options = sel.map(|s| !s.results.is_empty()).unwrap_or(false);
            let hint = if has_options {
                "Space toggles an option · Enter runs"
            } else {
                "Enter runs"
            };
            lines.push(Line::from(Span::styled(hint, Style::default().fg(Color::DarkGray))));
        }
    }
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
    panels.push(PanelFrame::new(area).title("Command").focused(focused).open_bottom(true));
}

// --- helpers ---------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::{follow_cursor, list_geometry};
    use crate::core::{AppState, Entry, ListLayout};

    fn state_with(n: usize, layout: ListLayout, name_len: usize) -> AppState {
        let mut s = AppState::new(std::path::PathBuf::from("/d"));
        s.list_layout = layout;
        s.entries = (0..n)
            .map(|i| Entry {
                path: std::path::PathBuf::from(format!("/d/{i}")),
                name: "x".repeat(name_len),
                is_dir: false,
                is_symlink: false,
                size: 0,
                mode: 0,
                uid: 0,
                gid: 0,
                modified: None,
                created: None,
                accessed: None,
                symlink_target: None,
            })
            .collect();
        s
    }

    #[test]
    fn rows_geometry_is_one_full_width_column() {
        let s = state_with(10, ListLayout::Rows, 4);
        let (cols, rows, widths) = list_geometry(&s.entries, s.list_layout, &s.selection,80);
        assert_eq!((cols, rows), (1, 10));
        assert_eq!(widths, vec![80]);
    }

    #[test]
    fn columns_is_ls_column_major_and_fits_width() {
        // 10 names, each cell width = 1(mark)+4(name)+1(icon) = 6.
        let s = state_with(10, ListLayout::Columns, 4);
        let (cols, rows, widths) = list_geometry(&s.entries, s.list_layout, &s.selection,40);
        assert!(cols > 1, "should pack multiple columns into width 40, got {cols}");
        assert_eq!(rows, 10_usize.div_ceil(cols), "column-major rows");
        let total: usize = widths.iter().sum::<usize>() + 2 * (cols - 1);
        assert!(total <= 40, "packed columns fit the width: {total} <= 40");
    }

    #[test]
    fn columns_widths_are_per_column() {
        // Mixed name lengths: per-column widths should differ (true ls sizing),
        // not a single global width.
        let mut s = state_with(0, ListLayout::Columns, 0);
        let lens = [2usize, 2, 20, 2]; // col-major with rows=2,cols=2: col0={2,2}, col1={20,2}
        s.entries = lens
            .iter()
            .enumerate()
            .map(|(i, &l)| Entry {
                path: std::path::PathBuf::from(format!("/d/{i}")),
                name: "x".repeat(l),
                is_dir: false,
                is_symlink: false,
                size: 0,
                mode: 0,
                uid: 0,
                gid: 0,
                modified: None,
                created: None,
                accessed: None,
                symlink_target: None,
            })
            .collect();
        let (cols, _rows, widths) = list_geometry(&s.entries, s.list_layout, &s.selection,40);
        // True `ls` sizing: a column with a long name is wider than the others,
        // so the per-column widths are not all equal (unlike Grid's uniform cells).
        if cols >= 2 {
            let max = widths.iter().max().unwrap();
            let min = widths.iter().min().unwrap();
            assert_ne!(max, min, "columns are sized to their own entries: {widths:?}");
        }
    }

    #[test]
    fn grid_is_square_ish_and_fits() {
        let s = state_with(16, ListLayout::Grid, 4);
        let (cols, rows, widths) = list_geometry(&s.entries, s.list_layout, &s.selection,80);
        assert!(cols >= 1 && cols <= 80 / 10, "grid cols clamped to fit");
        assert!(cols * rows >= 16, "grid covers all entries");
        assert!(widths.iter().all(|&w| w == widths[0]), "grid cells are uniform");
    }

    #[test]
    fn tiny_width_degrades_to_single_column() {
        let s = state_with(10, ListLayout::Columns, 30);
        let (cols, _rows, _w) = list_geometry(&s.entries, s.list_layout, &s.selection,8);
        assert_eq!(cols, 1, "no room for multiple columns");
    }

    // doc/UI.md: the window stays put while the cursor moves within it, and
    // scrolls only when the cursor crosses the top or bottom edge.
    #[test]
    fn window_holds_until_the_cursor_leaves_it() {
        // viewport 5 over 100 items, window currently at the top.
        // Moving the cursor down inside [0, 4] never scrolls.
        for cursor in 0..5 {
            assert_eq!(follow_cursor(0, cursor, 5, 100), 0, "cursor {cursor} stays in window");
        }
        // Crossing the bottom edge scrolls by exactly one row, cursor at bottom.
        assert_eq!(follow_cursor(0, 5, 5, 100), 1);
        // With the window at offset 10, moving up inside [10, 14] never scrolls,
        assert_eq!(follow_cursor(10, 12, 5, 100), 10);
        // and crossing the top edge scrolls up so the cursor sits at the top.
        assert_eq!(follow_cursor(10, 9, 5, 100), 9);
    }

    // doc/UI.md: never scroll past the last page — a down step at the bottom is
    // a no-op for the window (the cursor is already clamped by the reducer).
    #[test]
    fn offset_never_passes_the_last_page() {
        // max offset = total - viewport = 5. Already there, cursor at the last
        // item: the window does not move.
        assert_eq!(follow_cursor(5, 9, 5, 10), 5);
        // A stale offset beyond the last page is pulled back to it.
        assert_eq!(follow_cursor(999, 9, 5, 10), 5);
    }

    #[test]
    fn content_that_fits_never_scrolls() {
        assert_eq!(follow_cursor(0, 4, 10, 5), 0);
        assert_eq!(follow_cursor(3, 4, 10, 5), 0, "viewport >= total pins to top");
    }

    #[test]
    fn empty_or_zero_viewport_is_offset_zero() {
        assert_eq!(follow_cursor(7, 0, 0, 10), 0);
        assert_eq!(follow_cursor(7, 0, 5, 0), 0);
    }
}

fn scrolled(lines: Vec<Line<'static>>, scroll: usize) -> Paragraph<'static> {
    let scroll = scroll.min(lines.len().saturating_sub(1)) as u16;
    Paragraph::new(lines).scroll((scroll, 0))
}
