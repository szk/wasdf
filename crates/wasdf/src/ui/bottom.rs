//! The bottom panel: the help row (see doc/UI.md), a single unboxed line of key
//! hints for the current mode and focus. The bar spans edge to edge; each hint's
//! trigger key is shown as a "negative" (inverted) chip, the rest of the label in
//! the normal bar style. All colors are themeable (the `help-*` roles).

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::core::{AppState, Mode, Role, Theme};

/// One hint: a trigger `key` (empty = plain instructional text) and its `label`.
type Hint = (&'static str, &'static str);

/// Render the help row.
pub fn render_bottom(frame: &mut Frame, area: Rect, state: &AppState, theme: &Theme) {
    let hints: Vec<Hint> = hints_for(state);

    let bar = theme.style(Role::HelpFg).bg(theme.role(Role::HelpBg));
    let key = theme.style(Role::HelpKeyFg).bg(theme.role(Role::HelpKeyBg));

    let mut spans: Vec<Span> = Vec::new();
    spans.push(Span::styled(" ", bar)); // left pad
    for (i, (k, label)) in hints.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled("  ", bar));
        }
        if k.is_empty() {
            // Instructional text with no trigger key.
            spans.push(Span::styled(*label, bar));
        } else if let Some((first, rest)) = first_letter_is_key(k, label) {
            // The key is the label's leading letter → highlight just that letter.
            spans.push(Span::styled(first, key));
            spans.push(Span::styled(rest, bar));
        } else {
            // Otherwise show the key chip, then the label.
            spans.push(Span::styled(*k, key));
            spans.push(Span::styled(" ", bar));
            spans.push(Span::styled(*label, bar));
        }
    }

    // `.style(bar)` fills the whole row background edge to edge; spans paint over it.
    frame.render_widget(Paragraph::new(Line::from(spans)).style(bar), area);
}

/// If `key` is a single character equal (case-insensitively) to the first letter
/// of `label`, split `label` into (first-letter, rest); otherwise `None`.
fn first_letter_is_key(key: &'static str, label: &'static str) -> Option<(&'static str, &'static str)> {
    let mut kc = key.chars();
    let (k, None) = (kc.next()?, kc.next()) else { return None };
    let first = label.chars().next()?;
    if !first.eq_ignore_ascii_case(&k) {
        return None;
    }
    let split = label.char_indices().nth(1).map(|(i, _)| i).unwrap_or(label.len());
    Some(label.split_at(split))
}

/// The hint list for the current mode and focus.
fn hints_for(state: &AppState) -> Vec<Hint> {
    match state.mode() {
        Mode::File => {
            if state.focused_panel == "function" {
                if state.function.search.input_active {
                    vec![("", "type pattern"), ("Enter", "search"), ("Esc", "cancel")]
                } else if !state.function.search.matches.is_empty() {
                    vec![
                        ("j/k", "scroll"), ("h/l", "◂▸"), ("n/p", "match"),
                        ("g/G", "top/bottom"), ("Enter", "open"), ("Esc", "hide"),
                    ]
                } else {
                    vec![
                        ("j/k", "scroll"), ("h/l", "◂▸"), ("/", "search"), ("n", "line#"),
                        ("g/G", "top/bottom"), ("Enter", "open"), ("Esc", "hide"),
                    ]
                }
            } else {
                vec![
                    ("wasd", "move"), ("Space", "select"), ("f", "find"), ("z", "palette"),
                    ("c", "copy"), ("m", "move"), ("R", "rename"), ("E", "edit"),
                    ("@", "vfs"), (",", "panel"), ("q", "quit"),
                ]
            }
        }
        // The command Select's input is the Argument (dest/name), not a filter,
        // and its marks toggle the command's Options.
        Mode::Select(spec) if spec.id == "command" => vec![
            ("", "type argument"), ("w/s", "move"), ("Space", "toggle option"),
            ("Enter", "run"), ("Esc", "cancel"),
        ],
        Mode::Select(_) => vec![
            ("", "type to filter"), ("Enter", "confirm/next"), ("w/s", "move"),
            ("Space", "mark"), ("Esc", "cancel"),
        ],
        Mode::Policy(_) => vec![("y/Enter", "confirm"), ("n/Esc", "cancel")],
        Mode::InputForm(_) => vec![("", "type value"), ("Enter", "confirm"), ("Esc", "cancel")],
        Mode::Extension { .. } => vec![("Esc", "back")],
    }
}
