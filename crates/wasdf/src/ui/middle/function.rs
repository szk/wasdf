//! Function-panel rendering: the right-hand renderer container. Dispatches the
//! Command (CmdOp) / Exec / Content frames, blits extension-produced
//! `FunctionDraw`s and lays out a pushed logical `PanelView`, and draws the
//! Select command summary. Reads the read-only AppState and the render-only
//! `ScrollMemory` (whose function-scroll bounds it records for the kernel's
//! post-draw clamp).

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};

use crate::core::{AppState, Block, FunctionHint, Mode, PanelView, Role, SpanStyle, SubLayout, TextSpan, Theme, LINE_NUMBER_WIDTH};
use crate::extension::{ExtensionRegistry, FunctionDraw, FunctionRenderCtx};
use std::sync::Arc;

use crate::ui::chrome::{PanelArea, PanelFrame, ScrollInfo};
use crate::ui::content;
use crate::ui::graphics::{self, Graphic, GraphicsConfig, Placement};

use super::{max_scroll, scrolled, ScrollMemory};

pub fn render_function(
    frame: &mut Frame,
    area: PanelArea,
    state: &AppState,
    extensions: &ExtensionRegistry,
    mem: &mut ScrollMemory,
    panels: &mut Vec<PanelFrame>,
    theme: &Theme,
) {
    // In a Select the picker says what the panel shows; in File mode the
    // content provider decides.
    let hint = match state.mode() {
        Mode::Select(spec) => spec.function_hint.clone(),
        _ => None,
    };
    let inner = area.inner();
    let focused = super::focus(state).is_function();
    let scroll = state.function.scroll.offset();

    // A form extension hosting the panel (form_owner) draws the whole inner rect
    // via its render_form hook; core wraps the border/title chrome. The generic
    // seam that lets an extension own the panel without core knowing what it is.
    if let Some(owner) = &state.function.form_owner
        && let Some(ext) = extensions.find(owner) {
            let sources = state.targets();
            let ctx = FunctionRenderCtx { width: inner.width, height: inner.height, focused, func: &state.function, sources: &sources, theme };
            if let Some(title) = ext.render_form(frame, inner, &ctx) {
                match ext.form_scroll(&ctx) {
                    Some((total, position)) => {
                        push_function(panels, area, &title, focused, ScrollInfo { total, viewport: inner.height as usize, position }, mem)
                    }
                    None => push_unscrolled(panels, area, &title, focused, mem),
                }
            }
            return;
        }

    // Exec frame. The vertical scroll is shared with the Content frame and clamped
    // to the last row at the viewport bottom (no over-scroll); push_function records
    // the bound so the kernel discards over-scroll input after the draw.
    if state.function.sublayout == SubLayout::Exec {
        let mut lines: Vec<Line> =
            state.function.exec.lines.iter().map(|l| Line::from(l.clone())).collect();
        if !state.function.exec.finished {
            lines.push(Line::from(Span::styled("… running", theme.style(Role::ExecRunning))));
        } else if let Some(code) = state.function.exec.exit {
            lines.push(Line::from(Span::styled(
                format!("[exit {code}]"),
                theme.style(if code == 0 { Role::ExecOk } else { Role::ExecFail }),
            )));
        }
        let total = lines.len();
        let scroll = scroll.min(max_scroll(total, inner.height as usize));
        // The exec frame does not h-scroll, so its horizontal bound is zero.
        mem.function_hmax = 0;
        frame.render_widget(scrolled(lines, scroll), inner);
        push_function(panels, area, "exec", focused, ScrollInfo { total, viewport: inner.height as usize, position: scroll }, mem);
        return;
    }

    // Content frame: dispatch by hint (Select) or by the content provider (File).
    match hint {
        Some(FunctionHint::CommandSummary) => render_command_summary(frame, area, inner, state, focused, panels, mem, theme),
        Some(FunctionHint::DirListing) => render_dir_listing(frame, area, state, extensions, mem, panels, theme),
        Some(FunctionHint::Extension(_)) | None => {
            let sources = state.targets();
            let ctx = FunctionRenderCtx { width: inner.width, height: inner.height, focused, func: &state.function, sources: &sources, theme };
            // A pushed view (ShowFunctionContent) takes precedence over the
            // provider's own pull view. Either way core lays it out.
            let view = state.function.content.clone().or_else(|| extensions.provider().and_then(|p| p.view(&ctx)));
            if let Some(view) = view {
                let title = content::title(content_target(state).as_deref(), &state.function.search);
                let draw = layout_view(&view, state, inner, title, theme, mem);
                draw_function(frame, area, inner, draw, focused, mem, panels);
                blank_placed(frame, mem, theme);
            } else {
                // No content to draw. Distinguish "nothing under the cursor"
                // (an empty directory — there is nothing to preview) from a
                // genuine in-flight load, so the panel never sits on a false
                // "(loading…)" forever (doc/UI.md: empty-directory rule).
                let msg = if state.current_entry().is_none() {
                    "(empty)"
                } else if extensions.has_function_content() {
                    "(loading…)"
                } else {
                    // A cursor entry exists, but the cursor-follow preview has
                    // nothing to show for it (no content provider registered,
                    // or it declined this file) — distinct from "(empty)"
                    // (nothing under the cursor at all).
                    "N/A"
                };
                frame.render_widget(
                    Paragraph::new(Span::styled(msg, theme.style(Role::Muted))),
                    inner,
                );
                push_unscrolled(panels, area, "content", focused, mem);
            }
        }
    }
}

fn push_function(
    panels: &mut Vec<PanelFrame>,
    area: PanelArea,
    title: &str,
    focused: bool,
    scroll: ScrollInfo,
    mem: &mut ScrollMemory,
) {
    // Record the scroll bound for the kernel's post-draw clamp (kernel-owned
    // scroll; the reducer can't see the viewport).
    mem.function_max = max_scroll(scroll.total, scroll.viewport);
    mem.set_function_title(title);
    let mut f = area.frame().title(title).focused(focused);
    if scroll.total > scroll.viewport {
        f = f.scroll(scroll);
    }
    panels.push(f);
}

/// Push a panel frame that has no scrollbar.
fn push_unscrolled(panels: &mut Vec<PanelFrame>, area: PanelArea, title: &str, focused: bool, mem: &mut ScrollMemory) {
    mem.set_function_title(title);
    panels.push(area.frame().title(title).focused(focused));
}

/// The columns an image block takes: clear of the line-number gutter, so the
/// picture lines up with the prose around it. Returns `(gutter, cols)`.
fn image_cols(inner: Rect, line_numbers: bool) -> (u16, u16) {
    let gutter = if line_numbers { LINE_NUMBER_WIDTH } else { 0 };
    (gutter, inner.width.saturating_sub(gutter))
}

/// Blit a laid-out [`FunctionDraw`] into the function panel: its visible
/// `lines`, draw the optional bottom-row prompt, and push the panel
/// frame (title + scrollbar). This is core's only content-drawing code; it has
/// no notion of search / scroll / highlight — those are baked into what the
/// extension returned.
fn draw_function(
    frame: &mut Frame,
    area: PanelArea,
    inner: Rect,
    draw: FunctionDraw,
    focused: bool,
    mem: &mut ScrollMemory,
    panels: &mut Vec<PanelFrame>,
) {
    let has_prompt = draw.prompt.is_some();
    let content_h = inner.height.saturating_sub(if has_prompt { 1 } else { 0 });
    let content = Rect { height: content_h, ..inner };
    // `lines` is already the visible window starting at row `draw.scroll`.
    let scroll = draw.scroll;
    // Record the horizontal bound (the lines are already h-scrolled; `width`
    // is the content width); the kernel clamps the stored hscroll to it.
    mem.function_hmax = max_scroll(draw.width, inner.width as usize);
    frame.render_widget(Paragraph::new(draw.lines), content);
    if let Some(line) = draw.prompt {
        let row = Rect { x: inner.x, y: inner.y + content_h, width: inner.width, height: 1 };
        frame.render_widget(Paragraph::new(line), row);
    }
    push_function(panels, area, &draw.title, focused, ScrollInfo { total: draw.total, viewport: content_h as usize, position: scroll }, mem);
}

/// The width-dependent part of laying out a view — its blocks lowered to
/// rows and its content width — kept until the view, the panel size, line
/// numbers, or the theme change (doc/ARCHITECTURE.md, Render Contract).
pub(super) struct Lowered {
    view: Arc<PanelView>,
    size: (u16, u16),
    line_numbers: bool,
    generation: u64,
    /// The graphics config the images were encoded with.
    graphics: GraphicsConfig,
    /// The encoded images, by where they sit in `rows`.
    images: Vec<LoweredImage>,
    rows: Vec<Row>,
    /// Per row, the block it shows and the bytes before that block's text on
    /// the row (a field's label column) — how logical search matches are put
    /// on rows. `None` for rows without text.
    origins: Vec<Option<(usize, usize)>>,
    /// Per block, the row it starts on.
    first_row: Vec<usize>,
    width: usize,
}

/// An image lowered with a graphics protocol: its first row, its left column
/// offset in the panel, the rows its block spans, and the encoded picture.
pub(super) struct LoweredImage {
    row: usize,
    col: u16,
    span: usize,
    graphic: Arc<Graphic>,
}

fn lower(view: &Arc<PanelView>, inner: Rect, line_numbers: bool, generation: u64, graphics: GraphicsConfig, theme: &Theme) -> Lowered {
    let mut images = Vec::new();
    let (rows, origins, first_row, width) = match &**view {
        PanelView::Document(blocks) => {
            let label_w = blocks
                .iter()
                .filter_map(|b| match b {
                    Block::Field { label, .. } => Some(label.chars().count()),
                    _ => None,
                })
                .max()
                .unwrap_or(0);
            let (mut rows, mut origins, mut first_row) = (Vec::new(), Vec::new(), Vec::with_capacity(blocks.len()));
            for (i, b) in blocks.iter().enumerate() {
                first_row.push(rows.len());
                let text_len = b.text().map(|t| t.len());
                // The chosen protocol cannot draw here: say so on the image's
                // first row; its symbol cells follow.
                if let (Block::Image { .. }, Some(msg)) = (b, graphics.unavailable()) {
                    rows.push(unavailable_row(msg, theme));
                    origins.push(None);
                }
                let start = rows.len();
                if let (Block::Image { width: w, height: h, rgb }, Some((proto, px))) = (b, graphics.active()) {
                    let (gutter, cols) = image_cols(inner, line_numbers);
                    let span = b.rows(cols, inner.height.max(1));
                    if let Some(g) = graphics::prepare(proto, px, *w, *h, rgb, cols, span) {
                        images.push(LoweredImage { row: start, col: gutter, span: span as usize, graphic: g });
                    }
                }
                for row in lower_block(b, label_w, inner, line_numbers, theme) {
                    origins.push(match (&row, text_len) {
                        (Row::Text(text, _), Some(n)) => Some((i, text.len() - n)),
                        _ => None,
                    });
                    rows.push(row);
                }
            }
            let width = rows
                .iter()
                .filter_map(|r| match r {
                    // An image row is exactly the panel width, so it never widens
                    // the h-scroll bound.
                    Row::Text(text, _) => Some(content::line_width(text)),
                    Row::Image(_) => None,
                })
                .max()
                .unwrap_or(0);
            (rows, origins, first_row, width)
        }
        PanelView::Image { width, height, rgb } => {
            let note = graphics.unavailable().map(|msg| unavailable_row(msg, theme));
            let cell_rows = inner.height.saturating_sub(u16::from(note.is_some()));
            let cells = content::image_cells(*width, *height, rgb, inner.width, cell_rows).into_iter().map(Row::Image);
            let rows: Vec<Row> = note.into_iter().chain(cells).collect();
            if let Some((proto, px)) = graphics.active()
                && let Some(g) = graphics::prepare(proto, px, *width, *height, rgb, inner.width, inner.height)
            {
                images.push(LoweredImage { row: 0, col: 0, span: rows.len(), graphic: g });
            }
            let origins = vec![None; rows.len()];
            (rows, origins, Vec::new(), 0)
        }
    };
    Lowered { view: view.clone(), size: (inner.width, inner.height), line_numbers, generation, graphics, images, rows, origins, first_row, width }
}

/// Lay out a logical [`PanelView`] into a drawable [`FunctionDraw`]. This is the
/// single content layout path — used for both a pushed view (dynamic extensions)
/// and a provider's pulled view (Preview). Core owns everything here: Field label
/// alignment, span theming (role→color, literal rgb), heading emphasis, then per
/// visible line the horizontal scroll, line numbers, and search-match highlight
/// via [`content::build_line`]. The extension supplied only logical blocks + spans.
fn layout_view(view: &Arc<PanelView>, state: &AppState, inner: Rect, title: String, theme: &Theme, mem: &mut ScrollMemory) -> FunctionDraw {
    let f = &state.function;
    let fresh = |l: &Lowered| {
        Arc::ptr_eq(&l.view, view)
            && l.size == (inner.width, inner.height)
            && l.line_numbers == f.show_line_numbers
            && l.generation == mem.generation
            && l.graphics == mem.graphics
    };
    if !mem.lowered.as_ref().is_some_and(fresh) {
        mem.lowered = Some(lower(view, inner, f.show_line_numbers, mem.generation, mem.graphics, theme));
    }
    let Some(lowered) = mem.lowered.as_ref() else { unreachable!("filled above") };
    let document = matches!(&**view, PanelView::Document(_));
    let prompt = if document { content::prompt_line(&f.search, theme) } else { None };
    let content_h = inner.height.saturating_sub(prompt.is_some() as u16) as usize;
    let total = lowered.rows.len();
    // Clamp so the last row stops at the viewport bottom — no over-scroll into blank.
    let mut scroll = if document { f.scroll.offset().min(max_scroll(total, content_h)) } else { 0 };
    if document && f.search.reveal {
        // The current match's block decides the row; the kernel keeps it as
        // the scroll after this draw.
        if let Some(&row) = f.search.matches.get(f.search.current).and_then(|m| lowered.first_row.get(m.0)) {
            scroll = row.min(max_scroll(total, content_h));
        }
        mem.revealed = Some(scroll);
    }
    let matches = &f.search.matches;
    // Clamp h-scroll to the content width (no over-scroll into blank).
    let hscroll = f.hscroll.offset().min(lowered.width.saturating_sub(inner.width as usize));
    // Place every image whose picture is wholly on screen; the rest keep
    // their cells. `blank_placed` clears the cells under them after the draw.
    if mem.graphics_allowed {
        for img in &lowered.images {
            if img.row >= scroll && img.row + img.graphic.rows as usize <= scroll + content_h {
                let (x, y) = (inner.x + img.col, inner.y + (img.row - scroll) as u16);
                let span = img.span.min(scroll + content_h - img.row) as u16;
                mem.placements.push(Placement { x, y, graphic: img.graphic.clone() });
                mem.graphic_blanks.push(Rect { x, y, width: inner.width.saturating_sub(img.col), height: span });
            }
        }
    }
    let lines: Vec<Line> = lowered.rows[scroll..(scroll + content_h).min(total)]
        .iter()
        .enumerate()
        .map(|(k, row)| match row {
            Row::Text(text, segs) => {
                // This row's matches: its block's, shifted past the label column.
                let highlights: Vec<(usize, usize, bool)> = match lowered.origins[scroll + k] {
                    Some((block, prefix)) => {
                        let lo = matches.partition_point(|m| m.0 < block);
                        let hi = matches.partition_point(|m| m.0 <= block);
                        (lo..hi).map(|i| (matches[i].1 + prefix, matches[i].2 + prefix, i == f.search.current)).collect()
                    }
                    None => Vec::new(),
                };
                content::build_line(text, segs, hscroll, scroll + k, &highlights, f.show_line_numbers, theme)
            }
            Row::Image(line) => line.clone(),
        })
        .collect();
    FunctionDraw { lines, title, scroll, total, width: lowered.width, prompt }
}

/// The muted line an image shows when `image-protocol` cannot draw on this
/// display (`ui::graphics::GraphicsConfig::unavailable`).
fn unavailable_row(msg: String, theme: &Theme) -> Row {
    let segs = vec![(msg.len(), theme.style(Role::Muted))];
    Row::Text(msg, segs)
}

/// Clear the cells under this frame's placed images: the whole block area to
/// the base color, and the picture's own cells marked `skip` so the backend
/// never paints over the graphic the event loop writes there.
fn blank_placed(frame: &mut Frame, mem: &mut ScrollMemory, theme: &Theme) {
    let base = theme.style(Role::Base);
    let buf = frame.buffer_mut();
    let area = buf.area;
    for r in mem.graphic_blanks.drain(..) {
        for y in r.top()..r.bottom().min(area.bottom()) {
            for x in r.left()..r.right().min(area.right()) {
                buf[(x, y)].reset();
                buf[(x, y)].set_style(base);
            }
        }
    }
    for p in &mem.placements {
        let r = p.rect().intersection(area);
        for y in r.top()..r.bottom() {
            for x in r.left()..r.right() {
                buf[(x, y)].set_diff_option(ratatui::buffer::CellDiffOption::Skip);
            }
        }
    }
}

/// One laid-out panel row. Text rows still carry their logical `(byte-len,
/// Style)` segments so [`content::build_line`] can apply h-scroll, line numbers
/// and search highlight per row; image rows are already-painted cells that none
/// of those apply to.
pub(super) enum Row {
    Text(String, Vec<(usize, Style)>),
    Image(Line<'static>),
}

/// Lower one logical block into the panel rows it occupies. Every block but
/// [`Block::Image`] is exactly one text row; an image is blitted at the panel
/// width into as many rows as its aspect ratio needs, capped at the viewport
/// height.
fn lower_block(b: &Block, label_w: usize, inner: Rect, line_numbers: bool, theme: &Theme) -> Vec<Row> {
    let width = inner.width as usize;
    let text_row = |(text, segs)| vec![Row::Text(text, segs)];
    match b {
        Block::Field { label, value } => {
            let prefix = format!("{label:<label_w$}  ");
            let mut segs = vec![(prefix.len(), theme.style_by_name("label"))];
            let mut text = prefix;
            for s in value {
                segs.push((s.text.len(), span_style(s, theme, false)));
                text.push_str(&s.text);
            }
            vec![Row::Text(text, segs)]
        }
        Block::Line(v) => text_row(lower_spans(v, theme, false)),
        Block::Heading(v) => text_row(lower_spans(v, theme, true)),
        Block::Rule => {
            let text = "─".repeat(width.max(1));
            let segs = vec![(text.len(), theme.style(Role::Muted))];
            vec![Row::Text(text, segs)]
        }
        Block::Image { width: w, height: h, rgb } => {
            let (gutter, cols) = image_cols(inner, line_numbers);
            let rows = b.rows(cols, inner.height.max(1));
            content::image_cells(*w, *h, rgb, cols, rows)
                .into_iter()
                .map(|line| {
                    if gutter == 0 {
                        return Row::Image(line);
                    }
                    let mut spans = vec![Span::raw(" ".repeat(gutter as usize))];
                    spans.extend(line.spans);
                    Row::Image(Line::from(spans))
                })
                .collect()
        }
    }
}

fn lower_spans(spans: &[TextSpan], theme: &Theme, emphasized: bool) -> (String, Vec<(usize, Style)>) {
    let mut text = String::new();
    let mut segs = Vec::with_capacity(spans.len());
    for s in spans {
        segs.push((s.text.len(), span_style(s, theme, emphasized)));
        text.push_str(&s.text);
    }
    (text, segs)
}

/// Resolve a logical span's style: its color (theme role or literal rgb) plus
/// its own emphasis flags. `emphasized` adds bold on top (for headings).
fn span_style(s: &TextSpan, theme: &Theme, emphasized: bool) -> Style {
    let mut style = match &s.style {
        SpanStyle::Default => Style::default(),
        SpanStyle::Role(name) => theme.style_by_name(name),
        SpanStyle::Rgb { fg, bg } => {
            let mut st = Style::default().fg(Color::Rgb(fg.0, fg.1, fg.2));
            if let Some(b) = bg {
                st = st.bg(Color::Rgb(b.0, b.1, b.2));
            }
            st
        }
    };
    for (on, m) in [
        (s.emph.bold || emphasized, Modifier::BOLD),
        (s.emph.italic, Modifier::ITALIC),
        (s.emph.underline, Modifier::UNDERLINED),
        (s.emph.dim, Modifier::DIM),
    ] {
        if on {
            style = style.add_modifier(m);
        }
    }
    style
}

/// The path whose content the function panel is showing (for the title): the
/// File-mode cursor entry, or the moving hit in any Select entry-grid view
/// (deep file-search, a shallow filter/refine). Mirrors the
/// previewer's own cursor target so the title matches what is rendered; both
/// share `AppState::cursor_path`, the single mode-dispatch for "what's under
/// the cursor".
fn content_target(state: &AppState) -> Option<std::path::PathBuf> {
    state.cursor_path()
}

fn render_dir_listing(
    frame: &mut Frame,
    area: PanelArea,
    state: &AppState,
    extensions: &ExtensionRegistry,
    mem: &mut ScrollMemory,
    panels: &mut Vec<PanelFrame>,
    theme: &Theme,
) {
    let inner = area.inner();
    let focused = super::focus(state).is_function();
    let sources = state.targets();
    let ctx = FunctionRenderCtx { width: inner.width, height: inner.height, focused, func: &state.function, sources: &sources, theme };
    if let Some(view) = extensions.provider().and_then(|p| p.view(&ctx)) {
        let title = content::title(content_target(state).as_deref(), &state.function.search);
        let draw = layout_view(&view, state, inner, title, theme, mem);
        draw_function(frame, area, inner, draw, focused, mem, panels);
        return;
    }
    let sel = state.select.as_ref().and_then(|s| s.results.get(s.selected));
    let lines = match sel {
        Some(c) => vec![Line::from(c.label.clone())],
        None => vec![Line::from("(no selection)")],
    };
    frame.render_widget(Paragraph::new(lines), inner);
    panels.push(area.frame().title("content").focused(focused));
}

/// Whether the active Select is the command flow (Argument input + Options list).
pub(super) fn is_command_select(state: &AppState) -> bool {
    matches!(state.mode(), Mode::Select(s) if s.id == "command")
}

#[allow(clippy::too_many_arguments)]
fn render_command_summary(frame: &mut Frame, area: PanelArea, inner: Rect, state: &AppState, focused: bool, panels: &mut Vec<PanelFrame>, mem: &mut ScrollMemory, theme: &Theme) {
    let mut lines: Vec<Line> = Vec::new();
    if let Mode::Select(spec) = state.mode()
        && let crate::core::OnConfirm::Resolve { template, fill } = &spec.on_confirm {
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
            // (extension ctx / tests), walk the op key plus the same args.
            use crate::core::CmdToken;
            let fallback;
            let tokens = if spec.command_line.is_empty() {
                fallback = [CmdToken::Lit(template.op.clone()), CmdToken::Opts, CmdToken::Src, CmdToken::Paths, CmdToken::Dst];
                &fallback[..]
            } else {
                &spec.command_line[..]
            };
            let disp = |p: &std::path::PathBuf| p.display().to_string();
            let mut parts: Vec<String> = Vec::new();
            for tok in tokens {
                match tok {
                    CmdToken::Lit(s) => parts.push(s.clone()),
                    CmdToken::Opts => parts.extend(opts.iter().cloned()),
                    CmdToken::Src => parts.extend(template.src.iter().map(disp)),
                    CmdToken::Paths => parts.extend(template.paths.iter().map(disp)),
                    CmdToken::Dst | CmdToken::Path => parts.push(target.clone()),
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
            lines.push(Line::from(Span::styled(hint, theme.style(Role::Muted))));
        }
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
    push_unscrolled(panels, area, "Command", focused, mem);
}
