//! The reducer: pure, synchronous state transitions. It mutates AppState and
//! returns Effects describing the IO the kernel must perform (plans to spawn,
//! notices to show). It never performs IO itself. Async completions are
//! consumed by the pair (purpose, payload); handling never branches per intent.

use std::path::{Path, PathBuf};

use crate::core::event::{AsyncResult, AsyncStatus, Payload, Plan, Purpose, ResolverRequest};
use crate::core::intent::{Intent, Key, KeyCode};
use crate::core::mode::{InputKind, Mode, SelectInput, SelectSource, SubLayout};
use crate::core::state::{AppState, FilterFrame, InputFormState, Ratio, SelectPhase, SelectState};
use crate::core::{PANEL_FILE, PANEL_FUNCTION};
use crate::vfs;

/// A user-facing notice produced by the reducer for the event loop to display.
#[derive(Debug, Clone, PartialEq)]
pub struct Notice {
    pub text: String,
    pub error: bool,
}

impl Notice {
    pub fn info(text: impl Into<String>) -> Self {
        Notice { text: text.into(), error: false }
    }
    pub fn error(text: impl Into<String>) -> Self {
        Notice { text: text.into(), error: true }
    }
}

/// A read-only view of the command registry the reducer needs to resolve the
/// RunCommand confirm action and fill the palette. Defined in core so the
/// reducer stays decoupled from services.
pub trait CommandLookup {
    fn intent_of(&self, name: &str) -> Option<Intent>;
    fn command_candidates(&self) -> Vec<crate::core::mode::Candidate>;
    /// The key list's rows: every binding that would fire in `state`'s mode
    /// and focused panel. Default none.
    fn key_candidates(&self, _state: &AppState) -> Vec<crate::core::mode::Candidate> {
        Vec::new()
    }
    /// The selectable options (token, label) for a resolver op, for the command
    /// Select's checkboxes. Default none; the kernel supplies them from the chain.
    fn resolver_options(&self, _op: &str) -> Vec<(String, String)> {
        Vec::new()
    }
    /// The resolved command-line skeleton for an op (the literal executable and
    /// flags interleaved with placeholder tokens), for the Command panel. Default
    /// empty; the kernel supplies it from the chain.
    fn resolver_command(&self, _op: &str) -> Vec<crate::core::CmdToken> {
        Vec::new()
    }
}

/// The side-effect description returned by the reducer: plans to spawn, notices
/// to show, and follow-up intents to re-dispatch.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Effects {
    pub plans: Vec<Plan>,
    pub notices: Vec<Notice>,
    pub intents: Vec<Intent>,
}

impl Effects {
    fn none() -> Self {
        Effects::default()
    }
    fn plan(p: Plan) -> Self {
        Effects { plans: vec![p], ..Default::default() }
    }
    fn notice(n: Notice) -> Self {
        Effects { notices: vec![n], ..Default::default() }
    }
    fn emit(intents: Vec<Intent>) -> Self {
        Effects { intents, ..Default::default() }
    }
}

impl Plan {
    /// The purpose this plan completes under.
    pub fn purpose(&self) -> Purpose {
        match self {
            Plan::ReadDir { .. } => Purpose::Refresh,
            // An owner-addressed deep search completes as a generic content
            // Read (routed to the owner's accept_content), not a Select's
            // own Search completion — see doc/ARCHITECTURE.md (Async Contract).
            Plan::Search { owner: None, .. } => Purpose::Search,
            Plan::Search { owner: Some(_), .. } => Purpose::Content,
            Plan::Read { .. } => Purpose::Content,
            Plan::ResolveAndRun { .. } => Purpose::Resolver,
            Plan::Execute { .. } => Purpose::Execute,
            Plan::Suspend { .. } => Purpose::Execute,
            Plan::EvalScheme { .. } => Purpose::Scheme,
        }
    }
}

/// The middle panel an intent acts on, if any. Focus follows the action: file
/// list navigation (wasd / Enter-to-enter) targets the file panel; function-panel
/// scrolling, CmdOp listing navigation, and open (hjkl / scroll / Enter-to-open)
/// target the function panel. Intents that don't operate on a middle panel — mode
/// stack, initiators that manage focus themselves, selection-set edits, async
/// results, quit — return None and leave focus unchanged. This is the single,
/// uniform focus-routing rule; no per-intent focus code in the event loop.
fn intent_focus(intent: &Intent) -> Option<&'static str> {
    use Intent::*;
    match intent {
        CursorUp | CursorDown | CursorLeft | CursorRight | CursorTop | CursorBottom | Activate => {
            Some(PANEL_FILE)
        }
        FuncUp | FuncDown | FuncLeft | FuncRight | ScrollUp | ScrollDown | ScrollTop
        | ScrollBottom | PageUp | PageDown | Open { .. } => Some(PANEL_FUNCTION),
        _ => None,
    }
}

/// Whether the function panel is drawn: beside a Select that gives it a hint,
/// otherwise when it has been shown.
fn function_on_screen(state: &AppState) -> bool {
    match state.mode() {
        Mode::Select(spec) => spec.function_hint.is_some(),
        _ => state.function.visible,
    }
}

/// Apply an intent to the state, returning the effects to perform.
pub fn apply_intent(state: &mut AppState, intent: Intent, ctx: &dyn CommandLookup) -> Effects {
    // Focus follows the action: before handling, move focus to the middle panel
    // the intent operates on. Handlers that manage focus themselves (initiators,
    // function search, focus ops) run afterward and override as needed.
    // A hidden function panel is never focused: the keys would switch to its
    // scope with nothing on screen to show it.
    if let Some(panel) = intent_focus(&intent)
        && (panel != PANEL_FUNCTION || function_on_screen(state))
    {
        state.focused_panel = panel.into();
    }
    match intent {
        // --- Navigation (geometric; the file list is laid out by list_geom) --
        // The same cursor intents drive whichever entry grid is focused: the file
        // panel, or a path-valued Select's results (file-search). Token-valued
        // Selects (palette / options) move ±1 in their candidate list.
        Intent::CursorUp => cursor_move(state, Dir::Up),
        Intent::CursorDown => cursor_move(state, Dir::Down),
        Intent::CursorTop => cursor_jump(state, false),
        Intent::CursorBottom => cursor_jump(state, true),
        Intent::CursorLeft => cursor_move(state, Dir::Left),
        Intent::CursorRight => cursor_move(state, Dir::Right),
        Intent::Activate => activate(state),
        Intent::NavigateTo(path) => navigate_to(state, path),
        Intent::ToggleVfsRoot => toggle_vfs_root(state),

        // --- Function cursor (operate the panel from the file panel) ----
        // While a CmdOp panel is open, the function-cursor keys drive its
        // unified cursor so the listing is navigable without leaving the file
        // panel (h/l descend/ascend, j/k move the cursor).
        Intent::FuncUp => {
            state.function.scroll.retreat(1);
            Effects::none()
        }
        Intent::FuncDown => {
            state.function.scroll.advance(1);
            Effects::none()
        }
        Intent::FuncLeft => {
            state.function.hscroll.retreat(HSCROLL_STEP);
            Effects::none()
        }
        Intent::FuncRight => {
            state.function.hscroll.advance(HSCROLL_STEP);
            Effects::none()
        }

        // --- Selection --------------------------------------------------
        Intent::ToggleSelect => {
            if let Some(InputFormState::Bool { value }) = &mut state.input_form {
                *value = !*value;
                return Effects::none();
            }
            if matches!(state.mode(), Mode::Select(_)) {
                // Path results (file-search) mark into the shared, path-keyed file
                // selection; token results mark/unmark the current candidate.
                let view_path = state
                    .select
                    .as_ref()
                    .and_then(|s| s.view.as_ref())
                    .and_then(|v| v.current())
                    .map(|e| e.path.clone());
                if let Some(p) = view_path {
                    if !state.selection.remove(&p) {
                        state.selection.insert(p);
                    }
                } else if let Some(sel) = &mut state.select
                    && let Some(c) = sel.results.get(sel.selected) {
                        let label = c.label.clone();
                        if !sel.marks.remove(&label) {
                            sel.marks.insert(label);
                        }
                    }
            } else if let Some(e) = state.current_entry() {
                let p = e.path.clone();
                if !state.selection.remove(&p) {
                    state.selection.insert(p);
                }
            }
            Effects::none()
        }
        Intent::SelectAll => {
            for e in &state.entries {
                state.selection.insert(e.path.clone());
            }
            Effects::none()
        }
        Intent::ClearSelection => {
            state.selection.clear();
            Effects::none()
        }
        Intent::ClearFilter => clear_filter(state),
        Intent::FilterListing => {
            if !matches!(state.mode(), Mode::File) || state.entries.is_empty() {
                return Effects::none();
            }
            push_mode(state, Mode::Select(crate::core::SelectSpec::filter_list(state.entries.clone())), ctx)
        }
        Intent::SelectRefine => {
            let Some(entries) = state.select.as_ref().and_then(|s| s.view.as_ref()).map(|v| v.entries.clone()) else {
                return Effects::none();
            };
            if entries.is_empty() {
                return Effects::none();
            }
            push_mode(state, Mode::Select(crate::core::SelectSpec::filter_list(entries)), ctx)
        }
        Intent::LoadSearch { owner, root, query } => Effects::plan(Plan::Search {
            root,
            query,
            show_hidden: state.show_hidden,
            owner: Some(owner),
        }),

        // --- External commands → plans ----------------------------------
        // Copy / move / rename / mkdir / touch / delete all resolve here.
        Intent::RunResolver(req) => resolve_plan(req),
        Intent::Open { path } => {
            // The function-panel Enter binding sends an empty path: open cursor.
            let path = if path.as_os_str().is_empty() {
                match state.current_entry() {
                    Some(e) => e.path.clone(),
                    None => return Effects::none(),
                }
            } else {
                path
            };
            resolve_plan(ResolverRequest {
                op: "open".into(),
                src: None,
                dst: None,
                path: Some(path.to_string_lossy().into_owned()),
                paths: vec![path],
                opts: Vec::new(),
                label: "open".into(),
            })
        }
        Intent::Edit { path } => {
            let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".into());
            Effects::plan(Plan::Suspend { argv: vec![editor, path.to_string_lossy().into_owned()] })
        }
        Intent::Execute { argv } => {
            state.function.visible = true;
            state.function.sublayout = SubLayout::Exec;
            state.function.exec.lines.clear();
            state.function.exec.finished = false;
            state.function.exec.exit = None;
            Effects::plan(Plan::Execute { argv })
        }

        // --- Initiators (expanded here, then re-dispatched / pushed) ----
        Intent::DeleteSelected => {
            let paths = state.targets();
            if paths.is_empty() {
                return Effects::notice(Notice::error("nothing to delete"));
            }
            Effects::emit(vec![Intent::RunResolver(op_request("delete", paths, None))])
        }
        Intent::StartEdit => {
            let targets = state.targets();
            if targets.len() != 1 {
                return Effects::notice(Notice::error("edit needs exactly one target"));
            }
            Effects::emit(vec![Intent::Edit { path: targets.into_iter().next().unwrap() }])
        }

        // --- Mode stack -------------------------------------------------
        Intent::PushMode(mode) => push_mode(state, *mode, ctx),
        Intent::PopMode => {
            let was_select = matches!(state.mode(), Mode::Select(_));
            let was_extension = matches!(state.mode(), Mode::Extension { .. });
            state.pop_mode();
            if was_select && !matches!(state.mode(), Mode::Select(_)) {
                state.select = None;
            }
            // An overlay extension mode owned the panel content; when it pops, drop
            // that content and return focus to the file panel so the ambient
            // provider (e.g. the previewer) reclaims the panel on the next
            // cursor-changed broadcast (the kernel re-broadcasts on mode change).
            if was_extension {
                state.function.content = None;
                state.function.content_owner = None;
                state.focused_panel = PANEL_FILE.into();
            }
            Effects::none()
        }

        // --- Function panel view ----------------------------------------
        Intent::SetSubLayout(sl) => {
            state.function.visible = true;
            state.function.sublayout = sl;
            Effects::none()
        }
        Intent::CycleFunctionPanel => {
            cycle_function_panel(state);
            Effects::none()
        }
        Intent::HideFunctionPanel => {
            state.function.visible = false;
            state.focused_panel = PANEL_FILE.into();
            // Hiding the panel also exits a top overlay extension mode, so the
            // mode stack never strands a hidden overlay.
            if matches!(state.mode(), Mode::Extension { .. }) {
                state.pop_mode();
                state.function.content = None;
                state.function.content_owner = None;
            }
            Effects::none()
        }
        Intent::ShowFunctionContent { owner, view } => {
            // Generic: store the extension's view, show + focus the panel, and
            // reset the transient view state (scroll positions, any search).
            state.function.visible = true;
            state.function.sublayout = SubLayout::Content;
            state.function.scroll.go_top();
            state.function.hscroll.go_top();
            state.function.search = crate::core::PanelSearch::default();
            state.function.content = Some(std::sync::Arc::new(view.clone()));
            state.function.content_owner = Some(owner);
            state.focused_panel = PANEL_FUNCTION.into();
            Effects::none()
        }
        Intent::UpdateFunctionView(value) => {
            // Generic: store the owning extension's opaque view state verbatim.
            state.function.ext_view = value;
            Effects::none()
        }
        Intent::SetFunctionForm(owner) => {
            // Generic: an extension takes over (or releases) the function panel as
            // an interactive form. Taking over shows + focuses the panel; releasing
            // returns focus to the file panel and drops any open input.
            match owner {
                Some(id) => {
                    state.function.form_owner = Some(id);
                    state.function.visible = true;
                    state.focused_panel = PANEL_FUNCTION.into();
                }
                None => {
                    state.function.form_owner = None;
                    state.function.input = None;
                    state.focused_panel = PANEL_FILE.into();
                }
            }
            Effects::none()
        }
        Intent::HostFunctionForm(owner) => {
            // Generic: like SetFunctionForm, but passive — no focus change, so
            // a cursor-follow form (e.g. theme's per-role editor) can be shown
            // while the file panel keeps focus and keeps driving wasd/hjkl.
            match owner {
                Some(id) => {
                    state.function.form_owner = Some(id);
                    state.function.visible = true;
                }
                None => {
                    state.function.form_owner = None;
                    state.function.input = None;
                }
            }
            Effects::none()
        }
        Intent::BeginInput { purpose, seed, multiline } => {
            // Generic: open a function-panel text input for the current form owner.
            let caret = seed.len();
            state.function.input = Some(crate::core::FunctionInput { query: seed, caret, purpose, multiline });
            Effects::none()
        }
        Intent::LoadContent { owner, path, offset } => {
            // Generic cursor-follow read: remember the owner and issue the chunk read.
            state.function.content_owner = Some(owner.clone());
            Effects::plan(Plan::Read { owner, path, offset })
        }

        // --- Scroll -----------------------------------------------------
        Intent::ScrollUp => {
            scroll_target(state, |s| s.retreat(1));
            Effects::none()
        }
        Intent::ScrollDown => {
            scroll_target(state, |s| s.advance(1));
            Effects::none()
        }
        Intent::ScrollTop => {
            scroll_target(state, |s| s.go_top());
            Effects::none()
        }
        Intent::ScrollBottom => {
            scroll_target(state, |s| s.go_bottom());
            Effects::none()
        }
        Intent::PageUp => {
            scroll_target(state, |s| s.retreat(10));
            Effects::none()
        }
        Intent::PageDown => {
            scroll_target(state, |s| s.advance(10));
            Effects::none()
        }

        // --- InputForm `List` row management (browsing only) -------------
        Intent::AddItem => {
            if let Some(InputFormState::List { editing, .. }) = &mut state.input_form
                && editing.is_none() {
                    *editing = Some((usize::MAX, String::new(), 0));
                }
            Effects::none()
        }
        Intent::RemoveItem => {
            if let Some(InputFormState::List { items, selected, editing }) = &mut state.input_form
                && editing.is_none() && *selected < items.len() {
                    items.remove(*selected);
                    *selected = (*selected).min(list_confirm_row(items));
                }
            Effects::none()
        }
        Intent::SetScroll(n) => {
            state.function.scroll.set(n);
            Effects::none()
        }

        // --- Content less-style search ----------------------------------
        Intent::FunctionSearchStart => {
            let s = &mut state.function.search;
            s.input_active = true;
            s.hint = "type pattern".into();
            s.query.clear();
            s.caret = 0;
            s.matches.clear();
            s.current = 0;
            // Borrow focus to the function panel while typing the query, so Enter
            // resolves to submit-search (function scope) rather than open-file
            // (file scope). Submit/Cancel release it (R2.2).
            state.focused_panel = PANEL_FUNCTION.into();
            Effects::none()
        }
        // Submit hands the query to the owning extension, which matches over its
        // own content and returns the matches (+ a jump) as generic intents.
        Intent::FunctionSearchSubmit => {
            state.function.search.input_active = false;
            // Release focus to the file panel: jump mode is operated from the
            // file list (n/p step matches), w/a/s/d resume browsing (R2.2).
            state.focused_panel = PANEL_FILE.into();
            match state.function.content_owner.clone() {
                Some(owner) => Effects::emit(vec![Intent::Extension(crate::core::ExtensionIntent {
                    extension: owner,
                    intent: "search".into(),
                    data: crate::core::ExtensionValue::String(state.function.search.query.clone()),
                })]),
                None => Effects::none(),
            }
        }
        Intent::SetSearchMatches(matches) => {
            state.function.search.current = 0;
            // Kept sorted by (block, start): render finds a block's matches by
            // binary search instead of scanning them all.
            let mut matches = matches.clone();
            matches.sort_unstable();
            state.function.search.reveal = !matches.is_empty();
            state.function.search.matches = matches;
            Effects::none()
        }
        Intent::FunctionSearchNext => {
            search_step(state, true);
            Effects::none()
        }
        Intent::FunctionSearchPrev => {
            search_step(state, false);
            Effects::none()
        }
        Intent::FunctionSearchCancel => {
            state.function.search.input_active = false;
            state.focused_panel = PANEL_FILE.into();
            Effects::none()
        }
        Intent::ResetContentView => {
            state.function.scroll.go_top();
            state.function.hscroll.go_top();
            state.function.search = crate::core::PanelSearch::default();
            Effects::none()
        }

        // --- Select editing / generic function-panel input --------------
        Intent::RawKeyEvent(key) => {
            // A Mode::InputForm popup takes priority over everything else: it's
            // a true modal on top of the stack.
            if matches!(state.mode(), Mode::InputForm(_)) {
                return input_form_key(state, key);
            }
            // The generic function-panel input (any form extension) takes priority:
            // edit the buffer, and on Enter/Esc hand the text back to the owner.
            if state.function.input.is_some() {
                return function_input_key(state, key);
            }
            raw_key(state, key)
        }

        // --- View toggles -----------------------------------------------
        Intent::ToggleDotFiles => {
            state.show_hidden = !state.show_hidden;
            read_dir_effect(state)
        }
        Intent::ToggleLineNumbers => {
            state.function.show_line_numbers = !state.function.show_line_numbers;
            Effects::none()
        }
        Intent::CycleListLayout => {
            state.list_layout = state.list_layout.next();
            Effects::none()
        }

        // --- Panel focus ------------------------------------------------
        Intent::FocusNextPanel => {
            focus_cycle(state, true);
            Effects::none()
        }
        Intent::FocusPrevPanel => {
            focus_cycle(state, false);
            Effects::none()
        }
        Intent::FocusPanel(id) => {
            state.focused_panel = id;
            Effects::none()
        }

        Intent::KillProcess => {
            state.function.exec.finished = true;
            Effects::none()
        }

        Intent::Refresh => refresh(state),
        Intent::Quit => {
            state.quit = true;
            Effects::none()
        }
        Intent::QuitCd => {
            state.quit = true;
            state.cd_on_quit = true;
            Effects::none()
        }
        Intent::Noop => Effects::none(),
        Intent::SetInputCapture(c) => {
            state.input_capture = c.as_deref().cloned();
            Effects::none()
        }
        Intent::Notify(text) => Effects::notice(Notice::info(text.clone())),
        Intent::Configure(write) => Effects::plan(Plan::EvalScheme { expr: write.to_scheme(), owner: None }),
        Intent::Eval { owner, expr } => Effects::plan(Plan::EvalScheme { expr: expr.clone(), owner: Some(owner.clone()) }),

        // Confirm/Cancel: Policy is handled by dispatch; Select and InputForm
        // are handled here (phase switching / confirm-shape construction /
        // popping the popup all live in the reducer).
        Intent::Confirm => {
            if matches!(state.mode(), Mode::InputForm(_)) {
                return input_form_confirm(state);
            }
            select_confirm(state, ctx)
        }
        Intent::Cancel => {
            if matches!(state.mode(), Mode::InputForm(_)) {
                state.pop_mode();
                state.input_form = None;
            } else if matches!(state.mode(), Mode::Select(_)) {
                state.pop_mode();
                state.select = None;
            }
            Effects::none()
        }

        // --- Extension --------------------------------------------------
        Intent::UpdateExtensionState(value) => {
            if let Some(frame) = state.modes.last_mut()
                && let Mode::Extension { state: s, .. } = &mut frame.mode {
                    *s = value;
                }
            Effects::none()
        }
        Intent::Extension(_) => Effects::none(),

    }
}

/// A geometric move direction over the column-major grid.
#[derive(Clone, Copy)]
enum Dir {
    Up,
    Down,
    Left,
    Right,
}

/// Visual rows per column for `n` items given the last-measured geometry.
/// `list_geom.rows == 0` means the geometry has not been measured yet (before the
/// first draw, or in unit tests); fall back to a single column (`rows = n`) so
/// navigation behaves like Rows.
fn grid_rows(n: usize, geom: crate::core::ListGeom) -> usize {
    let rows = if geom.rows == 0 { n } else { geom.rows.min(n) };
    rows.max(1)
}

/// The neighbor index one step in `dir` over a column-major grid of `n` items in
/// `rows` rows (vertical neighbors are adjacent indices), or `None` at the grid
/// wall. Pure: the single stepping rule shared by the file panel and the search
/// results.
fn step_index(n: usize, rows: usize, cursor: usize, dir: Dir) -> Option<usize> {
    if n == 0 {
        return None;
    }
    let idx = cursor.min(n - 1);
    // `then` (lazy) — the subtractions must not be evaluated when the guard is false.
    match dir {
        Dir::Down => (!(idx + 1).is_multiple_of(rows) && idx + 1 < n).then(|| idx + 1),
        Dir::Up => (!idx.is_multiple_of(rows)).then(|| idx - 1),
        Dir::Left => (idx >= rows).then(|| idx - rows),
        Dir::Right => (idx + rows < n).then(|| idx + rows),
    }
}

/// Move the focused entry cursor. In a Select it drives the results (below); in
/// File mode it drives the file panel, whose grid walls carry directory
/// semantics: left at the wall escapes to the parent, right at the Rows wall acts
/// on the entry (R2.1) — a directory is entered, a file cycles the content panel.
fn cursor_move(state: &mut AppState, dir: Dir) -> Effects {
    if state.input_form.is_some() {
        return input_form_move(state, dir);
    }
    if state.select.is_some() {
        return select_move(state, dir);
    }
    let n = state.entries.len();
    let rows = grid_rows(n, state.list_geom);
    if let Some(i) = step_index(n, rows, state.cursor, dir) {
        state.cursor = i;
        return Effects::none();
    }
    match dir {
        Dir::Left => parent_dir(state),
        Dir::Right if state.list_layout == crate::core::ListLayout::Rows => {
            match state.current_entry() {
                Some(e) if e.is_dir => enter_dir(state),
                Some(_) => cycle_content(state),
                None => Effects::none(),
            }
        }
        _ => Effects::none(), // Columns/Grid right wall and up/down clamp
    }
}

/// Jump to the top/bottom of the focused entry list (Select results, an
/// InputForm widget, or the file panel).
fn cursor_jump(state: &mut AppState, bottom: bool) -> Effects {
    if state.input_form.is_some() {
        return input_form_jump(state, bottom);
    }
    if let Some(sel) = &mut state.select {
        if let Some(view) = &mut sel.view {
            view.cursor = if bottom { view.len().saturating_sub(1) } else { 0 };
        } else {
            sel.selected = if bottom { sel.results.len().saturating_sub(1) } else { 0 };
        }
    } else {
        state.cursor = if bottom { state.entries.len().saturating_sub(1) } else { 0 };
    }
    Effects::none()
}

/// Move within a Select's results: the path grid (file-search) by full 2-D
/// geometry — identical to the file panel — or the token candidate list by ±1
/// (left/right are no-ops there).
fn select_move(state: &mut AppState, dir: Dir) -> Effects {
    let geom = state.list_geom;
    let Some(sel) = &mut state.select else { return Effects::none() };
    if let Some(view) = &mut sel.view {
        let rows = grid_rows(view.len(), geom);
        if let Some(i) = step_index(view.len(), rows, view.cursor, dir) {
            view.cursor = i;
        }
        // A view-based Select (file-search/filter-list) never reaches Navigate
        // through Enter — OnConfirm::Navigate short-circuits straight to
        // commit_file_search on every Enter, by design (single Enter commits;
        // see select_confirm). Cursor movement is the only other signal that
        // the user is done typing and browsing the frozen results, so it
        // carries the same phase switch Enter performs for every other Select
        // kind — without it, select-phase-navigate (and so `f` → SelectRefine)
        // would be permanently unreachable here.
        sel.phase = SelectPhase::Navigate;
    } else {
        // The candidate list as drawn: one column (left/right do nothing), or
        // the key list's columns — the same stepping rule either way.
        let n = sel.results.len();
        if let Some(i) = step_index(n, grid_rows(n, geom), sel.selected, dir) {
            sel.selected = i;
        }
    }
    Effects::none()
}

/// The shared wasd/arrow directional intents, reinterpreted per `InputFormState`
/// variant (see keymap.scm's "input-form" scope and the "input-form-typing"
/// condition that keeps these off `Text` and a `List` row's nested edit).
fn input_form_move(state: &mut AppState, dir: Dir) -> Effects {
    let Some(form) = &mut state.input_form else { return Effects::none() };
    match form {
        InputFormState::Text { .. } => {}
        InputFormState::Bool { value } => {
            if matches!(dir, Dir::Up | Dir::Down | Dir::Left | Dir::Right) {
                *value = !*value;
            }
        }
        InputFormState::Choice { options, selected } => match dir {
            Dir::Up | Dir::Left => *selected = selected.saturating_sub(1),
            Dir::Down | Dir::Right if *selected + 1 < options.len() => *selected += 1,
            _ => {}
        },
        InputFormState::Int { value, min, max, typed } => {
            let step = match dir {
                Dir::Left => -1,
                Dir::Right => 1,
                Dir::Down => -10,
                Dir::Up => 10,
            };
            typed.clear();
            *value = (*value + step).clamp(*min, *max);
        }
        InputFormState::List { items, selected, .. } => match dir {
            Dir::Up => *selected = selected.saturating_sub(1),
            Dir::Down => *selected = (*selected + 1).min(list_confirm_row(items)),
            Dir::Left | Dir::Right => {}
        },
    }
    Effects::none()
}

/// The shared "jump to top/bottom" intent (`W`/`S` in file mode), reinterpreted
/// per kind: `Int` jumps to min/max, `Choice`/`List` jump to the first/last row.
fn input_form_jump(state: &mut AppState, bottom: bool) -> Effects {
    let Some(form) = &mut state.input_form else { return Effects::none() };
    match form {
        InputFormState::Choice { selected, options } => {
            *selected = if bottom { options.len().saturating_sub(1) } else { 0 };
        }
        InputFormState::Int { value, min, max, typed } => {
            typed.clear();
            *value = if bottom { *max } else { *min };
        }
        InputFormState::List { items, selected, .. } => {
            *selected = if bottom { list_confirm_row(items) } else { 0 };
        }
        InputFormState::Text { .. } | InputFormState::Bool { .. } => {}
    }
    Effects::none()
}

/// Cycle the content panel (R2.1): ensure the function panel is visible in the
/// Content sublayout, then cycle its width — never hiding, keeping the shared
/// ratio on first open, and leaving focus on the file panel. Distinct from
/// `cycle_function_panel` (the comma key), which hides and resets the ratio.
fn cycle_content(state: &mut AppState) -> Effects {
    let f = &mut state.function;
    if !f.visible {
        f.visible = true;
        f.sublayout = SubLayout::Content;
    } else if f.sublayout != SubLayout::Content {
        f.sublayout = SubLayout::Content;
    } else {
        f.ratio = match f.ratio {
            Ratio::R2_1 => Ratio::R1_1,
            Ratio::R1_1 => Ratio::R1_2,
            Ratio::R1_2 => Ratio::R2_1,
        };
    }
    Effects::none()
}

/// Enter the directory under the cursor (no-op on a file).
/// Move into `path`: reset the cursor and selection, then read the new listing.
/// The single landing shared by entering a child and escaping to the parent.
fn enter_directory(state: &mut AppState, path: PathBuf) -> Effects {
    state.cwd = path;
    state.cursor = 0;
    state.selection.clear();
    read_dir_effect(state)
}

fn enter_dir(state: &mut AppState) -> Effects {
    let Some(entry) = state.current_entry() else {
        return Effects::none();
    };
    if entry.is_dir {
        enter_directory(state, entry.path.clone())
    } else {
        Effects::none()
    }
}

/// Escape to the parent directory, landing the cursor on the directory we left.
///
/// VFS climb-out rules:
/// - From real `/` → go to the VFS root `@`, landing on the `filesystem` entry.
/// - From the VFS root `@` → no-op (already at the top).
/// - Otherwise → lexical `parent()` as usual (works for both real and `@/…` paths).
fn parent_dir(state: &mut AppState) -> Effects {
    // Bridge: real root → VFS root (filesystem is the landing cursor).
    if state.cwd == std::path::Path::new("/") {
        state.pending_focus = Some(PathBuf::from("/"));
        return enter_directory(state, vfs::vfs_root());
    }
    // No-op: already at the VFS root.
    if state.cwd == vfs::vfs_root() {
        return Effects::none();
    }
    if let Some(parent) = state.cwd.parent().map(PathBuf::from) {
        // Guard against the empty-path parent of a single-component relative path.
        if parent.as_os_str().is_empty() {
            return Effects::none();
        }
        state.pending_focus = Some(state.cwd.clone());
        enter_directory(state, parent)
    } else {
        Effects::none()
    }
}

/// Activate the cursor entry (Enter): forward a declared virtual intent, enter
/// the directory, or **open the file** via the resolver (R2.3). "d = look,
/// Enter = do."
fn activate(state: &mut AppState) -> Effects {
    let Some(entry) = state.current_entry() else {
        return Effects::none();
    };
    // Virtual leaf: forward the activation intent without interpreting it.
    if let Some(ei) = entry.activation.clone() {
        return Effects::emit(vec![Intent::Extension(ei)]);
    }
    if entry.is_dir {
        enter_dir(state)
    } else {
        // Reuse the resolver Open path (empty path = cursor entry).
        Effects::emit(vec![Intent::Open { path: PathBuf::new() }])
    }
}

/// `$HOME`, falling back to `/` if unset — the same fallback `toggle_vfs_root`
/// already used for its own home landing.
fn home_dir() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

fn navigate_to(state: &mut AppState, path: PathBuf) -> Effects {
    // `~` is the one shell-style path this app expands — the `~` key binds
    // literally to `(navigate-to "~")`; a typed `~` elsewhere (e.g. the
    // command Select's destination input) is not expanded, matching real
    // `cp`/`mv`'s own behavior (only the shell expands `~`, not the program).
    let path = if path == Path::new("~") { home_dir() } else { path };
    // Virtual paths and real directories: enter directly.
    // Real files: enter the parent and focus the file.
    let is_dir = vfs::is_virtual(&path) || path.is_dir();
    let (dir, focus) = if is_dir {
        (path.clone(), Some(path))
    } else {
        (path.parent().map(PathBuf::from).unwrap_or(path.clone()), Some(path))
    };
    state.cwd = dir;
    state.cursor = 0;
    state.selection.clear();
    state.pending_focus = focus;
    read_dir_effect(state)
}

/// Toggle between the current real directory and the VFS root (`@`). From a real
/// path, remember it and jump to `@`; from anywhere virtual, return to the
/// remembered path (falling back to the home directory if none was saved).
fn toggle_vfs_root(state: &mut AppState) -> Effects {
    if vfs::is_virtual(&state.cwd) {
        let target = state.root_return.take().unwrap_or_else(home_dir);
        navigate_to(state, target)
    } else {
        state.root_return = Some(state.cwd.clone());
        navigate_to(state, vfs::vfs_root())
    }
}

fn push_mode(state: &mut AppState, mode: Mode, ctx: &dyn CommandLookup) -> Effects {
    // Fill the command palette's candidates from the live registry at push time.
    let mut mode = match mode {
        Mode::Select(spec)
            if spec.id == "command-palette"
                && matches!(&spec.source, SelectSource::Static(c) if c.is_empty()) =>
        {
            Mode::Select(crate::core::SelectSpec::command_palette(ctx.command_candidates()))
        }
        // The key list describes the mode it was opened from, so it is filled
        // before the push changes the mode.
        Mode::Select(spec)
            if spec.id == "key-list" && matches!(&spec.source, SelectSource::Static(c) if c.is_empty()) =>
        {
            Mode::Select(crate::core::SelectSpec::key_list(ctx.key_candidates(state)))
        }
        other => other,
    };
    // Fill the command Select's command-line skeleton from the resolver chain, so
    // the Command panel shows the actual external command (e.g. `mv …`, `cp -R …`).
    if let Mode::Select(spec) = &mut mode
        && spec.id == "command" && spec.command_line.is_empty()
            && let crate::core::OnConfirm::Resolve { template, .. } = &spec.on_confirm {
                spec.command_line = ctx.resolver_command(&template.op);
            }
    let is_select = matches!(mode, Mode::Select(_));
    // A Select replaces a topmost Select; otherwise push.
    let replacing_select = is_select && matches!(state.mode(), Mode::Select(_));
    let spec = if let Mode::Select(spec) = &mode {
        Some(spec.clone())
    } else {
        None
    };
    if replacing_select {
        state.replace_mode(mode);
    } else {
        state.push_mode(mode);
    }
    if let Some(spec) = spec {
        let has_input = !matches!(spec.input, SelectInput::None);
        state.select = Some(SelectState::new(
            spec.initial_query.clone().unwrap_or_default(),
            has_input,
        ));
        return refilter(state);
    }
    if let Mode::InputForm(spec) = state.mode() {
        state.input_form = Some(decode_input_state(&spec.kind, &spec.initial));
    }
    Effects::none()
}

/// Build a resolver request template for `op` with the given target paths (and
/// optional single `src`); dst/path/opts are filled later by the Select flow.
fn op_request(op: &str, paths: Vec<PathBuf>, src: Option<PathBuf>) -> ResolverRequest {
    ResolverRequest {
        op: op.into(),
        src,
        dst: None,
        path: None,
        paths,
        opts: Vec::new(),
        label: op.into(),
    }
}

/// The option checkboxes a command op declares, as Static candidates (label =
/// token + description, value = the token).
fn option_candidates(ctx: &dyn CommandLookup, op: &str) -> Vec<crate::core::Candidate> {
    ctx.resolver_options(op)
        .into_iter()
        .map(|(token, label)| {
            crate::core::Candidate::new(
                format!("{token}  {label}"),
                crate::core::ExtensionValue::String(token),
            )
        })
        .collect()
}

/// Resolve a Select confirmation into follow-up intents (or a phase switch).
fn select_confirm(state: &mut AppState, ctx: &dyn CommandLookup) -> Effects {
    use crate::core::{ConfirmShape, OnConfirm, ResolveFill};
    let Mode::Select(spec) = state.mode().clone() else {
        return Effects::none();
    };
    let Some(sel) = state.select.clone() else {
        return Effects::none();
    };
    let has_input = !matches!(spec.input, SelectInput::None);

    // Deep file-search (FileWalk) and shallow filter/refine (Entries) both use
    // OnConfirm::Navigate and neither reaches a Navigate phase of its own:
    // confirming the query commits its matches straight into the file panel's
    // own listing, in File mode — see `commit_file_search`. Every existing
    // File-mode key (copy, move, rename, delete, bookmark, hjkl scroll, …)
    // then just works on the filtered listing; nothing needs a parallel
    // Select-scoped binding. (No other Select constructor uses Navigate, so
    // keying on `on_confirm` alone is equivalent to keying on the source.)
    if matches!(spec.on_confirm, OnConfirm::Navigate) {
        return commit_file_search(state, sel);
    }

    // Enter in the Input phase switches to Navigate, uniformly.
    if has_input && sel.phase == SelectPhase::Input {
        if let Some(s) = &mut state.select {
            s.phase = SelectPhase::Navigate;
        }
        return Effects::none();
    }

    // Build the confirm shape.
    let shape = match spec.input {
        // The only remaining Path-input Select is `command` (mkdir/touch/
        // cmdop), which sources its Static option checkboxes, not path
        // completions — it always confirms the typed destination text as-is.
        // (Extensions asking for free text use `Mode::InputForm`.)
        SelectInput::Path => ConfirmShape::InputOnly(sel.query.clone()),
        _ => {
            // The command Select reads its marks (option toggles) directly below;
            // elsewhere marks are inert and confirm always yields the current entry.
            if let Some(c) = sel.results.get(sel.selected) {
                ConfirmShape::Single(c.clone())
            } else {
                // Nothing to confirm.
                return Effects::none();
            }
        }
    };

    // Pop the Select before producing the follow-up.
    state.pop_mode();
    state.select = None;

    let follow = match spec.on_confirm {
        OnConfirm::Navigate => unreachable!("handled above, before the Select is even popped"),
        OnConfirm::RunCommand => match &shape {
            ConfirmShape::Single(c) => c
                .value
                .as_str()
                .and_then(|name| ctx.intent_of(name))
                .map(|i| vec![i])
                .unwrap_or_default(),
            _ => Vec::new(),
        },
        OnConfirm::Resolve { mut template, fill } => {
            // The typed input fills dst/name.
            let text = match &shape {
                ConfirmShape::InputOnly(t) => t.clone(),
                _ => return Effects::none(),
            };
            match fill {
                ResolveFill::Dst => template.dst = Some(text),
                ResolveFill::Path => template.path = Some(text),
            }
            // The marked option candidates fill opts.
            template.opts = sel
                .results
                .iter()
                .filter(|c| sel.marks.contains(&c.label))
                .filter_map(|c| c.value.as_str().map(String::from))
                .collect();
            vec![Intent::RunResolver(template)]
        }
    };
    Effects::emit(follow)
}

/// Commit a file-search query (Enter): pop the Select, adopt its matched
/// entries as the file panel's own listing, and push what the panel showed
/// before onto the filter stack so `Intent::ClearFilter` (Esc) can step back
/// out — one level per nested `f`. A no-op if the async search hasn't
/// produced a view yet (nothing to commit).
fn commit_file_search(state: &mut AppState, sel: SelectState) -> Effects {
    let Some(view) = sel.view else { return Effects::none() };
    state.pop_mode();
    state.select = None;
    state.filter_stack.push(FilterFrame {
        entries: std::mem::take(&mut state.entries),
        cursor: state.cursor,
        query: state.filter_query.take(),
    });
    state.entries = view.entries;
    state.cursor = view.cursor;
    state.selection.clear();
    state.filter_query = Some(sel.query);
    Effects::none()
}

/// Pop one filter level (Esc while filtered), restoring the entries/cursor/
/// active query it saved. Bound only while `has-filter` holds (keymap.scm);
/// a no-op with no filter active.
fn clear_filter(state: &mut AppState) -> Effects {
    let Some(frame) = state.filter_stack.pop() else { return Effects::none() };
    state.entries = frame.entries;
    state.cursor = frame.cursor;
    state.filter_query = frame.query;
    state.clamp_cursor();
    Effects::none()
}

/// A no-op command lookup for contexts that never run commands.
struct NoCommands;
impl CommandLookup for NoCommands {
    fn intent_of(&self, _name: &str) -> Option<Intent> {
        None
    }
    fn command_candidates(&self) -> Vec<crate::core::mode::Candidate> {
        Vec::new()
    }
}

fn cycle_function_panel(state: &mut AppState) {
    let f = &mut state.function;
    if !f.visible {
        f.visible = true;
        f.ratio = Ratio::R2_1;
    } else {
        match f.ratio {
            Ratio::R2_1 => f.ratio = Ratio::R1_1,
            Ratio::R1_1 => f.ratio = Ratio::R1_2,
            Ratio::R1_2 => {
                // While an extension hosts an interactive form (e.g. the CmdOp
                // builder), the panel is pinned visible, so cycling wraps back to
                // R2_1 instead of hiding it. Otherwise the last step hides it.
                if f.form_owner.is_some() {
                    f.ratio = Ratio::R2_1;
                } else {
                    f.visible = false;
                    state.focused_panel = PANEL_FILE.into();
                }
            }
        }
    }
}

/// Scroll the function panel (shared offset for both the content and exec
/// frames; the content renderer clamps it to its own length).
fn scroll_target(state: &mut AppState, f: impl FnOnce(&mut crate::core::Scroll)) {
    f(&mut state.function.scroll);
}

/// Columns the text/hex content scrolls horizontally per FuncLeft/FuncRight.
const HSCROLL_STEP: usize = 8;

/// Move to the next/previous match (wrapping) and ask the layout to bring it
/// into view.
fn search_step(state: &mut AppState, forward: bool) {
    let n = state.function.search.matches.len();
    if n == 0 {
        return;
    }
    let cur = &mut state.function.search.current;
    *cur = if forward { (*cur + 1) % n } else { (*cur + n - 1) % n };
    state.function.search.reveal = true;
}


fn focus_cycle(state: &mut AppState, forward: bool) {
    // The two focusable panels in File mode: file and (when visible) function.
    let panels: Vec<&str> = if state.function.visible {
        vec![PANEL_FILE, PANEL_FUNCTION]
    } else {
        vec![PANEL_FILE]
    };
    let cur = panels.iter().position(|p| *p == state.focused_panel).unwrap_or(0);
    let next = if forward {
        (cur + 1) % panels.len()
    } else {
        (cur + panels.len() - 1) % panels.len()
    };
    state.focused_panel = panels[next].to_string();
}

/// Issue a ReadDir for the current directory.
fn read_dir_effect(state: &AppState) -> Effects {
    Effects::plan(Plan::ReadDir {
        path: state.cwd.clone(),
        show_hidden: state.show_hidden,
    })
}

/// Refresh from disk: re-read the directory listing, and — when the function
/// panel is showing content — reload that content too. The content reload goes
/// out as a forced `LoadContent` at offset 0, which re-reads the file via the
/// kernel directly and so bypasses a content owner's same-path load cache (the
/// previewer skips a re-read for an already-loaded path). This is what makes an
/// edit performed in a suspended editor appear on return without first moving
/// the cursor off the file and back. Generic: any content owner, the cursor path.
fn refresh(state: &AppState) -> Effects {
    let mut fx = read_dir_effect(state);
    if state.function.visible && state.function.sublayout == SubLayout::Content
        && let (Some(owner), Some(entry)) =
            (state.function.content_owner.clone(), state.current_entry())
        {
            fx.intents.push(Intent::LoadContent {
                owner,
                path: entry.path.clone(),
                offset: 0,
            });
        }
    fx
}

fn resolve_plan(request: ResolverRequest) -> Effects {
    Effects::plan(Plan::ResolveAndRun { request })
}

/// Apply a raw key to the generic function-panel input (§S2). Enter/Esc close it
/// and hand the text (or a cancel) back to the `form_owner` as an extension intent
/// tagged with the input's `purpose`; every other key edits the buffer generically.
/// The reducer never interprets the text — that is the owning extension's job.
fn function_input_key(state: &mut AppState, key: Key) -> Effects {
    let Some(input) = &mut state.function.input else { return Effects::none() };
    // Multiline: Enter inserts `\n` unless the current line is already
    // empty, in which case it falls through to submit like any other Enter —
    // a blank line terminates the batch (mirrors `InputKind::List`'s
    // row-editing convention, for free text instead of discrete rows).
    if input.multiline && key.code == KeyCode::Enter && !current_line_is_empty(&input.query, input.caret) {
        input.query.insert(input.caret, '\n');
        input.caret += 1;
        return Effects::none();
    }
    match key.code {
        KeyCode::Enter | KeyCode::Esc => {
            let submitted = matches!(key.code, KeyCode::Enter);
            let Some(input) = state.function.input.take() else { return Effects::none() };
            let Some(owner) = state.function.form_owner.clone() else { return Effects::none() };
            Effects::emit(vec![Intent::Extension(crate::core::ExtensionIntent {
                extension: owner,
                intent: if submitted { "input-submit" } else { "input-cancel" }.into(),
                data: crate::core::ExtensionValue::Map(
                    [
                        ("purpose".to_string(), crate::core::ExtensionValue::String(input.purpose)),
                        ("text".to_string(), crate::core::ExtensionValue::String(input.query)),
                    ]
                    .into_iter()
                    .collect(),
                ),
            })])
        }
        _ => {
            readline_query(&mut input.query, &mut input.caret, key);
            Effects::none()
        }
    }
}

/// Whether the line containing byte offset `caret` in `text` is empty — the
/// blank-line-submits check for a multiline `FunctionInput`. Only the line
/// straddling `caret` matters (not the whole buffer), so editing mid-buffer
/// behaves the same way a cursor-aware text area would.
fn current_line_is_empty(text: &str, caret: usize) -> bool {
    let start = text[..caret].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let end = caret + text[caret..].find('\n').unwrap_or(text.len() - caret);
    start == end
}

/// Apply a raw key to the active text input. The function-panel search input takes
/// priority when open; otherwise it edits the Select query and re-filters.
fn raw_key(state: &mut AppState, key: Key) -> Effects {
    if state.function.search.input_active {
        readline_query(&mut state.function.search.query, &mut state.function.search.caret, key);
        return Effects::none();
    }
    // A picker without an input (the key list) has nothing to type into.
    if matches!(state.mode(), Mode::Select(spec) if matches!(spec.input, SelectInput::None)) {
        return Effects::none();
    }
    let Some(sel) = &mut state.select else {
        return Effects::none();
    };
    // A printable key in Navigate phase returns to Input.
    if sel.phase == SelectPhase::Navigate {
        if key.printable_char().is_some() {
            sel.phase = SelectPhase::Input;
        } else {
            return Effects::none();
        }
    }
    readline_query(&mut sel.query, &mut sel.caret, key);
    refilter(state)
}

/// Minimal readline editing on a (query, caret) pair, applied by the reducer.
/// Shared by the Select query and the function-panel search input.
fn readline_query(query: &mut String, caret: &mut usize, key: Key) {
    let ctrl = key.mods.ctrl;
    match key.code {
        KeyCode::Char(c) if ctrl => match c {
            'a' => *caret = 0,
            'e' => *caret = query.len(),
            'k' => query.truncate(*caret),
            'u' => {
                query.drain(..*caret);
                *caret = 0;
            }
            'w' => {
                let start = word_start(query, *caret);
                query.drain(start..*caret);
                *caret = start;
            }
            'b' => *caret = prev_boundary(query, *caret),
            'f' => *caret = next_boundary(query, *caret),
            'h' => backspace(query, caret),
            'd' => delete_forward(query, caret),
            _ => {}
        },
        KeyCode::Char(c) => {
            query.insert(*caret, c);
            *caret += c.len_utf8();
        }
        KeyCode::Backspace => backspace(query, caret),
        KeyCode::Delete => delete_forward(query, caret),
        KeyCode::Left => *caret = prev_boundary(query, *caret),
        KeyCode::Right => *caret = next_boundary(query, *caret),
        KeyCode::Home => *caret = 0,
        KeyCode::End => *caret = query.len(),
        _ => {}
    }
}

/// Decode `spec.initial` into runtime widget state per `spec.kind` (called
/// once, from `push_mode`) — the inverse of `encode_input_value`.
fn decode_input_state(kind: &InputKind, initial: &str) -> InputFormState {
    match kind {
        InputKind::Text => {
            InputFormState::Text { value: initial.to_string(), cursor: initial.chars().count() }
        }
        InputKind::Bool => InputFormState::Bool { value: initial == "#t" },
        InputKind::Choice(options) => {
            let selected = options.iter().position(|o| o == initial).unwrap_or(0);
            InputFormState::Choice { options: options.clone(), selected }
        }
        InputKind::Int { min, max } => {
            let value = initial.parse::<i64>().unwrap_or(*min).clamp(*min, *max);
            InputFormState::Int { value, min: *min, max: *max, typed: String::new() }
        }
        InputKind::List => {
            let items: Vec<String> =
                initial.split(',').map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect();
            // Start on the trailing "Confirm" row so a no-op Enter confirms
            // the unchanged list immediately, same as every other kind.
            let selected = list_confirm_row(&items);
            InputFormState::List { items, selected, editing: None }
        }
    }
}

/// Encode the current widget state into the `KEY_ITEM` wire string — the
/// inverse of `decode_input_state`, see doc/UI.md's Input Form Mode section
/// for the per-kind wire format.
fn encode_input_value(form: &InputFormState) -> String {
    match form {
        InputFormState::Text { value, .. } => value.clone(),
        InputFormState::Bool { value } => if *value { "#t" } else { "#f" }.to_string(),
        InputFormState::Choice { options, selected } => options[*selected].clone(),
        InputFormState::Int { value, .. } => value.to_string(),
        InputFormState::List { items, .. } => items.join(","),
    }
}

/// Apply a raw key to the active `Mode::InputForm` popup. Structured
/// navigation (movement, toggle, add/remove row) is handled by the shared
/// Cursor*/ToggleSelect/AddItem/RemoveItem intents instead (bound via
/// keymap.scm's "input-form" scope, gated off while typing — see
/// "input-form-typing" in `script::condition`); what's left here is exactly
/// the free-text paths keymap.scm can't reasonably enumerate: `Text`'s
/// tui-input caret, `Int`'s direct digit-typing (needed because some ranges
/// span millions, far too wide for arrow-only stepping), and a `List` row's
/// nested tui-input item edit.
fn input_form_key(state: &mut AppState, key: Key) -> Effects {
    let Some(form) = &mut state.input_form else { return Effects::none() };
    match form {
        InputFormState::Text { .. } => return input_form_text_key(state, key),
        InputFormState::Int { value, min, max, typed } => {
            match key.code {
                KeyCode::Char('-') if typed.is_empty() && *min < 0 => typed.push('-'),
                KeyCode::Char(c) if c.is_ascii_digit() => typed.push(c),
                KeyCode::Backspace if !typed.is_empty() => {
                    typed.pop();
                }
                _ => return Effects::none(),
            }
            // A lone `-` (or an emptied buffer) keeps the last value.
            if let Ok(v) = typed.parse::<i64>() {
                *value = v.clamp(*min, *max);
            }
        }
        InputFormState::List { editing: Some((_, buf, cursor)), .. } => {
            if let Some(req) = to_input_request(key) {
                let mut input = tui_input::Input::default().with_value(buf.clone()).with_cursor(*cursor);
                input.handle(req);
                *buf = input.value().to_string();
                *cursor = input.cursor();
            }
        }
        // Bool/Choice/Color, and List while browsing: fully Intent-driven.
        _ => {}
    }
    Effects::none()
}

/// The `List` kind's row layout: `items.len()` real rows, then a `"+ Add
/// item"` row, then a trailing `"* Confirm"` row. Enter's meaning depends on
/// which row `selected` is on (see `input_form_confirm`); a dedicated
/// trailing row is needed because `keymap.scm` binds Enter directly to
/// `Intent::Confirm` for every InputForm instance, so there is no way to give
/// Enter a row-local meaning *and* a way to actually confirm without one.
fn list_add_row(items: &[String]) -> usize {
    items.len()
}
fn list_confirm_row(items: &[String]) -> usize {
    items.len() + 1
}

/// The `Text` kind's key handling, via `tui_input`: a transient `Input` is
/// rebuilt from `state.input_form` (value + char-index cursor), the
/// translated request applied, and the result written back.
/// `core::reducer` stays terminal-agnostic — this uses `tui_input`'s
/// backend-independent `Input::handle(InputRequest)`, never `handle_event`
/// (which takes a raw crossterm event and needs an optional feature this
/// crate doesn't enable).
fn input_form_text_key(state: &mut AppState, key: Key) -> Effects {
    let Some(InputFormState::Text { value, cursor }) = &mut state.input_form else {
        return Effects::none();
    };
    let Some(req) = to_input_request(key) else { return Effects::none() };
    let mut input = tui_input::Input::default().with_value(value.clone()).with_cursor(*cursor);
    input.handle(req);
    *value = input.value().to_string();
    *cursor = input.cursor();
    Effects::none()
}

/// Translate this codebase's own `Key` into a `tui_input::InputRequest`.
/// Mirrors the same operations `readline_query` hand-rolls for Select/
/// function-panel input (Ctrl-a/e/k/w/b/f/h/d, Backspace/Delete/arrows/Home/
/// End); `Ctrl-u` maps to `DeleteLine` (tui-input has no "delete to start"
/// counterpart to `DeleteTillEnd`, so this clears the whole line instead of
/// just the part before the cursor — a minor, acceptable simplification).
fn to_input_request(key: Key) -> Option<tui_input::InputRequest> {
    use tui_input::InputRequest::*;
    if key.mods.ctrl {
        return match key.code {
            KeyCode::Char('a') => Some(GoToStart),
            KeyCode::Char('e') => Some(GoToEnd),
            KeyCode::Char('k') => Some(DeleteTillEnd),
            KeyCode::Char('u') => Some(DeleteLine),
            KeyCode::Char('w') => Some(DeletePrevWord),
            KeyCode::Char('b') => Some(GoToPrevChar),
            KeyCode::Char('f') => Some(GoToNextChar),
            KeyCode::Char('h') => Some(DeletePrevChar),
            KeyCode::Char('d') => Some(DeleteNextChar),
            _ => None,
        };
    }
    match key.code {
        KeyCode::Char(c) => Some(InsertChar(c)),
        KeyCode::Backspace => Some(DeletePrevChar),
        KeyCode::Delete => Some(DeleteNextChar),
        KeyCode::Left => Some(GoToPrevChar),
        KeyCode::Right => Some(GoToNextChar),
        KeyCode::Home => Some(GoToStart),
        KeyCode::End => Some(GoToEnd),
        _ => None,
    }
}

/// Confirm a `Mode::InputForm` popup: pop it, then re-inject `spec.template`
/// with the encoded value inserted under `KEY_ITEM` — the same contract as
/// `Select`'s `OnConfirm::Emit`, so one `*-apply` extension handler serves
/// both.
///
/// For the `List` kind specifically, Enter is contextual rather than always
/// confirming (see `list_add_row`/`list_confirm_row`): on a real item row it
/// starts (or commits) editing that item, on the "+ Add" row it starts a new
/// one, and only on the trailing "* Confirm" row does it fall through to the
/// normal pop-and-emit below.
fn input_form_confirm(state: &mut AppState) -> Effects {
    let Mode::InputForm(spec) = state.mode().clone() else { return Effects::none() };

    if let Some(InputFormState::List { items, selected, editing }) = &mut state.input_form {
        if let Some((idx, buf, _)) = editing.take() {
            let text = buf.trim().to_string();
            if idx == usize::MAX {
                if !text.is_empty() {
                    items.push(text);
                }
            } else if text.is_empty() {
                items.remove(idx);
            } else {
                items[idx] = text;
            }
            *selected = (*selected).min(list_confirm_row(items));
            return Effects::none();
        }
        let add_row = list_add_row(items);
        if *selected == add_row {
            *editing = Some((usize::MAX, String::new(), 0));
            return Effects::none();
        }
        if *selected < add_row {
            let text = items[*selected].clone();
            let cursor = text.chars().count();
            *editing = Some((*selected, text, cursor));
            return Effects::none();
        }
        // selected == list_confirm_row(items): fall through to confirm.
    }

    let Some(form) = state.input_form.take() else { return Effects::none() };
    state.pop_mode();
    let value = encode_input_value(&form);
    let data = spec.template.data.with(crate::core::KEY_ITEM, crate::core::ExtensionValue::String(value));
    Effects::emit(vec![Intent::Extension(crate::core::ExtensionIntent {
        extension: spec.template.extension,
        intent: spec.template.intent,
        data,
    })])
}

fn backspace(query: &mut String, caret: &mut usize) {
    if *caret > 0 {
        let prev = prev_boundary(query, *caret);
        query.drain(prev..*caret);
        *caret = prev;
    }
}

fn delete_forward(query: &mut String, caret: &mut usize) {
    if *caret < query.len() {
        let next = next_boundary(query, *caret);
        query.drain(*caret..next);
    }
}

fn prev_boundary(s: &str, i: usize) -> usize {
    s[..i].char_indices().last().map(|(idx, _)| idx).unwrap_or(0)
}

fn next_boundary(s: &str, i: usize) -> usize {
    s[i..].chars().next().map(|c| i + c.len_utf8()).unwrap_or(i)
}

fn word_start(s: &str, i: usize) -> usize {
    let bytes = &s[..i];
    let trimmed = bytes.trim_end_matches(|c: char| c.is_whitespace());
    match trimmed.rfind(|c: char| c.is_whitespace()) {
        Some(idx) => idx + 1,
        None => 0,
    }
}

/// Re-rank candidates against the current query. FileWalk (deep) goes async
/// via a Search plan; Entries (shallow), Static, and Commands rank locally
/// and synchronously — see doc/UI.md (Deep search vs. shallow filter).
fn refilter(state: &mut AppState) -> Effects {
    let Mode::Select(spec) = state.mode().clone() else {
        return Effects::none();
    };
    let query = state.select.as_ref().map(|s| s.query.clone()).unwrap_or_default();
    match &spec.source {
        SelectSource::FileWalk => Effects::plan(Plan::Search {
            root: state.cwd.clone(),
            query,
            show_hidden: state.show_hidden,
            owner: None,
        }),
        SelectSource::Entries(pool) => {
            let entries = filter_entries(pool.clone(), &query);
            if let Some(sel) = &mut state.select {
                sel.view = Some(crate::core::EntryList { entries, cursor: 0 });
            }
            Effects::none()
        }
        SelectSource::Static(pool) => {
            // Only a Fuzzy input filters the pool (the palette, the format picker).
            // With a Path input the query is free text (the command Select's
            // destination), so the candidates — the option checkboxes — stay
            // unfiltered; the result list is purely for selection, not search.
            let results = if matches!(spec.input, SelectInput::Fuzzy) {
                fuzzy_rank(pool.clone(), &query)
            } else {
                pool.clone()
            };
            apply_results(state, results);
            Effects::none()
        }
        SelectSource::Commands => Effects::none(),
    }
}

/// Rank a fixed `Entry` pool against `query` by subsequence score on
/// `Entry::name`, synchronously — the shallow-filter counterpart of
/// `fuzzy_rank` (which scores `Candidate::label` instead). An empty query
/// returns the pool unranked, same convention as `fuzzy_rank`.
fn filter_entries(mut pool: Vec<crate::core::state::Entry>, query: &str) -> Vec<crate::core::state::Entry> {
    if query.is_empty() {
        return pool;
    }
    let q: Vec<char> = query.to_lowercase().chars().collect();
    let mut scored: Vec<(i64, crate::core::state::Entry)> = pool
        .drain(..)
        .filter_map(|e| subseq_score(&e.name.to_lowercase(), &q).map(|s| (s, e)))
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.name.cmp(&b.1.name)));
    scored.into_iter().map(|(_, e)| e).collect()
}

fn apply_results(state: &mut AppState, results: Vec<crate::core::mode::Candidate>) {
    if let Some(sel) = &mut state.select {
        sel.results = results;
        if sel.selected >= sel.results.len() {
            sel.selected = sel.results.len().saturating_sub(1);
        }
    }
}

/// A tiny dependency-free subsequence fuzzy ranker for local candidate pools.
pub fn fuzzy_rank(
    mut pool: Vec<crate::core::mode::Candidate>,
    query: &str,
) -> Vec<crate::core::mode::Candidate> {
    if query.is_empty() {
        return pool;
    }
    let q: Vec<char> = query.to_lowercase().chars().collect();
    let mut scored: Vec<(i64, crate::core::mode::Candidate)> = pool
        .drain(..)
        .filter_map(|c| subseq_score(&c.label.to_lowercase(), &q).map(|s| (s, c)))
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.label.cmp(&b.1.label)));
    scored.into_iter().map(|(_, c)| c).collect()
}

fn subseq_score(hay: &str, needle: &[char]) -> Option<i64> {
    let hay: Vec<char> = hay.chars().collect();
    let mut hi = 0;
    let mut score = 0i64;
    let mut last_match: Option<usize> = None;
    for &nc in needle {
        let mut found = false;
        while hi < hay.len() {
            if hay[hi] == nc {
                // Reward adjacency and earlier matches.
                if let Some(l) = last_match
                    && hi == l + 1 {
                        score += 5;
                    }
                score += 10 - (hi as i64).min(9);
                last_match = Some(hi);
                hi += 1;
                found = true;
                break;
            }
            hi += 1;
        }
        if !found {
            return None;
        }
    }
    Some(score)
}

/// Consume an async result by (purpose, payload). Never branches per intent.
pub fn apply_result(state: &mut AppState, result: AsyncResult) -> Effects {
    if let AsyncStatus::Failed(msg) = &result.status {
        return Effects::notice(Notice::error(msg.clone()));
    }
    if result.status != AsyncStatus::Ok {
        return Effects::none(); // Cancelled / StaleDiscarded: silent.
    }
    match (result.purpose, result.payload) {
        (Purpose::Refresh, Payload::Entries { path, entries, trail }) => {
            if path == state.cwd {
                load_entries(state, entries);
                state.trail = trail;
            }
            Effects::none()
        }
        (Purpose::Search, Payload::Entries { entries, .. }) => {
            // The hits are filesystem entries: keep them as an entry list (no
            // flattening) so they render and navigate as the file panel does.
            if let Some(sel) = &mut state.select {
                let cursor = sel.view.as_ref().map(|v| v.cursor).unwrap_or(0);
                let mut view = crate::core::EntryList { entries, cursor };
                view.clamp();
                sel.view = Some(view);
            }
            Effects::none()
        }
        // Content completions (Purpose::Content) are handled by the kernel: it
        // hands the read result to the owning extension's accept_content and
        // resets the view. They do not flow through the reducer.
        (Purpose::Resolver, Payload::OpDone(done)) => {
            let mut fx = if done.success {
                read_dir_effect(state)
            } else {
                Effects::none()
            };
            let text = done.message.unwrap_or_else(|| {
                format!("{} {}", done.label, if done.success { "done" } else { "failed" })
            });
            fx.notices.push(if done.success {
                Notice::info(text)
            } else {
                Notice::error(text)
            });
            fx
        }
        (Purpose::Execute, Payload::Exec(out)) => {
            state.function.exec.lines = out.lines;
            state.function.exec.finished = out.finished;
            state.function.exec.exit = out.exit;
            state.function.visible = true;
            state.function.sublayout = SubLayout::Exec;
            Effects::none()
        }
        // The kernel has already applied the change set and routed the value to
        // its owner; an unowned success (a Configure) needs nothing more, and a
        // failure arrives as `Failed` above.
        (Purpose::Scheme, Payload::Scheme { .. }) => Effects::none(),
        _ => Effects::none(),
    }
}

/// Install a fresh listing, dropping vanished selection paths and fixing the
/// cursor: a pending focus wins, else keep the previous entry, else the nearest
/// surviving neighbor by position (clamp). Any real directory (re)load — a
/// fresh `cd`, a refresh, a bookmark jump — always lands here, so it is the
/// one place that drops a stale filter: the saved levels refer to a listing
/// this fresh read is about to replace anyway.
fn load_entries(state: &mut AppState, entries: Vec<crate::core::state::Entry>) {
    let prev_path = state.current_entry().map(|e| e.path.clone());
    let pending = state.pending_focus.take();
    state.entries = entries;
    state.filter_stack.clear();
    state.filter_query = None;
    state.selection.retain(|p| state.entries.iter().any(|e| &e.path == p));
    let focus = pending.or(prev_path);
    if let Some(p) = focus
        && let Some(i) = state.entries.iter().position(|e| e.path == p) {
            state.cursor = i;
        }
    state.clamp_cursor();
}

#[cfg(test)]
mod tests {
    use super::*;
    

    fn type_query(s: &mut AppState, q: &str) {
        for c in q.chars() {
            apply_intent(s, Intent::RawKeyEvent(Key::plain(KeyCode::Char(c))), &NoCommands);
        }
    }

    #[test]
    fn multiline_function_input_enter_adds_a_line_then_blank_enter_submits() {
        let mut s = AppState::new(std::env::temp_dir());
        s.function.form_owner = Some("cmdop".into());
        apply_intent(
            &mut s,
            Intent::BeginInput { purpose: "create-lines".into(), seed: String::new(), multiline: true },
            &NoCommands,
        );

        type_query(&mut s, "foo");
        assert_eq!(s.function.input.as_ref().unwrap().query, "foo");

        // Enter on a non-empty line inserts a newline — no submit yet.
        apply_intent(&mut s, Intent::RawKeyEvent(Key::plain(KeyCode::Enter)), &NoCommands);
        assert_eq!(s.function.input.as_ref().unwrap().query, "foo\n");

        type_query(&mut s, "bar");
        apply_intent(&mut s, Intent::RawKeyEvent(Key::plain(KeyCode::Enter)), &NoCommands);
        assert_eq!(s.function.input.as_ref().unwrap().query, "foo\nbar\n");

        // Enter on the now-blank line submits: input closes, hands the text
        // back to the owner as an extension intent.
        let fx = apply_intent(&mut s, Intent::RawKeyEvent(Key::plain(KeyCode::Enter)), &NoCommands);
        assert!(s.function.input.is_none(), "submit closes the input");
        match fx.intents.as_slice() {
            [Intent::Extension(ei)] => {
                assert_eq!(ei.extension, "cmdop");
                assert_eq!(ei.intent, "input-submit");
                assert_eq!(ei.data.get("text").and_then(|v| v.as_str()), Some("foo\nbar\n"));
            }
            other => panic!("expected a single input-submit Extension intent, got {other:?}"),
        }
    }

    #[test]
    fn clear_filter_pops_one_level_and_restores_the_prior_listing() {
        use crate::core::{EntryList, Mode, SelectSpec};
        let mut s = AppState::new(std::path::PathBuf::from("/d"));
        let original = file_entries(2);
        s.entries = original.clone();
        s.cursor = 1;

        s.push_mode(Mode::Select(SelectSpec::file_search()));
        let matches = file_entries(3);
        let mut sel = SelectState::new("abc".into(), true);
        sel.view = Some(EntryList { entries: matches.clone(), cursor: 0 });
        s.select = Some(sel);
        apply_intent(&mut s, Intent::Confirm, &NoCommands);
        assert_eq!(s.entries, matches);
        assert_eq!(s.filter_query.as_deref(), Some("abc"));

        apply_intent(&mut s, Intent::ClearFilter, &NoCommands);
        assert_eq!(s.entries, original, "clear-filter restores the exact pre-filter listing");
        assert_eq!(s.cursor, 1, "and the cursor position it had then");
        assert!(s.filter_query.is_none());
        assert!(s.filter_stack.is_empty());
    }

    // --- Shallow filter (FilterListing / SelectRefine) ---

    #[test]
    fn shallow_confirm_commits_via_filter_stack_like_file_search() {
        use crate::core::Mode;
        let mut s = AppState::new(std::path::PathBuf::from("/d"));
        let original = file_entries(2);
        s.entries = original.clone();
        s.cursor = 1;
        apply_intent(&mut s, Intent::FilterListing, &NoCommands);
        type_query(&mut s, "0");
        let fx = apply_intent(&mut s, Intent::Confirm, &NoCommands);
        assert!(fx.intents.is_empty());
        assert!(matches!(s.mode(), Mode::File), "commits straight into File mode, like deep file-search");
        assert!(s.select.is_none());
        assert_eq!(s.entries.len(), 1);
        assert_eq!(s.entries[0].name, "f0");
        assert_eq!(s.filter_stack.len(), 1, "the pre-filter listing is saved, same as deep file-search");
        assert_eq!(s.filter_stack[0].entries, original);
        assert_eq!(s.filter_query.as_deref(), Some("0"));

        apply_intent(&mut s, Intent::ClearFilter, &NoCommands);
        assert_eq!(s.entries, original, "Esc unwinds a shallow filter exactly like a deep one");
    }

    // --- Select refine (shallow-filter a result view) ---

    #[test]
    fn refine_freezes_deep_results_and_ranks_them_shallowly() {
        use crate::core::{EntryList, Mode, SelectSource, SelectSpec};
        let mut s = AppState::new(std::path::PathBuf::from("/d"));
        s.push_mode(Mode::Select(SelectSpec::file_search()));
        let deep_results = file_entries(3); // f0, f1, f2 — simulates a completed deep walk
        let mut sel = SelectState::new("anything".into(), true);
        sel.phase = SelectPhase::Navigate;
        sel.view = Some(EntryList { entries: deep_results.clone(), cursor: 1 });
        s.select = Some(sel);

        let fx = apply_intent(&mut s, Intent::SelectRefine, &NoCommands);
        assert!(fx.plans.is_empty(), "refine never spawns a plan — it filters what's already in hand");
        // The Select is replaced in place (push_mode's replacing_select path),
        // not stacked, and its source is now the frozen deep-result pool.
        match s.mode() {
            Mode::Select(spec) => {
                assert_eq!(spec.id, "filter-list");
                assert!(matches!(&spec.source, SelectSource::Entries(v) if v == &deep_results));
            }
            other => panic!("expected the Select replaced with filter-list, got {other:?}"),
        }
        // Fresh query state (empty), so refine starts from the full frozen set.
        assert_eq!(s.select.as_ref().unwrap().query, "");
        assert_eq!(s.select.as_ref().unwrap().view.as_ref().unwrap().entries, deep_results);

        // Typing ranks the frozen pool synchronously — not a fresh deep
        // walk (which would go through Plan::Search / FileWalk instead).
        let fx = apply_intent(&mut s, Intent::RawKeyEvent(Key::plain(KeyCode::Char('1'))), &NoCommands);
        assert!(fx.plans.is_empty(), "shallow refine ranking never spawns a plan either");
        let view = s.select.as_ref().unwrap().view.as_ref().unwrap();
        assert_eq!(view.entries.len(), 1);
        assert_eq!(view.entries[0].name, "f1");
    }

    #[test]
    fn input_form_int_clamps_at_min_and_max() {
        use crate::core::{ExtensionIntent, ExtensionValue, InputFormSpec, InputKind, Mode, KEY_ITEM};
        let template = ExtensionIntent { extension: "x".into(), intent: "y".into(), data: ExtensionValue::Nil };
        let mut s = AppState::new(std::env::temp_dir());
        let spec = InputFormSpec::emit_input_widget("i", template, InputKind::Int { min: 0, max: 5 }, "3");
        apply_intent(&mut s, Intent::PushMode(Box::new(Mode::InputForm(spec))), &NoCommands);

        for _ in 0..10 {
            apply_intent(&mut s, Intent::CursorRight, &NoCommands);
        }
        let fx = apply_intent(&mut s, Intent::Confirm, &NoCommands);
        match fx.intents.as_slice() {
            [Intent::Extension(ei)] => assert_eq!(ei.data.get(KEY_ITEM).and_then(|v| v.as_str()), Some("5")),
            other => panic!("expected Intent::Extension, got {other:?}"),
        }
    }


    // --- File-list layout cursor geometry ---------------------------------

    fn file_entries(n: usize) -> Vec<crate::core::Entry> {
        (0..n)
            .map(|i| crate::core::Entry {
                path: std::path::PathBuf::from(format!("/d/f{i}")),
                name: format!("f{i}"),
                is_dir: false,
                is_symlink: false,
                size: 0,
                mode: 0,
                uid: 0,
                gid: 0, owner: "".into(), group: "".into(),
                modified: None,
                created: None,
                accessed: None,
                symlink_target: None,
                activation: None,
            })
            .collect()
    }

    fn grid_state(n: usize, cols: usize, rows: usize) -> AppState {
        let mut s = AppState::new(std::path::PathBuf::from("/d"));
        s.entries = file_entries(n);
        s.list_layout = crate::core::ListLayout::Grid;
        s.list_geom = crate::core::ListGeom { cols, rows };
        s
    }

    fn mv(s: &mut AppState, intent: Intent) {
        apply_intent(s, intent, &NoCommands);
    }

    #[test]
    fn grid_wasd_moves_2d_column_major() {
        // n=7, cols=3, rows=3: col0=0,1,2  col1=3,4,5  col2=6
        let mut s = grid_state(7, 3, 3);
        s.cursor = 0;
        mv(&mut s, Intent::CursorDown); // within col0
        assert_eq!(s.cursor, 1);
        mv(&mut s, Intent::CursorRight); // col0 → col1, same row
        assert_eq!(s.cursor, 4);
        mv(&mut s, Intent::CursorRight); // col1 → col2 row1 does not exist → clamp
        assert_eq!(s.cursor, 4);
        mv(&mut s, Intent::CursorUp); // col1 row0
        assert_eq!(s.cursor, 3);
        mv(&mut s, Intent::CursorRight); // col1 → col2 row0 exists
        assert_eq!(s.cursor, 6);
        mv(&mut s, Intent::CursorUp); // top of col2 → clamp
        assert_eq!(s.cursor, 6);
    }

    #[test]
    fn left_edge_escapes_to_parent_all_layouts() {
        // Cursor in the leftmost column (col0) → CursorLeft goes to the parent.
        let mut s = grid_state(7, 3, 3);
        s.cursor = 1; // col0
        mv(&mut s, Intent::CursorLeft);
        assert_eq!(s.cwd, std::path::PathBuf::from("/"), "left wall escapes to parent");
        assert_eq!(s.pending_focus.as_deref(), Some(std::path::Path::new("/d")));
    }

    // --- Revision 2: d shows content, Enter opens, search focus ----------------

    #[test]
    fn a_function_intent_never_focuses_a_hidden_function_panel() {
        use crate::core::{PANEL_FILE, PANEL_FUNCTION};
        let mut s = AppState::new(std::path::PathBuf::from("/d"));
        s.entries = file_entries(3);
        s.focused_panel = PANEL_FILE.into();
        for i in [Intent::FuncDown, Intent::FuncLeft, Intent::ScrollUp, Intent::PageDown] {
            mv(&mut s, i);
            assert_eq!(s.focused_panel, PANEL_FILE, "the panel is hidden");
        }
        s.function.visible = true;
        mv(&mut s, Intent::FuncDown);
        assert_eq!(s.focused_panel, PANEL_FUNCTION, "shown, it takes focus as before");
    }

    #[test]
    fn quit_cd_quits_and_asks_for_the_directory_to_be_handed_back() {
        let mut s = AppState::new(std::path::PathBuf::from("/d"));
        mv(&mut s, Intent::Quit);
        assert!(s.quit && !s.cd_on_quit);
        let mut s = AppState::new(std::path::PathBuf::from("/d"));
        mv(&mut s, Intent::QuitCd);
        assert!(s.quit && s.cd_on_quit);
    }

    #[test]
    fn focus_follows_the_intent_target_panel() {
        use crate::core::{PANEL_FILE, PANEL_FUNCTION};
        let mut s = AppState::new(std::path::PathBuf::from("/d"));
        s.entries = file_entries(3);
        s.function.visible = true;
        s.function.content_owner = Some("x".into());

        // A function-panel intent (hjkl → Func* / scroll) focuses the function panel.
        s.focused_panel = PANEL_FILE.into();
        mv(&mut s, Intent::FuncDown);
        assert_eq!(s.focused_panel, PANEL_FUNCTION, "Func* focuses the function panel");
        mv(&mut s, Intent::ScrollUp);
        assert_eq!(s.focused_panel, PANEL_FUNCTION, "scroll keeps function focus");

        // A file-panel intent (wasd → Cursor*) returns focus to the file panel,
        // even while the function panel is focused.
        mv(&mut s, Intent::CursorDown);
        assert_eq!(s.focused_panel, PANEL_FILE, "Cursor* focuses the file panel");

        // Intents that don't act on a middle panel leave focus unchanged.
        s.focused_panel = PANEL_FUNCTION.into();
        mv(&mut s, Intent::Refresh);
        assert_eq!(s.focused_panel, PANEL_FUNCTION, "neutral intents don't move focus");
    }

    #[test]
    fn enter_on_file_opens_via_resolver() {
        // Activate (Enter) on a file emits Open (cursor entry), not the content view.
        let mut s = AppState::new(std::path::PathBuf::from("/d"));
        s.entries = file_entries(2);
        s.cursor = 0;
        let fx = apply_intent(&mut s, Intent::Activate, &NoCommands);
        assert!(!s.function.visible, "Enter on a file does not show content");
        match fx.intents.as_slice() {
            [Intent::Open { path }] => assert!(path.as_os_str().is_empty(), "opens the cursor entry"),
            other => panic!("expected an Open intent, got {other:?}"),
        }
    }

    // --- VFS virtual navigation -------------------------------------------

    // --- command flow: target + options + run ----------------------------

    #[test]
    fn delete_selected_emits_a_destructive_run_resolver() {
        let mut s = AppState::new(std::path::PathBuf::from("/d"));
        s.entries = file_entries(2);
        s.cursor = 0;
        let fx = apply_intent(&mut s, Intent::DeleteSelected, &NoCommands);
        match fx.intents.as_slice() {
            [Intent::RunResolver(r)] => {
                assert_eq!(r.op, "delete");
                assert_eq!(r.paths.len(), 1, "cursor entry is the target");
            }
            other => panic!("expected RunResolver(delete), got {other:?}"),
        }
    }

}
