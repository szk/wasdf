//! The frame compositor: the top-level `render` entry, the shared layout split
//! helpers, and the full-screen overlays (notice and Policy confirm). The three
//! row bands it lays out are rendered by their own modules — `top` (the top
//! panel), `middle` (the main file/select area), and `bottom` (the help row).
//!
//! Panels are drawn as borderless content into their inner rects; the connected
//! single-line border, titles, and right-edge scrollbars are overlaid by the
//! chrome layer (see chrome.rs and doc/UI.md). Panel rects overlap by one cell
//! on shared edges so neighbouring borders coincide into one line with proper
//! junctions.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Cell, Clear, Gauge, List, ListItem, ListState, Paragraph, Row, Table,
};

use crate::core::{AppState, InputFormState, Mode, Notice, Role, Theme};
use crate::extension::ExtensionRegistry;
use crate::ui::chrome::{self, PanelFrame};
use crate::ui::middle::ScrollMemory;

fn top_rows(height: u16) -> u16 {
    if height >= 25 { 7 } else { 5 }
}

/// Split a rect into left and right sharing the divider column.
pub(crate) fn split_h(r: Rect, left_w: u16) -> (Rect, Rect) {
    let left_w = left_w.clamp(2, r.width.saturating_sub(2).max(2));
    let sx = r.x + left_w - 1;
    let left = Rect { x: r.x, y: r.y, width: sx - r.x + 1, height: r.height };
    let right = Rect { x: sx, y: r.y, width: r.x + r.width - sx, height: r.height };
    (left, right)
}

/// Split a rect into top and bottom sharing the divider row.
pub(crate) fn split_v(r: Rect, top_h: u16) -> (Rect, Rect) {
    let top_h = top_h.clamp(2, r.height.saturating_sub(2).max(2));
    let sy = r.y + top_h - 1;
    let top = Rect { x: r.x, y: r.y, width: r.width, height: sy - r.y + 1 };
    let bot = Rect { x: r.x, y: sy, width: r.width, height: r.y + r.height - sy };
    (top, bot)
}

pub(crate) fn ratio_left(width: u16, lw: u32, rw: u32) -> u16 {
    ((width as u32 * lw) / (lw + rw)) as u16
}

/// Pre-compute the absolute X column of the function panel's left border for the
/// current frame so `render_top` can place the bottom border path/filename labels.
/// Returns `None` when the function panel is not visible this frame.
fn compute_func_x(state: &AppState, main: Rect) -> Option<u16> {
    use crate::core::Mode;
    let show = match state.mode() {
        Mode::Select(spec) => spec.function_hint.is_some(),
        _ => state.function.visible,
    };
    if !show {
        return None;
    }
    let (lw, rw) = state.function.ratio.weights();
    // Replicate split_h's clamped left_w to find the shared column.
    let target = ratio_left(main.width, lw, rw);
    let left_w = target.clamp(2, main.width.saturating_sub(2).max(2));
    Some(main.x + left_w - 1)
}

pub fn render(
    frame: &mut Frame,
    state: &AppState,
    extensions: &ExtensionRegistry,
    scroll: &mut ScrollMemory,
    notice: Option<&Notice>,
    vfs: &crate::vfs::Vfs,
    opts: &crate::ui::top::TopOptions,
    theme: &Theme,
) {
    let area = frame.area();
    if area.width < 4 || area.height < 4 {
        return;
    }

    // Global base colors: fg/bg paint the entire frame — including the cells
    // behind the border and every panel — and are overridden per-cell wherever
    // a more specific role sets its own fg/bg. Applied first so everything that
    // follows composes on top of it. `theme.style(Role::Base)` already resolves
    // to the base fg/bg pair (see `Theme::resolved_fg_bg`), so no manual `.bg`
    // override is needed here anymore.
    frame.buffer_mut().set_style(area, theme.style(Role::Base));

    let tr = top_rows(area.height);
    // help on the last row (unboxed); the box spans the rest. Top and main
    // overlap by one row so their borders share a single divider line.
    let help_y = area.y + area.height - 1;
    let top = Rect { x: area.x, y: area.y, width: area.width, height: tr };
    let main = Rect {
        x: area.x,
        y: area.y + tr - 1,
        width: area.width,
        height: area.height.saturating_sub(tr),
    };
    let help = Rect { x: area.x, y: help_y, width: area.width, height: 1 };

    // Default to "no clamp"; the function-panel render records real bounds only
    // when it draws a scrollable frame this pass.
    scroll.reset_function_bounds();
    let mut panels: Vec<PanelFrame> = Vec::new();
    let func_x = compute_func_x(state, main);
    crate::ui::top::render_top(frame, top, state, vfs, &mut panels, opts, func_x, theme);
    match state.mode() {
        Mode::Select(_) => crate::ui::middle::render_select_main(frame, main, state, extensions, scroll, &mut panels, theme),
        _ => crate::ui::middle::render_file_main(frame, main, state, extensions, scroll, &mut panels, theme, opts.file_row_size_graph),
    }

    chrome::render(frame, &panels, &opts.border_style, &opts.scrollbar, theme);
    crate::ui::bottom::render_bottom(frame, help, state, theme);

    if let Some(n) = notice {
        render_notice(frame, area, n, theme);
    }
    if let Mode::Policy(pending) = state.mode() {
        render_policy(frame, area, pending, theme);
    }
    if let Mode::InputForm(spec) = state.mode() {
        if let Some(form) = &state.input_form {
            render_input_form(frame, area, spec, form, theme);
        }
    }
}

fn render_notice(frame: &mut Frame, area: Rect, notice: &Notice, theme: &Theme) {
    let style = if notice.error {
        theme.style(Role::NoticeErrorFg).bg(theme.role(Role::NoticeErrorBg))
    } else {
        theme.style(Role::NoticeInfoFg).bg(theme.role(Role::NoticeInfoBg))
    };
    let w = (notice.text.len() as u16 + 2).min(area.width);
    let rect = Rect { x: (area.width.saturating_sub(w)) / 2, y: 0, width: w, height: 1 };
    frame.render_widget(Clear, rect);
    frame.render_widget(Paragraph::new(format!(" {} ", notice.text)).style(style), rect);
}

/// The centered rect for a small popup dialog (Policy's Confirm box, and
/// InputForm's text-entry box) sized to its content, clamped to the frame.
fn popup_rect(area: Rect, content_w: u16, content_h: u16) -> Rect {
    let w = content_w.min(area.width.saturating_sub(2)).max(20);
    let h = content_h;
    Rect {
        x: (area.width.saturating_sub(w)) / 2,
        y: (area.height.saturating_sub(h)) / 2,
        width: w,
        height: h,
    }
}

fn render_policy(frame: &mut Frame, area: Rect, pending: &crate::core::Intent, theme: &Theme) {
    let summary = match pending {
        crate::core::Intent::RunResolver(r) => format!("{} {} item(s)?", r.op, r.paths.len()),
        crate::core::Intent::Extension(e) => format!("Run {}:{}?", e.extension, e.intent),
        _ => "Confirm operation?".into(),
    };
    // Wide enough for the longer of the summary and the key-hint line.
    let keys = "y / Enter : confirm     n / Esc : cancel";
    let body = summary.len().max(keys.len()) as u16;
    // Four body lines (blank, summary, blank, key hints) plus the two borders.
    let rect = popup_rect(area, body + 6, 6);
    frame.render_widget(Clear, rect);
    let lines = vec![
        Line::from(""),
        Line::from(Span::styled(summary, Style::default().add_modifier(Modifier::BOLD))),
        Line::from(""),
        Line::from(Span::styled(keys, Style::default().fg(Color::DarkGray))),
    ];
    let overlay_style = theme.style(Role::OverlayDangerFg).bg(theme.role(Role::OverlayDangerBg));
    let p = Paragraph::new(lines).alignment(Alignment::Center).style(overlay_style).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(overlay_style)
            .title("Confirm"),
    );
    frame.render_widget(p, rect);
}

/// A `Mode::InputForm` popup: a small centered box (same chrome shape as
/// `render_policy`'s Confirm dialog, but neutral-styled — this isn't a
/// destructive-action warning). The body widget depends on the form's kind
/// (see doc/UI.md's Input Form Mode section); only `Text` free-types through
/// `tui-input`'s caret — every other kind is a ratatui built-in widget
/// (List/Gauge/Table) driven by arrow keys.
fn render_input_form(
    frame: &mut Frame,
    area: Rect,
    spec: &crate::core::InputFormSpec,
    form: &InputFormState,
    theme: &Theme,
) {
    let keys = input_form_key_hint(form);
    let neutral_style = theme.style(Role::Base);
    let border_style = theme.style(Role::Border);

    // Body row count per kind: Text/Int show a fixed small body; Bool/
    // Choice/List grow with their row count (List: items + "+ Add" + "✓
    // Confirm"). All get one blank spacer row above the body and one hint
    // row below.
    let body_rows: u16 = match form {
        InputFormState::Text { .. } => 1,
        InputFormState::Int { .. } => 1,
        InputFormState::Bool { .. } => 2,
        InputFormState::Choice { options, .. } => options.len() as u16,
        InputFormState::List { items, .. } => (items.len() + 2) as u16,
    };
    let body_w = (spec.title.len().max(keys.len()) as u16) + 4;
    let rect = popup_rect(area, body_w, body_rows + 5);
    frame.render_widget(Clear, rect);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(spec.title.clone());
    let inner = block.inner(rect);
    frame.render_widget(Paragraph::new("").style(neutral_style).block(block), rect);

    let [_, body, _, hint] = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(body_rows),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .areas(inner);
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(keys, Style::default().fg(Color::DarkGray))))
            .alignment(Alignment::Center),
        hint,
    );

    match form {
        InputFormState::Text { value, cursor } => render_text_body(frame, body, value, *cursor),
        InputFormState::Bool { value } => {
            render_choice_body(frame, body, &["On".into(), "Off".into()], if *value { 0 } else { 1 })
        }
        InputFormState::Choice { options, selected } => render_choice_body(frame, body, options, *selected),
        InputFormState::Int { value, min, max, .. } => render_int_body(frame, body, *value, *min, *max),
        InputFormState::List { items, selected, editing } => {
            render_list_body(frame, body, items, *selected, editing)
        }
    }
}

/// The confirm/cancel (+ kind-specific) key hint line.
fn input_form_key_hint(form: &InputFormState) -> &'static str {
    match form {
        InputFormState::List { editing: Some(_), .. } => "Enter : commit     Esc : cancel",
        InputFormState::List { .. } => "↑↓ move  a add  d del  Enter : edit/confirm  Esc : cancel",
        _ => "Enter : confirm     Esc : cancel",
    }
}

/// `Text`: the typed value with a reverse-video caret cell (unchanged from
/// the original free-text-only InputForm).
fn render_text_body(frame: &mut Frame, area: Rect, value: &str, cursor: usize) {
    let caret = cursor.min(value.chars().count());
    let before: String = value.chars().take(caret).collect();
    let mut rest = value.chars().skip(caret);
    let under = rest.next().map(|c| c.to_string()).unwrap_or_else(|| " ".into());
    let after: String = rest.collect();
    let line = Line::from(vec![
        Span::raw(" "),
        Span::raw(before),
        Span::styled(under, Style::default().add_modifier(Modifier::REVERSED)),
        Span::raw(after),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

/// `Bool`/`Choice`: a `List` of options, the selected row reverse-video —
/// the same transient-`ListState` pattern `ui/middle.rs::render_select_candidates`
/// already uses (no ratatui widget state ever lives in `AppState`).
fn render_choice_body(frame: &mut Frame, area: Rect, options: &[String], selected: usize) {
    let items: Vec<ListItem> = options
        .iter()
        .enumerate()
        .map(|(i, o)| {
            let style = if i == selected { Style::default().add_modifier(Modifier::REVERSED) } else { Style::default() };
            ListItem::new(Line::from(Span::styled(o.clone(), style)))
        })
        .collect();
    let mut ls = ListState::default();
    ls.select(Some(selected));
    frame.render_stateful_widget(List::new(items), area, &mut ls);
}

/// `Int`: a `Gauge` showing the value's position in `min..max`.
fn render_int_body(frame: &mut Frame, area: Rect, value: i64, min: i64, max: i64) {
    let ratio = if max > min { (value - min) as f64 / (max - min) as f64 } else { 0.0 };
    let gauge = Gauge::default().ratio(ratio.clamp(0.0, 1.0)).label(value.to_string());
    frame.render_widget(gauge, area);
}

/// `List` (free multi-value): a `Table`, one row per item plus a trailing
/// `"+ Add item"` row and a `"✓ Confirm"` row (see `list_add_row`/
/// `list_confirm_row` in `core::reducer` for why Enter needs a dedicated
/// confirm row). The row being edited shows its live-typed text with the
/// same caret technique `render_text_body` uses.
fn render_list_body(
    frame: &mut Frame,
    area: Rect,
    items: &[String],
    selected: usize,
    editing: &Option<(usize, String, usize)>,
) {
    let mut rows: Vec<Row> = items
        .iter()
        .enumerate()
        .map(|(i, item)| {
            let text = match editing {
                Some((idx, buf, cursor)) if *idx == i => caret_line(buf, *cursor),
                _ => Line::from(item.clone()),
            };
            let style = if i == selected { Style::default().add_modifier(Modifier::REVERSED) } else { Style::default() };
            Row::new(vec![Cell::from(text)]).style(style)
        })
        .collect();
    let add_selected = selected == items.len();
    let add_text = match editing {
        Some((idx, buf, cursor)) if *idx == usize::MAX => caret_line(buf, *cursor),
        _ => Line::from("+ Add item"),
    };
    let add_style = if add_selected { Style::default().add_modifier(Modifier::REVERSED) } else { Style::default() };
    rows.push(Row::new(vec![Cell::from(add_text)]).style(add_style));
    let confirm_selected = selected == items.len() + 1;
    let confirm_style =
        if confirm_selected { Style::default().add_modifier(Modifier::REVERSED) } else { Style::default() };
    rows.push(Row::new(vec![Cell::from("✓ Confirm")]).style(confirm_style));
    let table = Table::new(rows, [Constraint::Percentage(100)]);
    frame.render_widget(table, area);
}

fn caret_line(buf: &str, cursor: usize) -> Line<'static> {
    let caret = cursor.min(buf.chars().count());
    let before: String = buf.chars().take(caret).collect();
    let mut rest = buf.chars().skip(caret);
    let under = rest.next().map(|c| c.to_string()).unwrap_or_else(|| " ".into());
    let after: String = rest.collect();
    Line::from(vec![
        Span::raw(before),
        Span::styled(under, Style::default().add_modifier(Modifier::REVERSED)),
        Span::raw(after),
    ])
}

