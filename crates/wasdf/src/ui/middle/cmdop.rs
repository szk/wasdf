//! CmdOp panel rendering: the copy / move / rename / mkdir / touch command
//! builder shown in the function panel. Lays out the 5-line CLI-builder preview,
//! the scrollable option and listing sections (each with its own right-edge
//! scrollbar), the inline filter row, and the destination prompt. Self-contained:
//! it reads a `CmdOpState` and the shared selection and paints into the panel's
//! inner rect; the connected frame and title are added by chrome via the pushed
//! `PanelFrame`.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap};

use crate::core::{CmdOpState, CmdToken, Role, Theme};
use crate::ui::chrome::PanelFrame;

/// Assemble the preview command line from a skeleton and live values.
fn build_preview(tokens: &[CmdToken], srcs: &[std::path::PathBuf], dest: &str, opts: &[&str]) -> String {
    let disp = |p: &std::path::PathBuf| p.display().to_string();
    let parts: Vec<String> = if tokens.is_empty() {
        let mut p: Vec<String> = opts.iter().map(|s| s.to_string()).collect();
        p.extend(srcs.iter().map(disp));
        p.push(dest.to_string());
        p
    } else {
        tokens.iter().flat_map(|tok| -> Vec<String> {
            match tok {
                CmdToken::Lit(s) => vec![s.clone()],
                CmdToken::Opts => opts.iter().map(|s| s.to_string()).collect(),
                CmdToken::Src => srcs.iter().take(1).map(disp).collect(),
                CmdToken::Paths => srcs.iter().map(disp).collect(),
                CmdToken::Dst | CmdToken::Path => vec![dest.to_string()],
            }
        }).collect()
    };
    parts.join(" ")
}

/// Build the preview line (row 0): the live command string from the CmdOp state.
fn preview_line(cmdop: &CmdOpState) -> String {
    let dest: String = if let Some(p) = &cmdop.prompt {
        p.text.clone()
    } else if cmdop.cursor >= cmdop.options.len() {
        if let Some(e) = cmdop.entries.get(cmdop.cursor - cmdop.options.len()) {
            e.path.to_string_lossy().into()
        } else {
            cmdop.list_cwd.to_string_lossy().into()
        }
    } else {
        cmdop.list_cwd.to_string_lossy().into()
    };
    let opts: Vec<&str> = cmdop.options.iter()
        .filter(|o| o.checked).map(|o| o.token.as_str()).collect();
    build_preview(&cmdop.command_line, &cmdop.srcs, &dest, &opts)
}

/// Simple substring filter for the CmdOp listing.
fn fuzzy_match_cmd(name: &str, query: &str) -> bool {
    name.to_lowercase().contains(&query.to_lowercase())
}

/// Draw a horizontal rule using the border color.
fn render_h_rule(frame: &mut Frame, x: u16, y: u16, width: u16, theme: &Theme) {
    let rule = "─".repeat(width as usize);
    frame.render_widget(
        Paragraph::new(Span::styled(rule, Style::default().fg(theme.role(Role::Border)))),
        Rect { x, y, width, height: 1 },
    );
}

/// Draw a vertical scrollbar (track + thumb, no arrows) in a single-column rect,
/// in place of an in-content section's right edge. A section that fully fits
/// shows a plain rail, matching the panel-border scrollbars from chrome.rs.
fn render_section_scrollbar(
    frame: &mut Frame,
    x: u16,
    y: u16,
    height: u16,
    total: usize,
    viewport: usize,
    position: usize,
    theme: &Theme,
) {
    if height == 0 {
        return;
    }
    let style = Style::default().fg(theme.role(Role::Border));
    if total <= viewport {
        let buf = frame.buffer_mut();
        for yy in y..y + height {
            if let Some(cell) = buf.cell_mut((x, yy)) {
                cell.set_char('│');
                cell.set_style(style);
            }
        }
        return;
    }
    let positions = total - viewport + 1;
    let mut st = ScrollbarState::new(positions)
        .viewport_content_length(viewport)
        .position(position.min(positions - 1));
    let bar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
        .begin_symbol(None)
        .end_symbol(None)
        .track_symbol(Some("│"))
        .thumb_symbol("█")
        .style(style);
    frame.render_stateful_widget(bar, Rect { x, y, width: 1, height }, &mut st);
}

/// Number of rows the CLI-builder preview occupies at the top of the panel.
const CMDOP_PREVIEW_H: u16 = 5;

pub(super) fn render_cmdop_panel(
    frame: &mut Frame,
    area: Rect,
    inner: Rect,
    cmdop: &CmdOpState,
    selection: &std::collections::HashSet<std::path::PathBuf>,
    focused: bool,
    panels: &mut Vec<PanelFrame>,
    theme: &Theme,
    mem: &mut super::ScrollMemory,
) {
    let push_frame =
        |panels: &mut Vec<PanelFrame>| {
            panels.push(PanelFrame::new(area).title(cmdop.op.label()).focused(focused).open_bottom(true));
        };
    if inner.height == 0 {
        push_frame(panels);
        return;
    }
    let bottom = inner.y + inner.height;
    // The rightmost interior column carries the section scrollbars; text rows are
    // one cell narrower so they never overprint it.
    let sb_x = inner.x + inner.width.saturating_sub(1);
    let text_w = inner.width.saturating_sub(1);
    let mut row = inner.y;

    // ── CLI builder (preview), CMDOP_PREVIEW_H rows, wrapped ──
    let preview = preview_line(cmdop);
    let preview_h = CMDOP_PREVIEW_H.min(inner.height);
    frame.render_widget(
        Paragraph::new(Span::styled(preview, Style::default().fg(theme.role(Role::Muted))))
            .wrap(Wrap { trim: false }),
        Rect { x: inner.x, y: row, width: inner.width, height: preview_h },
    );
    row += preview_h;

    if row >= bottom {
        push_frame(panels);
        return;
    }

    // ── Separator ──
    render_h_rule(frame, inner.x, row, inner.width, theme);
    row += 1;
    if row >= bottom {
        push_frame(panels);
        return;
    }

    // ── Reserve prompt rows at the bottom ──
    let prompt_h: u16 = if cmdop.prompt.is_some() { 2 } else { 0 };
    let body_bottom = bottom.saturating_sub(prompt_h);

    // Split the body between the option section and the listing section. Options
    // take what they need, capped at half the body so the listing always shows.
    let opt_sep: u16 = if cmdop.options.is_empty() { 0 } else { 1 };
    let filter_h: u16 = if cmdop.filter.is_some() { 1 } else { 0 };
    let body_h = body_bottom.saturating_sub(row);
    let opt_cap = body_h.saturating_sub(opt_sep + filter_h) / 2;
    let opt_vh: u16 = (cmdop.options.len() as u16).min(opt_cap.max(if opt_sep > 0 { 1 } else { 0 }));

    // Compute visible listing entries early so list length is available for
    // the follow_cursor scroll calculation (done after the listing height is known).
    let visible_entries: Vec<&crate::core::Entry> = if let Some(f) = &cmdop.filter {
        if f.query.is_empty() {
            cmdop.entries.iter().collect()
        } else {
            cmdop.entries.iter().filter(|e| fuzzy_match_cmd(&e.name, &f.query)).collect()
        }
    } else {
        cmdop.entries.iter().collect()
    };

    // ── Option rows (scrollable) ──
    // follow_cursor: viewport holds still while cursor stays within it; scrolls
    // only when cursor would leave the visible window (same contract as file list).
    let opt_scroll = if cmdop.cursor < cmdop.options.len() {
        let s = super::follow_cursor(mem.cmdop_opt, cmdop.cursor, opt_vh as usize, cmdop.options.len());
        mem.cmdop_opt = s;
        s
    } else {
        mem.cmdop_opt  // cursor in listing — preserve option viewport as-is
    };
    let opt_top = row;
    for vi in 0..opt_vh {
        let i = opt_scroll + vi as usize;
        let Some(opt) = cmdop.options.get(i) else { break };
        let mark = if opt.checked { "[x]" } else { "[ ]" };
        // The unified cursor is always drawn in reverse video so its position is
        // visible even while the file panel (not this panel) holds focus.
        let style = if cmdop.cursor == i {
            Style::default().add_modifier(Modifier::REVERSED)
        } else {
            Style::default().fg(theme.role(Role::Entry))
        };
        frame.render_widget(
            Paragraph::new(Span::styled(format!("{mark} {}", opt.label), style)),
            Rect { x: inner.x, y: row, width: text_w, height: 1 },
        );
        row += 1;
    }
    if opt_vh > 0 {
        render_section_scrollbar(
            frame, sb_x, opt_top, opt_vh, cmdop.options.len(), opt_vh as usize, opt_scroll, theme,
        );
    }

    // Separator between options and listing (only when options exist).
    if !cmdop.options.is_empty() && row < body_bottom {
        render_h_rule(frame, inner.x, row, inner.width, theme);
        row += 1;
    }

    // ── Filter row ──
    if let Some(f) = &cmdop.filter {
        if row < body_bottom {
            let marker = if f.active { "/ " } else { "  " };
            frame.render_widget(
                Paragraph::new(Span::styled(
                    format!("{marker}{}", f.query),
                    Style::default().fg(theme.role(Role::SelectionMarker)),
                )),
                Rect { x: inner.x, y: row, width: text_w, height: 1 },
            );
            row += 1;
        }
    }

    // ── File listing (scrollable) ──
    let list_top = row;
    let list_vh = body_bottom.saturating_sub(row);
    let list_cursor = cmdop.cursor.saturating_sub(cmdop.options.len());
    let cursor_on_listing = cmdop.cursor >= cmdop.options.len();
    let list_scroll = if cursor_on_listing {
        let s = super::follow_cursor(mem.cmdop_list, list_cursor, list_vh as usize, visible_entries.len());
        mem.cmdop_list = s;
        s
    } else {
        mem.cmdop_list  // cursor in options — preserve listing viewport as-is
    };
    for vi in 0..list_vh {
        let i = list_scroll + vi as usize;
        let Some(entry) = visible_entries.get(i) else { break };
        let is_cursor = cursor_on_listing && i == list_cursor;
        let marked = selection.contains(&entry.path);
        let style = if is_cursor {
            Style::default().add_modifier(Modifier::REVERSED)
        } else if marked {
            Style::default().fg(theme.role(Role::SelectionMarker))
        } else if entry.is_dir {
            Style::default().fg(theme.role(Role::EntryDir))
        } else {
            Style::default().fg(theme.role(Role::Entry))
        };
        // A leading marker column shows membership in the file selection.
        let marker = if marked { "*" } else { " " };
        let icon = if entry.is_dir { "/ " } else { "  " };
        frame.render_widget(
            Paragraph::new(Span::styled(format!("{marker}{icon}{}", entry.name), style)),
            Rect { x: inner.x, y: row, width: text_w, height: 1 },
        );
        row += 1;
    }
    if list_vh > 0 {
        render_section_scrollbar(
            frame, sb_x, list_top, list_vh, visible_entries.len(), list_vh as usize, list_scroll, theme,
        );
    }

    // ── Prompt ──
    if let Some(p) = &cmdop.prompt {
        let mut row = body_bottom;
        if row < bottom {
            render_h_rule(frame, inner.x, row, inner.width, theme);
            row += 1;
        }
        if row < bottom {
            let marker = Span::styled("→ ", Style::default().fg(theme.role(Role::SelectionMarker)));
            let before = Span::raw(p.text[..p.caret].to_string());
            let rest = &p.text[p.caret..];
            let under_ch = rest.chars().next();
            let under_char = under_ch.map(|c| c.to_string()).unwrap_or_else(|| " ".to_string());
            let after_start = p.caret + under_ch.map(|c| c.len_utf8()).unwrap_or(0);
            let after = Span::raw(p.text[after_start..].to_string());
            let caret_span = Span::styled(under_char, Style::default().add_modifier(Modifier::REVERSED));
            frame.render_widget(
                Paragraph::new(Line::from(vec![marker, before, caret_span, after])),
                Rect { x: inner.x, y: row, width: inner.width, height: 1 },
            );
        }
    }

    push_frame(panels);
}
