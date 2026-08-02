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
use crate::ui::chrome::{self, PanelFrame, ScrollInfo};
use crate::ui::content;

use super::{max_scroll, scrolled, ScrollMemory};

pub(super) fn render_function(
    frame: &mut Frame,
    area: Rect,
    state: &AppState,
    extensions: &ExtensionRegistry,
    hint: Option<FunctionHint>,
    mem: &mut ScrollMemory,
    panels: &mut Vec<PanelFrame>,
    theme: &Theme,
) {
    let inner = chrome::inner_open_bottom(area);
    let focused = super::focus(state).is_function();
    let scroll = state.function.scroll.offset();

    // A form extension hosting the panel (form_owner) draws the whole inner rect
    // via its render_form hook; core wraps the border/title chrome. The generic
    // seam that lets an extension own the panel without core knowing what it is.
    if let Some(owner) = &state.function.form_owner {
        if let Some(ext) = extensions.find(owner) {
            let sources = state.targets();
            let ctx = FunctionRenderCtx { width: inner.width, height: inner.height, focused, func: &state.function, sources: &sources, theme };
            if let Some(title) = ext.render_form(frame, inner, &ctx) {
                panels.push(PanelFrame::new(area).title(title).focused(focused).open_bottom(true));
            }
            return;
        }
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
        push_function(panels, area, "exec", focused, total, inner.height, scroll, mem);
        return;
    }

    // Content frame: dispatch by hint (Select) or by the content provider (File).
    match hint {
        Some(FunctionHint::CommandSummary) => render_command_summary(frame, area, inner, state, focused, panels, theme),
        Some(FunctionHint::DirListing) => render_dir_listing(frame, area, inner, state, extensions, focused, mem, panels, theme),
        Some(FunctionHint::Extension(_)) | None => {
            let sources = state.targets();
            let ctx = FunctionRenderCtx { width: inner.width, height: inner.height, focused, func: &state.function, sources: &sources, theme };
            // A pushed view (ShowFunctionContent) takes precedence over the
            // provider's own pull view. Either way core lays it out.
            let view = state.function.content.clone().or_else(|| extensions.provider().and_then(|p| p.view(&ctx)));
            if let Some(view) = view {
                let title = content::title(content_target(state).as_deref(), &state.function.search);
                let draw = layout_view(&view, state, inner, title, theme);
                draw_function(frame, area, inner, draw, focused, mem, panels);
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

/// Blit a laid-out [`FunctionDraw`] into the function panel: scroll
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

/// Lay out a logical [`PanelView`] into a drawable [`FunctionDraw`]. This is the
/// single content layout path — used for both a pushed view (dynamic extensions)
/// and a provider's pulled view (Preview). Core owns everything here: Field label
/// alignment, span theming (role→color, literal rgb), heading emphasis, then per
/// line the horizontal scroll, line numbers, and search-match highlight via
/// [`content::build_line`]. The extension supplied only logical blocks + spans.
fn layout_view(view: &PanelView, state: &AppState, inner: Rect, title: String, theme: &Theme) -> FunctionDraw {
    let f = &state.function;
    match view {
        PanelView::Document(blocks) => {
            let label_w = blocks
                .iter()
                .filter_map(|b| match b {
                    Block::Field { label, .. } => Some(label.chars().count()),
                    _ => None,
                })
                .max()
                .unwrap_or(0);
            let rows: Vec<Row> = blocks
                .iter()
                .flat_map(|b| lower_block(b, label_w, inner, f.show_line_numbers, theme))
                .collect();
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
            // Clamp h-scroll to the content width (no over-scroll into blank).
            let hscroll = f.hscroll.offset().min(width.saturating_sub(inner.width as usize));
            let lines: Vec<Line> = rows
                .iter()
                .enumerate()
                .map(|(i, row)| match row {
                    Row::Text(text, segs) => {
                        content::build_line(text, segs, hscroll, i, &f.search, f.show_line_numbers, theme)
                    }
                    Row::Image(line) => line.clone(),
                })
                .collect();
            let total = lines.len();
            FunctionDraw {
                lines,
                title,
                scroll: f.scroll.offset(),
                total,
                width,
                prompt: content::prompt_line(&f.search, theme),
            }
        }
        PanelView::Image { width, height, rgb } => {
            let img = content::image_cells(*width, *height, rgb, inner.width, inner.height);
            let total = img.len();
            FunctionDraw { lines: img, title, scroll: 0, total, width: 0, prompt: None }
        }
    }
}

/// One laid-out panel row. Text rows still carry their logical `(byte-len,
/// Style)` segments so [`content::build_line`] can apply h-scroll, line numbers
/// and search highlight per row; image rows are already-painted cells that none
/// of those apply to.
enum Row {
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
            // Keep the picture clear of the line-number gutter so it lines up
            // with the prose around it.
            let gutter = if line_numbers { LINE_NUMBER_WIDTH } else { 0 };
            let cols = inner.width.saturating_sub(gutter);
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
/// (deep file-search, a shallow filter/refine — SEARCH_RENEW.md). Mirrors the
/// previewer's own cursor target so the title matches what is rendered; both
/// share `AppState::cursor_path`, the single mode-dispatch for "what's under
/// the cursor".
fn content_target(state: &AppState) -> Option<std::path::PathBuf> {
    state.cursor_path()
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
    theme: &Theme,
) {
    let sources = state.targets();
    let ctx = FunctionRenderCtx { width: inner.width, height: inner.height, focused, func: &state.function, sources: &sources, theme };
    if let Some(view) = extensions.provider().and_then(|p| p.view(&ctx)) {
        let title = content::title(content_target(state).as_deref(), &state.function.search);
        let draw = layout_view(&view, state, inner, title, theme);
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
pub(super) fn is_command_select(state: &AppState) -> bool {
    matches!(state.mode(), Mode::Select(s) if s.id == "command")
}

fn render_command_summary(frame: &mut Frame, area: Rect, inner: Rect, state: &AppState, focused: bool, panels: &mut Vec<PanelFrame>, theme: &Theme) {
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
            lines.push(Line::from(Span::styled(hint, theme.style(Role::Muted))));
        }
    }
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
    panels.push(PanelFrame::new(area).title("Command").focused(focused).open_bottom(true));
}
