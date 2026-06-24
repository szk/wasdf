//! The bottom panel: the help row (see doc/UI.md), a single unboxed line of
//! key hints for the current mode and focus, drawn in reverse video.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::core::{AppState, Mode};

/// Render the help row.
pub fn render_bottom(frame: &mut Frame, area: Rect, state: &AppState) {
    let text = match state.mode() {
        Mode::File => {
            if state.focused_panel == "function" {
                if state.function.search.input_active {
                    "type pattern  Enter search  Esc cancel"
                } else if !state.function.search.matches.is_empty() {
                    "j/k scroll  h/l ◂▸  n/p match  g/G top/bottom  Enter open  Esc hide"
                } else {
                    "j/k scroll  h/l ◂▸  / search  n line#  g/G top/bottom  Enter open  Esc hide"
                }
            } else {
                "wasd move  Space sel  f find  x palette  c/m/R/e copy/move/rename/edit  ,panel  q quit"
            }
        }
        // The command Select's input is the Argument (dest/name), not a filter,
        // and its marks toggle the command's Options.
        Mode::Select(spec) if spec.id == "command" => {
            "type argument  w/s move  Space toggle option  Enter run  Esc cancel"
        }
        Mode::Select(_) => "type to filter  Enter confirm/next  w/s move  Space mark  Esc cancel",
        Mode::Policy(_) => "y/Enter confirm   n/Esc cancel",
        Mode::Extension { .. } => "Esc back",
    };
    // Negative-colored (reverse video) help strings.
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(text, Style::default().add_modifier(Modifier::REVERSED)))),
        area,
    );
}
