//! The single codec for key, intent, and mode strings. The only module that
//! formats or parses these names — used by the help panel and to decode the
//! Scheme-sourced configuration (keymaps and commands) into core types.

use std::path::PathBuf;

use crate::core::{
    ExtensionIntent, ExtensionValue, Intent, Key, KeyCode, Mode, Mods, PanelContent, ResolveFill,
    ResolverRequest, SelectSpec, StyleRun, SubLayout,
};
use crate::script::condition::Cond;
use crate::script::sexpr::Datum;

/// A human-readable name for a key, e.g. `Ctrl-a`, `Enter`, `w` — the format
/// counterpart to [`parse_key`]. Kept as the codec's key-string surface (the
/// single owner of key string forms); not yet wired to a caller.
#[allow(dead_code)]
pub fn key_name(key: Key) -> String {
    let mut s = String::new();
    if key.mods.ctrl {
        s.push_str("Ctrl-");
    }
    if key.mods.alt {
        s.push_str("Alt-");
    }
    if key.mods.shift {
        if let KeyCode::Char(_) = key.code {
            // shift is folded into the character; do not prefix
        } else {
            s.push_str("Shift-");
        }
    }
    s.push_str(&match key.code {
        KeyCode::Char(' ') => "Space".into(),
        KeyCode::Char(c) => c.to_string(),
        KeyCode::Enter => "Enter".into(),
        KeyCode::Esc => "Esc".into(),
        KeyCode::Tab => "Tab".into(),
        KeyCode::BackTab => "Shift-Tab".into(),
        KeyCode::Backspace => "Backspace".into(),
        KeyCode::Delete => "Delete".into(),
        KeyCode::Left => "Left".into(),
        KeyCode::Right => "Right".into(),
        KeyCode::Up => "Up".into(),
        KeyCode::Down => "Down".into(),
        KeyCode::Home => "Home".into(),
        KeyCode::End => "End".into(),
        KeyCode::PageUp => "PageUp".into(),
        KeyCode::PageDown => "PageDown".into(),
    });
    s
}

// --- parse direction: decode Scheme-sourced config into core types ----------

/// Parse a key name (`w`, `Enter`, `Ctrl-a`, `Shift-Tab`, `Space`, …) into a Key.
pub fn parse_key(name: &str) -> Option<Key> {
    let mut mods = Mods::NONE;
    let mut rest = name;
    // Strip leading modifier segments (Ctrl-, Alt-, Shift-).
    loop {
        if let Some(r) = rest.strip_prefix("Ctrl-") {
            mods.ctrl = true;
            rest = r;
        } else if let Some(r) = rest.strip_prefix("Alt-") {
            mods.alt = true;
            rest = r;
        } else if let Some(r) = rest.strip_prefix("Shift-") {
            mods.shift = true;
            rest = r;
        } else {
            break;
        }
    }
    let code = match rest {
        "Enter" => KeyCode::Enter,
        "Esc" | "Escape" => KeyCode::Esc,
        "Tab" => KeyCode::Tab,
        "BackTab" => KeyCode::BackTab,
        "Backspace" => KeyCode::Backspace,
        "Delete" => KeyCode::Delete,
        "Space" => KeyCode::Char(' '),
        "Up" => KeyCode::Up,
        "Down" => KeyCode::Down,
        "Left" => KeyCode::Left,
        "Right" => KeyCode::Right,
        "Home" => KeyCode::Home,
        "End" => KeyCode::End,
        "PageUp" => KeyCode::PageUp,
        "PageDown" => KeyCode::PageDown,
        s => {
            let mut chars = s.chars();
            let c = chars.next()?;
            if chars.next().is_some() {
                return None; // multi-char unknown name
            }
            KeyCode::Char(c)
        }
    };
    // Shift-Tab is its own code.
    if mods.shift && code == KeyCode::Tab {
        return Some(Key { code: KeyCode::BackTab, mods: Mods::NONE });
    }
    Some(Key { code, mods })
}

/// Decode an intent datum (a symbol, or a small list form) into a core Intent.
pub fn intent_from_datum(d: &Datum) -> Option<Intent> {
    match d {
        Datum::Sym(s) => intent_from_name(s),
        Datum::List(items) => {
            let head = items.first()?.as_sym()?;
            match head {
                "push-path-input" => {
                    let op = items.get(1)?.as_sym()?;
                    if op != "mkdir" && op != "touch" {
                        return None;
                    }
                    let template = ResolverRequest {
                        op: op.to_string(),
                        src: None,
                        dst: None,
                        path: None,
                        paths: Vec::new(),
                        opts: Vec::new(),
                        label: op.to_string(),
                    };
                    let spec = SelectSpec::command(template, ResolveFill::Path, Vec::new(), None);
                    Some(Intent::PushMode(Box::new(Mode::Select(spec))))
                }
                // (ext "extension-id" "intent-id" [data]) — an extension intent.
                "ext" => {
                    let extension = items.get(1)?.text()?.to_string();
                    let intent = items.get(2)?.text()?.to_string();
                    let data = items.get(3).map(ext_value_from_datum).unwrap_or(ExtensionValue::Nil);
                    Some(Intent::Extension(ExtensionIntent { extension, intent, data }))
                }
                // (show-function-content "owner" (lines LINE…)) where
                // LINE = ("text" (LEN R G B) …). A line with no runs is plain.
                "show-function-content" => {
                    let owner = items.get(1)?.text()?.to_string();
                    let content = panel_lines_from_datum(items.get(2)?)?;
                    Some(Intent::ShowFunctionContent { owner, content })
                }
                // (update-function-view VALUE) — replace the opaque ext view state.
                "update-function-view" => Some(Intent::UpdateFunctionView(
                    items.get(1).map(ext_value_from_datum).unwrap_or(ExtensionValue::Nil),
                )),
                _ => None,
            }
        }
        _ => None,
    }
}

/// Decode a `(lines LINE…)` datum into `PanelContent::Lines`, where each
/// `LINE = ("text" RUN…)` and `RUN = (LEN R G B)` (fg only) or
/// `(LEN R G B BR BG BB)` (with a background). Runs tile the line by byte
/// length. Returns None on any malformed element.
fn panel_lines_from_datum(d: &Datum) -> Option<PanelContent> {
    let section = d.as_list()?;
    if section.first().and_then(|h| h.as_sym()) != Some("lines") {
        return None;
    }
    let mut lines = Vec::new();
    let mut styles = Vec::new();
    for line in &section[1..] {
        let parts = line.as_list()?;
        lines.push(parts.first()?.text()?.to_string());
        let mut runs = Vec::new();
        for run in &parts[1..] {
            let r = run.as_list()?;
            let int = |i: usize| match r.get(i)? {
                Datum::Int(n) => Some(*n),
                _ => None,
            };
            let bg = if r.len() >= 7 {
                Some((int(4)? as u8, int(5)? as u8, int(6)? as u8))
            } else {
                None
            };
            runs.push(StyleRun {
                len: int(0)? as usize,
                fg: (int(1)? as u8, int(2)? as u8, int(3)? as u8),
                bg,
            });
        }
        styles.push(runs);
    }
    Some(PanelContent::Lines { lines, styles })
}

fn intent_from_name(s: &str) -> Option<Intent> {
    use Intent::*;
    Some(match s {
        "cursor-up" => CursorUp,
        "cursor-down" => CursorDown,
        "cursor-left" => CursorLeft,
        "cursor-right" => CursorRight,
        "activate" => Activate,
        "cursor-top" => CursorTop,
        "cursor-bottom" => CursorBottom,
        "func-up" => FuncUp,
        "func-down" => FuncDown,
        "func-left" => FuncLeft,
        "func-right" => FuncRight,
        "toggle-select" => ToggleSelect,
        "select-all" => SelectAll,
        "clear-selection" => ClearSelection,
        "start-copy" => StartCopy,
        "start-move" => StartMove,
        "start-rename" => StartRename,
        "delete-selected" => DeleteSelected,
        "start-edit" => StartEdit,
        "cycle-function-panel" => CycleFunctionPanel,
        "hide-function-panel" => HideFunctionPanel,
        "toggle-dotfiles" => ToggleDotFiles,
        "toggle-line-numbers" => ToggleLineNumbers,
        "cycle-list-layout" => CycleListLayout,
        // LoadContent carries owner+path data, so it cannot be produced from a
        // bare keymap name; it is constructed by extensions (on_cursor_changed).
        "scroll-up" => ScrollUp,
        "scroll-down" => ScrollDown,
        "scroll-top" => ScrollTop,
        "scroll-bottom" => ScrollBottom,
        "page-up" => PageUp,
        "page-down" => PageDown,
        "function-search-start" => FunctionSearchStart,
        "function-search-submit" => FunctionSearchSubmit,
        "function-search-next" => FunctionSearchNext,
        "function-search-prev" => FunctionSearchPrev,
        "function-search-cancel" => FunctionSearchCancel,
        "confirm" => Confirm,
        "cancel" => Cancel,
        "focus-next-panel" => FocusNextPanel,
        "focus-prev-panel" => FocusPrevPanel,
        "kill-process" => KillProcess,
        "refresh" => Refresh,
        "quit" => Quit,
        "noop" => Noop,
        // Function-panel Enter sends an empty path; the reducer fills the cursor.
        "open" => Open { path: PathBuf::new() },
        "set-sublayout-content" => SetSubLayout(SubLayout::Content),
        "set-sublayout-exec" => SetSubLayout(SubLayout::Exec),
        "file-search" => PushMode(Box::new(Mode::Select(SelectSpec::file_search()))),
        "command-palette" => {
            PushMode(Box::new(Mode::Select(SelectSpec::command_palette(Vec::new()))))
        }
        _ => return None,
    })
}

/// Decode a `:when` condition datum. Absent (None) means always.
pub fn cond_from_datum(d: &Datum) -> Cond {
    match d {
        Datum::Bool(true) => Cond::Always,
        Datum::Sym(s) if s == "always" => Cond::Always,
        Datum::Sym(s) => Cond::pred(s),
        Datum::List(items) => match items.first().and_then(|d| d.as_sym()) {
            Some("not") => match items.get(1) {
                Some(inner) => Cond::not(cond_from_datum(inner)),
                None => Cond::Always,
            },
            Some("and") => Cond::And(items[1..].iter().map(cond_from_datum).collect()),
            Some("or") => Cond::Or(items[1..].iter().map(cond_from_datum).collect()),
            _ => Cond::Always,
        },
        _ => Cond::Always,
    }
}

/// Decode a list datum into core intents (each element via `intent_from_datum`).
/// Used to read the follow-up intents a dynamic extension returns.
pub fn intents_from_datum_list(d: &Datum) -> Vec<Intent> {
    d.as_list()
        .map(|items| items.iter().filter_map(intent_from_datum).collect())
        .unwrap_or_default()
}

/// Decode a datum into an ExtensionValue (the structural payload of an
/// extension intent; the inverse of `ext_value_to_scheme`).
pub fn ext_value_from_datum(d: &Datum) -> ExtensionValue {
    match d {
        Datum::Bool(b) => ExtensionValue::Bool(*b),
        Datum::Int(i) => ExtensionValue::Int(*i),
        Datum::Str(s) => ExtensionValue::String(s.clone()),
        Datum::Sym(s) => ExtensionValue::String(s.clone()),
        Datum::List(items) => {
            ExtensionValue::List(items.iter().map(ext_value_from_datum).collect())
        }
    }
}

/// Render an ExtensionValue as a Scheme datum string (passed across the C ABI to
/// a dynamic extension's handle_intent).
pub fn ext_value_to_scheme(v: &ExtensionValue) -> String {
    fn esc(s: &str) -> String {
        s.replace('\\', "\\\\").replace('"', "\\\"")
    }
    match v {
        ExtensionValue::Nil => "()".into(),
        ExtensionValue::Bool(b) => if *b { "#t".into() } else { "#f".into() },
        ExtensionValue::Int(i) => i.to_string(),
        ExtensionValue::String(s) => format!("\"{}\"", esc(s)),
        ExtensionValue::Path(p) => format!("\"{}\"", esc(&p.display().to_string())),
        ExtensionValue::List(items) => {
            let inner: Vec<String> = items.iter().map(ext_value_to_scheme).collect();
            format!("({})", inner.join(" "))
        }
        ExtensionValue::Map(m) => {
            let pairs: Vec<String> = m
                .iter()
                .map(|(k, val)| format!("(\"{}\" {})", esc(k), ext_value_to_scheme(val)))
                .collect();
            format!("({})", pairs.join(" "))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::sexpr::parse;

    #[test]
    fn ext_intent_form_decodes() {
        let d = parse("(ext \"example\" \"greet\")").unwrap();
        match intent_from_datum(&d) {
            Some(Intent::Extension(ei)) => {
                assert_eq!(ei.extension, "example");
                assert_eq!(ei.intent, "greet");
            }
            other => panic!("expected extension intent, got {other:?}"),
        }
    }

    #[test]
    fn intent_list_decodes() {
        let d = parse("(refresh quit)").unwrap();
        assert_eq!(intents_from_datum_list(&d), vec![Intent::Refresh, Intent::Quit]);
    }

    #[test]
    fn show_function_content_decodes() {
        let d = parse("(show-function-content \"ext\" (lines (\"ab\" (2 10 20 30)) (\"plain\")))").unwrap();
        match intent_from_datum(&d) {
            Some(Intent::ShowFunctionContent { owner, content: PanelContent::Lines { lines, styles } }) => {
                assert_eq!(owner, "ext");
                assert_eq!(lines, vec!["ab".to_string(), "plain".to_string()]);
                assert_eq!(styles[0], vec![StyleRun { len: 2, fg: (10, 20, 30), bg: None }]);
                assert!(styles[1].is_empty(), "a line with no runs is plain");
            }
            other => panic!("expected ShowFunctionContent, got {other:?}"),
        }
    }

    #[test]
    fn show_function_content_decodes_background_runs() {
        let d = parse("(show-function-content \"e\" (lines (\"ab\" (2 1 2 3 4 5 6))))").unwrap();
        match intent_from_datum(&d) {
            Some(Intent::ShowFunctionContent { content: PanelContent::Lines { styles, .. }, .. }) => {
                assert_eq!(styles[0][0], StyleRun { len: 2, fg: (1, 2, 3), bg: Some((4, 5, 6)) });
            }
            other => panic!("expected styled content, got {other:?}"),
        }
    }

    #[test]
    fn update_function_view_decodes() {
        let d = parse("(update-function-view 7)").unwrap();
        assert_eq!(intent_from_datum(&d), Some(Intent::UpdateFunctionView(ExtensionValue::Int(7))));
    }

    #[test]
    fn ext_value_renders_for_the_abi() {
        assert_eq!(ext_value_to_scheme(&ExtensionValue::Int(7)), "7");
        assert_eq!(ext_value_to_scheme(&ExtensionValue::Bool(true)), "#t");
        assert_eq!(ext_value_to_scheme(&ExtensionValue::String("a\"b".into())), "\"a\\\"b\"");
    }
}
