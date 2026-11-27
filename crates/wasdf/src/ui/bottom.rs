//! The bottom panel: the help row (see doc/UI.md, doc/HELP.md), a single
//! unboxed line of key hints for the current mode and focus. The bar spans edge
//! to edge; each hint's trigger key is shown as a "negative" (inverted) chip,
//! the rest of the label in the normal bar style. All colors are themeable (the
//! `help-*` roles).
//!
//! The hints are **generated from the keymap actually in force** — every chip is
//! a key that would really fire, resolved through the same walk `handle_key`
//! uses. Nothing here holds a second copy of the bindings, so the row cannot
//! drift from them.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::core::{AppState, Role, Theme};
use crate::script::codec;
use crate::script::condition::Conditions;
use crate::script::keymap::KeymapRegistry;

/// One hint: a trigger `key` (empty = plain instructional text) and its `label`.
type Hint = (String, String);

/// Render the help row.
pub fn render_bottom(
    frame: &mut Frame,
    area: Rect,
    state: &AppState,
    keymap: &KeymapRegistry,
    conds: &Conditions,
    theme: &Theme,
) {
    let hints = hints_for(state, keymap, conds);

    let bar = theme.style(Role::Help);
    let key = theme.style(Role::HelpKey);

    let mut spans: Vec<Span> = Vec::new();
    spans.push(Span::styled(" ", bar)); // left pad
    for (i, (k, label)) in hints.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled("  ", bar));
        }
        if k.is_empty() {
            // Instructional text with no trigger key.
            spans.push(Span::styled(label.clone(), bar));
        } else if let Some((first, rest)) = key_leads_label(k, label) {
            // The key spells the label's start → highlight just that part.
            spans.push(Span::styled(first, key));
            spans.push(Span::styled(rest, bar));
        } else {
            // Otherwise show the key chip, then the label.
            spans.push(Span::styled(chip_name(k).to_string(), key));
            spans.push(Span::styled(" ", bar));
            spans.push(Span::styled(label.clone(), bar));
        }
    }

    // `.style(bar)` fills the whole row background edge to edge; spans paint over it.
    frame.render_widget(Paragraph::new(Line::from(spans)).style(bar), area);
}

/// If `key`, as the help row shows it, spells the start of `label`
/// (case-insensitively: `f`/`find`, `Del`/`delete`), split `label` into the
/// key part — written as the key — and the rest; otherwise `None`.
fn key_leads_label(key: &str, label: &str) -> Option<(String, String)> {
    let key = chip_name(key);
    let n = key.len();
    let head = label.get(..n)?;
    head.eq_ignore_ascii_case(key).then(|| (key.to_string(), label[n..].to_string()))
}

/// How the help row spells a key: `Del` for Delete, so it can lead `delete`.
fn chip_name(key: &str) -> &str {
    match key {
        "Delete" => "Del",
        other => other,
    }
}

/// The hint list for the current state.
fn hints_for(state: &AppState, keymap: &KeymapRegistry, conds: &Conditions) -> Vec<Hint> {
    // A captured keyboard reaches no binding at all, so a keymap-derived row
    // would be empty — worst in the one state where the user most needs to be
    // told the way out. Render the capture itself instead; everything else is
    // suppressed because nothing else is reachable.
    if let Some(cap) = &state.input_capture {
        return capture_hints(cap, state, keymap, conds);
    }
    let mut hints: Vec<Hint> = Vec::new();
    if let Some(text) = input_hint(state) {
        hints.push((String::new(), text));
    }
    hints.extend(keymap.help_hints(conds, state, &state.focused_panel));
    hints
}

/// The row while an extension holds the raw keyboard: the release key, then
/// whatever else core will honour. Both come from the capture and the keymap,
/// so they follow a rebound prefix or a rebound cursor key without this knowing.
fn capture_hints(
    cap: &crate::core::InputCapture,
    state: &AppState,
    keymap: &KeymapRegistry,
    conds: &Conditions,
) -> Vec<Hint> {
    let mut hints = vec![(codec::key_name(cap.release), "release".to_string())];
    let mut keys: Vec<crate::core::Key> = Vec::new();
    for intent in &cap.escape {
        for k in keymap.keys_for_intent(conds, state, &state.focused_panel, intent) {
            if !keys.contains(&k) {
                keys.push(k);
            }
        }
    }
    if !keys.is_empty() {
        let chip: Vec<String> = keys.iter().map(|k| codec::key_name(*k)).collect();
        let joined = if chip.iter().all(|n| n.chars().count() == 1) {
            chip.concat()
        } else {
            chip.join("/")
        };
        hints.push((joined, "file cursor".to_string()));
    }
    hints
}

/// The keyless "what this input is for" line, when one is open. It describes a
/// *mode with an open input*, not a key, so it lives on the spec that opened it
/// rather than in a table here (doc/HELP.md).
fn input_hint(state: &AppState) -> Option<String> {
    if state.function.search.input_active {
        return Some(state.function.search.hint.clone());
    }
    match state.mode() {
        crate::core::Mode::Select(spec) => spec.input_hint.clone(),
        crate::core::Mode::InputForm(spec) => spec.input_hint.clone(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{InputCapture, Intent, Key, KeyCode, Mods};
    use crate::script::keymap::{Binding, Layer};
    use crate::script::condition::Cond;

    fn state() -> AppState {
        AppState::new(std::path::PathBuf::from("/"))
    }

    fn bind(key: Key, intent: Intent, help: Option<&str>) -> Binding {
        Binding {
            mode: "file".into(),
            panel: Some("file".into()),
            key,
            intent,
            when: Cond::Always,
            layer: Layer::Core,
            help: help.map(str::to_string),
            owner: None,
        }
    }

    fn keymap() -> KeymapRegistry {
        let mut km = KeymapRegistry::new();
        for (c, i) in [
            ('w', Intent::CursorUp),
            ('a', Intent::CursorLeft),
            ('s', Intent::CursorDown),
            ('d', Intent::CursorRight),
        ] {
            km.add(bind(Key::plain(KeyCode::Char(c)), i, Some("cursor")));
        }
        km.add(bind(Key::plain(KeyCode::Char('q')), Intent::Quit, Some("quit")));
        km
    }

    fn capture(escape: Vec<Intent>) -> InputCapture {
        InputCapture {
            owner: "someone".into(),
            release: Key::new(KeyCode::Char('b'), Mods::CTRL),
            escape,
        }
    }

    #[test]
    fn a_captured_keyboard_shows_the_release_key_and_nothing_else_of_the_keymap() {
        // Under capture no binding is reachable, so advertising them would be a
        // lie — and the one key that *is* reachable is the one to show.
        let mut s = state();
        s.input_capture = Some(capture(Vec::new()));
        let hints = hints_for(&s, &keymap(), &Conditions::default());
        assert_eq!(hints, vec![("Ctrl-b".to_string(), "release".to_string())]);
    }

}
