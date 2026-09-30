//! The single codec for key, intent, and mode strings. The only module that
//! formats or parses these names — used by the help panel and to decode the
//! Scheme-sourced configuration (keymaps and commands) into core types.

use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::core::{
    Block, ExtensionIntent, ExtensionValue, Intent, Key, KeyCode, KeyKind, Mode, Mods, PanelView,
    SelectSpec, SpanStyle, SubLayout, TextSpan,
};
use crate::script::condition::Cond;
use crate::script::sexpr::Datum;

/// A human-readable name for a key, e.g. `Ctrl-a`, `Enter`, `w` — the format
/// counterpart to [`parse_key`]. Kept as the codec's key-string surface (the
/// single owner of key string forms).
pub fn key_name(key: Key) -> String {
    let mut s = String::new();
    if key.mods.ctrl {
        s.push_str("Ctrl-");
    }
    if key.mods.alt {
        s.push_str("Alt-");
    }
    if key.mods.sup {
        s.push_str("Super-");
    }
    if key.mods.hyper {
        s.push_str("Hyper-");
    }
    if key.mods.meta {
        s.push_str("Meta-");
    }
    if key.mods.shift {
        if let KeyCode::Char(_) = key.code {
            // shift is folded into the character; do not prefix
        } else {
            s.push_str("Shift-");
        }
    }
    s.push_str(&code_name(key.code));
    s
}

/// The bare code's spelling, without any modifier prefix. Split out from
/// [`key_name`] because [`encode_key`] carries the modifiers as separate
/// fields and must not have them baked into the code string.
pub fn code_name(code: KeyCode) -> String {
    match code {
        KeyCode::Char(' ') => "Space".into(),
        KeyCode::Char(c) => c.to_string(),
        KeyCode::Enter => "Enter".into(),
        KeyCode::Esc => "Esc".into(),
        KeyCode::Tab => "Tab".into(),
        KeyCode::BackTab => "Shift-Tab".into(),
        KeyCode::Backspace => "Backspace".into(),
        KeyCode::Delete => "Delete".into(),
        KeyCode::Insert => "Insert".into(),
        KeyCode::Left => "Left".into(),
        KeyCode::Right => "Right".into(),
        KeyCode::Up => "Up".into(),
        KeyCode::Down => "Down".into(),
        KeyCode::Home => "Home".into(),
        KeyCode::End => "End".into(),
        KeyCode::PageUp => "PageUp".into(),
        KeyCode::PageDown => "PageDown".into(),
        KeyCode::F(n) => format!("F{n}"),
    }
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
        } else if let Some(r) = rest.strip_prefix("Super-") {
            mods.sup = true;
            rest = r;
        } else if let Some(r) = rest.strip_prefix("Hyper-") {
            mods.hyper = true;
            rest = r;
        } else if let Some(r) = rest.strip_prefix("Meta-") {
            mods.meta = true;
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
        "Insert" => KeyCode::Insert,
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
            // `F` + digits, before the single-char fallback, so `F5` is the
            // function key and a lone `F` is still the letter.
            if let Some(n) = s.strip_prefix('F').and_then(|d| d.parse::<u8>().ok()) {
                KeyCode::F(n)
            } else {
                let mut chars = s.chars();
                let c = chars.next()?;
                if chars.next().is_some() {
                    return None; // multi-char unknown name
                }
                KeyCode::Char(c)
            }
        }
    };
    // Shift-Tab is its own code.
    if mods.shift && code == KeyCode::Tab {
        return Some(Key::plain(KeyCode::BackTab));
    }
    Some(Key::new(code, mods))
}

/// A [`Key`] as an [`ExtensionValue::Map`] — the wire form of a captured
/// keystroke on its way to the owning extension. Lossless, unlike
/// [`key_name`], which has no spelling for [`KeyKind`] and folds `Shift-Tab`
/// into a code.
pub fn encode_key(key: Key) -> ExtensionValue {
    let mut m = BTreeMap::new();
    m.insert("code".into(), ExtensionValue::String(code_name(key.code)));
    for (name, on) in [
        ("ctrl", key.mods.ctrl),
        ("shift", key.mods.shift),
        ("alt", key.mods.alt),
        ("super", key.mods.sup),
        ("hyper", key.mods.hyper),
        ("meta", key.mods.meta),
    ] {
        m.insert(name.into(), ExtensionValue::Bool(on));
    }
    let kind = match key.kind {
        KeyKind::Press => "press",
        KeyKind::Repeat => "repeat",
        KeyKind::Release => "release",
    };
    m.insert("kind".into(), ExtensionValue::String(kind.into()));
    ExtensionValue::Map(m)
}

/// Inverse of [`encode_key`].
pub fn decode_key(v: &ExtensionValue) -> Option<Key> {
    let ExtensionValue::Map(m) = v else { return None };
    let flag = |k: &str| m.get(k).and_then(|v| v.as_bool()).unwrap_or(false);
    // `parse_key` owns every code spelling; going through it keeps one table.
    let mut key = parse_key(m.get("code")?.as_str()?)?;
    key.mods = Mods {
        ctrl: flag("ctrl"),
        shift: flag("shift"),
        alt: flag("alt"),
        sup: flag("super"),
        hyper: flag("hyper"),
        meta: flag("meta"),
    };
    key.kind = match m.get("kind").and_then(|v| v.as_str()) {
        Some("repeat") => KeyKind::Repeat,
        Some("release") => KeyKind::Release,
        _ => KeyKind::Press,
    };
    Some(key)
}

/// Decode an intent datum (a symbol, or a small list form) into a core Intent.
pub fn intent_from_datum(d: &Datum) -> Option<Intent> {
    match d {
        Datum::Sym(s) => intent_from_name(s),
        Datum::List(items) => {
            let head = items.first()?.as_sym()?;
            match head {
                // (navigate-to "path") — navigate to a real or virtual path.
                "navigate-to" => {
                    let path = items.get(1)?.text()?.to_string();
                    Some(Intent::NavigateTo(std::path::PathBuf::from(path)))
                }
                // (ext "extension-id" "intent-id" [data]) — an extension intent.
                "ext" => extension_intent_from_ext_form(items).map(Intent::Extension),
                // (show-function-content "owner" (document BLOCK…)) — a logical
                // view core lays out (see `panel_view_from_datum`).
                "show-function-content" => {
                    let owner = items.get(1)?.text()?.to_string();
                    let view = panel_view_from_datum(items.get(2)?)?;
                    Some(Intent::ShowFunctionContent { owner, view })
                }
                // (update-function-view VALUE) — replace the opaque ext view state.
                "update-function-view" => Some(Intent::UpdateFunctionView(
                    items.get(1).map(ext_value_from_datum).unwrap_or(ExtensionValue::Nil),
                )),
                // (notify "text") — show a transient user-facing notice.
                "notify" => Some(Intent::Notify(items.get(1)?.text()?.to_string())),
                // (push-overlay "ext" "mode") — push an overlay extension mode
                // (falls through to the mode below for unbound keys; pop restores).
                "push-overlay" => {
                    let extension = items.get(1)?.text()?.to_string();
                    let mode = items.get(2)?.text()?.to_string();
                    Some(Intent::PushMode(Box::new(Mode::Extension {
                        extension,
                        mode,
                        state: ExtensionValue::Nil,
                        overlay: true,
                    })))
                }
                _ => None,
            }
        }
        _ => None,
    }
}

/// Decode a `(document BLOCK…)` datum into a [`PanelView::Document`] — the
/// logical, layout-free content an extension pushes; core owns the layout. Each
/// BLOCK is one of:
///   - `(field "label" (SPAN…))` — a key/value row,
///   - `(line (SPAN…))`          — one logical line of rich text,
///   - `(heading (SPAN…))`       — an emphasized section heading,
///   - `(rule)`                  — a full-width separator,
/// and each SPAN is `(span "text")` (default color), `(span "text" (role "name"))`
/// (themed), or `(span "text" (rgb R G B))` / `(rgb R G B BR BG BB)` (literal).
/// Returns None on any malformed element.
fn panel_view_from_datum(d: &Datum) -> Option<PanelView> {
    let section = d.as_list()?;
    if section.first().and_then(|h| h.as_sym()) != Some("document") {
        return None;
    }
    let blocks: Option<Vec<Block>> = section[1..].iter().map(block_from_datum).collect();
    Some(PanelView::Document(blocks?))
}

fn block_from_datum(d: &Datum) -> Option<Block> {
    let list = d.as_list()?;
    match list.first()?.as_sym()? {
        "field" => {
            let label = list.get(1)?.text()?.to_string();
            let value = spans_from_datum(list.get(2)?)?;
            Some(Block::Field { label, value })
        }
        "line" => Some(Block::Line(spans_from_datum(list.get(1)?)?)),
        "heading" => Some(Block::Heading(spans_from_datum(list.get(1)?)?)),
        "rule" => Some(Block::Rule),
        _ => None,
    }
}

/// Decode a `(SPAN…)` list, each `(span "text" [style])`.
fn spans_from_datum(d: &Datum) -> Option<Vec<TextSpan>> {
    d.as_list()?.iter().map(span_from_datum).collect()
}

fn span_from_datum(d: &Datum) -> Option<TextSpan> {
    let list = d.as_list()?;
    if list.first()?.as_sym()? != "span" {
        return None;
    }
    let text = list.get(1)?.text()?.to_string();
    let style = match list.get(2) {
        None => SpanStyle::Default,
        Some(s) => span_style_from_datum(s)?,
    };
    Some(TextSpan::styled(text, style))
}

fn span_style_from_datum(d: &Datum) -> Option<SpanStyle> {
    let s = d.as_list()?;
    match s.first()?.as_sym()? {
        "role" => Some(SpanStyle::Role(s.get(1)?.text()?.to_string())),
        "rgb" => {
            let int = |i: usize| match s.get(i)? {
                Datum::Int(n) => Some(*n as u8),
                _ => None,
            };
            let bg = if s.len() >= 7 {
                Some((int(4)?, int(5)?, int(6)?))
            } else {
                None
            };
            Some(SpanStyle::Rgb { fg: (int(1)?, int(2)?, int(3)?), bg })
        }
        _ => None,
    }
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
        "clear-filter" => ClearFilter,
        "delete-selected" => DeleteSelected,
        "start-edit" => StartEdit,
        "cycle-function-panel" => CycleFunctionPanel,
        "hide-function-panel" => HideFunctionPanel,
        "pop-mode" => PopMode,
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
        "add-item" => AddItem,
        "remove-item" => RemoveItem,
        "focus-next-panel" => FocusNextPanel,
        "focus-prev-panel" => FocusPrevPanel,
        "kill-process" => KillProcess,
        "toggle-vfs-root" => ToggleVfsRoot,
        "refresh" => Refresh,
        "quit" => Quit,
        "noop" => Noop,
        // Function-panel Enter sends an empty path; the reducer fills the cursor.
        "open" => Open { path: PathBuf::new() },
        "set-sublayout-content" => SetSubLayout(SubLayout::Content),
        "set-sublayout-exec" => SetSubLayout(SubLayout::Exec),
        "file-search" => PushMode(Box::new(Mode::Select(SelectSpec::file_search()))),
        // FilterListing/SelectRefine carry no data (the snapshot is taken in
        // the reducer, which has state the codec does not) — see
        // SEARCH_RENEW.md decision 3.
        "filter-list" => FilterListing,
        "select-refine" => SelectRefine,
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
            Some("option") => match items.get(1).and_then(Datum::text) {
                Some(key) => Cond::Option(key.to_string()),
                None => Cond::Pred(String::new()),
            },
            // Not part of the grammar: never holds (an empty predicate name is
            // never registered), per "an unknown condition is false".
            _ => Cond::Pred(String::new()),
        },
        _ => Cond::Pred(String::new()),
    }
}

/// Decode a list datum into core intents (each element via `intent_from_datum`).
/// Used to read the follow-up intents a dynamic extension returns.
pub fn intents_from_datum_list(d: &Datum) -> Vec<Intent> {
    d.as_list()
        .map(|items| items.iter().filter_map(intent_from_datum).collect())
        .unwrap_or_default()
}

/// Decode the fields of an `(ext "extension-id" "intent-id" [data])` form's
/// tail (`items` includes the leading `ext` symbol) into an `ExtensionIntent`.
/// Shared by `intent_from_datum`'s `"ext"` arm and `entries_from_datum_list`'s
/// leaf activation field, so both read the same shape identically.
fn extension_intent_from_ext_form(items: &[Datum]) -> Option<ExtensionIntent> {
    let extension = items.get(1)?.text()?.to_string();
    let intent = items.get(2)?.text()?.to_string();
    let data = items.get(3).map(ext_value_from_datum).unwrap_or(ExtensionValue::Nil);
    Some(ExtensionIntent { extension, intent, data })
}

/// Decode a dynamic extension's `wasdf_vfs_list` reply into `Entry`s under
/// `base` (the virtual path that was listed). Each element is one of:
///   - `(dir "display-name" "path-segment")` — a virtual subtree,
///   - `(leaf "display-name" "path-segment" (ext "id" "intent" [data]))` — an
///     activatable leaf, its `activation` decoded via the same `(ext …)` shape
///     `intent_from_datum` uses.
/// Malformed or unknown forms are skipped silently (a dynamic extension fault
/// disables that entry, not the caller).
pub fn entries_from_datum_list(d: &Datum, base: &std::path::Path) -> Vec<crate::core::Entry> {
    let Some(items) = d.as_list() else { return Vec::new() };
    items
        .iter()
        .filter_map(|form| {
            let parts = form.as_list()?;
            let head = parts.first()?.as_sym()?;
            let name = parts.get(1)?.text()?.to_string();
            let segment = parts.get(2)?.text()?.to_string();
            match head {
                "dir" => Some(crate::core::Entry::virtual_dir(base.join(segment), name)),
                "leaf" => {
                    let ext_form = parts.get(3)?.as_list()?;
                    if ext_form.first()?.as_sym()? != "ext" {
                        return None;
                    }
                    let activation = extension_intent_from_ext_form(ext_form)?;
                    let mut e = crate::core::Entry::virtual_leaf(base.join(segment), name);
                    e.activation = Some(activation);
                    Some(e)
                }
                _ => None,
            }
        })
        .collect()
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
    fn entries_from_datum_list_decodes_dirs_and_leaves() {
        let d = parse(
            "((dir \"Sub\" \"sub\") \
              (leaf \"Go\" \"go\" (ext \"myext\" \"goto\" \"/real/path\")) \
              (bogus \"skip\" \"me\"))",
        )
        .unwrap();
        let base = std::path::Path::new("@/myext");
        let entries = entries_from_datum_list(&d, base);
        assert_eq!(entries.len(), 2, "the unknown form is skipped");

        assert!(entries[0].is_dir);
        assert_eq!(entries[0].name, "Sub");
        assert_eq!(entries[0].path, base.join("sub"));
        assert!(entries[0].activation.is_none());

        assert!(!entries[1].is_dir);
        assert_eq!(entries[1].name, "Go");
        assert_eq!(entries[1].path, base.join("go"));
        let activation = entries[1].activation.as_ref().unwrap();
        assert_eq!(activation.extension, "myext");
        assert_eq!(activation.intent, "goto");
        assert_eq!(activation.data, ExtensionValue::String("/real/path".into()));
    }

    #[test]
    fn intent_list_decodes() {
        let d = parse("(refresh quit)").unwrap();
        assert_eq!(intents_from_datum_list(&d), vec![Intent::Refresh, Intent::Quit]);
    }

    #[test]
    fn show_function_content_decodes_a_document() {
        let d = parse(
            "(show-function-content \"ext\" (document \
               (field \"Name\" ((span \"hosts\"))) \
               (line ((span \"muted\" (role \"muted\")))) \
               (rule)))",
        )
        .unwrap();
        match intent_from_datum(&d) {
            Some(Intent::ShowFunctionContent { owner, view: PanelView::Document(blocks) }) => {
                assert_eq!(owner, "ext");
                assert_eq!(blocks.len(), 3);
                match &blocks[0] {
                    Block::Field { label, value } => {
                        assert_eq!(label, "Name");
                        assert_eq!(value[0], TextSpan::plain("hosts"));
                    }
                    other => panic!("expected a field, got {other:?}"),
                }
                match &blocks[1] {
                    Block::Line(spans) => assert_eq!(spans[0].style, SpanStyle::Role("muted".into())),
                    other => panic!("expected a line, got {other:?}"),
                }
                assert_eq!(blocks[2], Block::Rule);
            }
            other => panic!("expected ShowFunctionContent, got {other:?}"),
        }
    }

    #[test]
    fn span_decodes_literal_rgb_with_background() {
        let d = parse("(show-function-content \"e\" (document (line ((span \"ab\" (rgb 1 2 3 4 5 6))))))").unwrap();
        match intent_from_datum(&d) {
            Some(Intent::ShowFunctionContent { view: PanelView::Document(blocks), .. }) => {
                match &blocks[0] {
                    Block::Line(spans) => assert_eq!(
                        spans[0].style,
                        SpanStyle::Rgb { fg: (1, 2, 3), bg: Some((4, 5, 6)) },
                    ),
                    other => panic!("expected a line, got {other:?}"),
                }
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
    fn pop_mode_decodes() {
        assert_eq!(intent_from_datum(&parse("pop-mode").unwrap()), Some(Intent::PopMode));
    }

    #[test]
    fn push_overlay_decodes_an_overlay_extension_mode() {
        let d = parse("(push-overlay \"exampleext\" \"info\")").unwrap();
        match intent_from_datum(&d) {
            Some(Intent::PushMode(m)) => match *m {
                Mode::Extension { extension, mode, overlay, .. } => {
                    assert_eq!(extension, "exampleext");
                    assert_eq!(mode, "info");
                    assert!(overlay, "push-overlay is an overlay mode");
                }
                other => panic!("expected an extension mode, got {other:?}"),
            },
            other => panic!("expected PushMode, got {other:?}"),
        }
    }

    #[test]
    fn ext_value_renders_for_the_abi() {
        assert_eq!(ext_value_to_scheme(&ExtensionValue::Int(7)), "7");
        assert_eq!(ext_value_to_scheme(&ExtensionValue::Bool(true)), "#t");
        assert_eq!(ext_value_to_scheme(&ExtensionValue::String("a\"b".into())), "\"a\\\"b\"");
    }

    #[test]
    fn function_keys_and_insert_round_trip_through_their_names() {
        for code in [KeyCode::F(1), KeyCode::F(12), KeyCode::F(35), KeyCode::Insert] {
            let name = code_name(code);
            assert_eq!(parse_key(&name).map(|k| k.code), Some(code), "{name}");
        }
    }

    #[test]
    fn a_lone_f_is_still_the_letter() {
        assert_eq!(parse_key("F").map(|k| k.code), Some(KeyCode::Char('F')));
    }

    #[test]
    fn the_kitty_modifiers_survive_a_name_round_trip() {
        let key = Key::new(
            KeyCode::Char('b'),
            Mods { ctrl: true, sup: true, hyper: true, meta: true, ..Mods::NONE },
        );
        assert_eq!(parse_key(&key_name(key)), Some(key));
    }

    #[test]
    fn encode_key_round_trips_every_code_modifier_and_kind() {
        let codes = [
            KeyCode::Char('a'),
            KeyCode::Char(' '),
            KeyCode::Enter,
            KeyCode::Esc,
            KeyCode::Tab,
            KeyCode::Backspace,
            KeyCode::Delete,
            KeyCode::Insert,
            KeyCode::Left,
            KeyCode::Right,
            KeyCode::Up,
            KeyCode::Down,
            KeyCode::Home,
            KeyCode::End,
            KeyCode::PageUp,
            KeyCode::PageDown,
            KeyCode::F(5),
        ];
        let kinds = [KeyKind::Press, KeyKind::Repeat, KeyKind::Release];
        for code in codes {
            // Every one of the 64 modifier combinations, so a dropped flag or a
            // swapped pair (super/hyper/meta are easy to transpose) shows up.
            for bits in 0u8..64 {
                for kind in kinds {
                    let key = Key {
                        code,
                        mods: Mods {
                            ctrl: bits & 1 != 0,
                            shift: bits & 2 != 0,
                            alt: bits & 4 != 0,
                            sup: bits & 8 != 0,
                            hyper: bits & 16 != 0,
                            meta: bits & 32 != 0,
                        },
                        kind,
                    };
                    assert_eq!(decode_key(&encode_key(key)), Some(key), "{key:?}");
                }
            }
        }
    }

    #[test]
    fn encode_key_keeps_modifiers_out_of_the_code_field() {
        let key = Key::new(KeyCode::Char('b'), Mods::CTRL);
        let ExtensionValue::Map(m) = encode_key(key) else { panic!("not a map") };
        assert_eq!(m.get("code").and_then(|v| v.as_str()), Some("b"));
        assert_eq!(m.get("ctrl").and_then(|v| v.as_bool()), Some(true));
    }
}
