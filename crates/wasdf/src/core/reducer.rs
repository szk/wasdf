//! The reducer: pure, synchronous state transitions. It mutates AppState and
//! returns Effects describing the IO the kernel must perform (plans to spawn,
//! notices to show). It never performs IO itself. Async completions are
//! consumed by the pair (purpose, payload); handling never branches per intent.

use std::path::PathBuf;

use crate::core::event::{AsyncResult, AsyncStatus, Payload, Plan, Purpose, ResolverRequest};
use crate::core::intent::{Intent, Key, KeyCode};
use crate::core::mode::{Mode, SelectInput, SelectSource, SubLayout};
use crate::core::state::{AppState, Ratio, SelectPhase, SelectState};

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
            Plan::Search { .. } => Purpose::Search,
            Plan::Read { .. } => Purpose::Content,
            Plan::ResolveAndRun { .. } => Purpose::Resolver,
            Plan::Execute { .. } => Purpose::Execute,
            Plan::Suspend { .. } => Purpose::Execute,
            Plan::EvalScheme { .. } => Purpose::Scheme,
        }
    }
}

/// Apply an intent to the state, returning the effects to perform.
pub fn apply_intent(state: &mut AppState, intent: Intent, ctx: &dyn CommandLookup) -> Effects {
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

        // --- Function cursor (operate the panel from the file panel) ----
        Intent::FuncUp => {
            state.function.scroll = state.function.scroll.saturating_sub(1);
            Effects::none()
        }
        Intent::FuncDown => {
            state.function.scroll += 1;
            Effects::none()
        }
        Intent::FuncLeft => {
            state.function.hscroll = state.function.hscroll.saturating_sub(HSCROLL_STEP);
            Effects::none()
        }
        Intent::FuncRight => {
            state.function.hscroll += HSCROLL_STEP;
            Effects::none()
        }

        // --- Selection --------------------------------------------------
        Intent::ToggleSelect => {
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
                } else if let Some(sel) = &mut state.select {
                    if let Some(c) = sel.results.get(sel.selected) {
                        let label = c.label.clone();
                        if !sel.marks.remove(&label) {
                            sel.marks.insert(label);
                        }
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
        Intent::StartCopy => start_transfer(state, ctx, false),
        Intent::StartMove => start_transfer(state, ctx, true),
        Intent::StartRename => {
            let targets = state.targets();
            if targets.len() != 1 {
                return Effects::notice(Notice::error("rename needs exactly one target"));
            }
            let src = targets.into_iter().next().unwrap();
            let name = src.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            let template = op_request("rename", Vec::new(), Some(src));
            push_mode(
                state,
                Mode::Select(crate::core::SelectSpec::command(
                    template,
                    crate::core::ResolveFill::Dst,
                    option_candidates(ctx, "rename"),
                    Some(name),
                )),
                ctx,
            )
        }
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
            state.pop_mode();
            if was_select && !matches!(state.mode(), Mode::Select(_)) {
                state.select = None;
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
            state.focused_panel = "file".into();
            Effects::none()
        }
        Intent::ShowFunctionContent { owner, content } => {
            // Generic: store the extension's content, show + focus the panel, and
            // reset the transient view state (scroll positions, any search).
            state.function.visible = true;
            state.function.sublayout = SubLayout::Content;
            state.function.scroll = 0;
            state.function.hscroll = 0;
            state.function.search = crate::core::PanelSearch::default();
            state.function.content = Some(content);
            state.function.content_owner = Some(owner);
            state.focused_panel = "function".into();
            Effects::none()
        }
        Intent::UpdateFunctionView(value) => {
            // Generic: store the owning extension's opaque view state verbatim.
            state.function.ext_view = value;
            Effects::none()
        }
        Intent::LoadContent { owner, path, offset } => {
            // Generic cursor-follow read: remember the owner and issue the chunk read.
            state.function.content_owner = Some(owner.clone());
            Effects::plan(Plan::Read { owner, path, offset })
        }

        // --- Scroll -----------------------------------------------------
        Intent::ScrollUp => {
            scroll_target(state, |s| *s = s.saturating_sub(1));
            Effects::none()
        }
        Intent::ScrollDown => {
            scroll_target(state, |s| *s += 1);
            Effects::none()
        }
        Intent::ScrollTop => {
            scroll_target(state, |s| *s = 0);
            Effects::none()
        }
        Intent::ScrollBottom => {
            scroll_target(state, |s| *s = usize::MAX / 2);
            Effects::none()
        }
        Intent::PageUp => {
            scroll_target(state, |s| *s = s.saturating_sub(10));
            Effects::none()
        }
        Intent::PageDown => {
            scroll_target(state, |s| *s += 10);
            Effects::none()
        }
        Intent::SetScroll(n) => {
            state.function.scroll = n;
            Effects::none()
        }

        // --- Content less-style search ----------------------------------
        Intent::FunctionSearchStart => {
            let s = &mut state.function.search;
            s.input_active = true;
            s.query.clear();
            s.caret = 0;
            s.matches.clear();
            s.current = 0;
            // Borrow focus to the function panel while typing the query, so Enter
            // resolves to submit-search (function scope) rather than open-file
            // (file scope). Submit/Cancel release it (R2.2).
            state.focused_panel = "function".into();
            Effects::none()
        }
        // Submit hands the query to the owning extension, which matches over its
        // own content and returns the matches (+ a jump) as generic intents.
        Intent::FunctionSearchSubmit => {
            state.function.search.input_active = false;
            // Release focus to the file panel: jump mode is operated from the
            // file list (n/p step matches), w/a/s/d resume browsing (R2.2).
            state.focused_panel = "file".into();
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
            state.focused_panel = "file".into();
            Effects::none()
        }
        Intent::ResetContentView => {
            state.function.scroll = 0;
            state.function.hscroll = 0;
            state.function.search = crate::core::PanelSearch::default();
            Effects::none()
        }

        // --- Select editing ---------------------------------------------
        Intent::RawKeyEvent(key) => raw_key(state, key),

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
        Intent::Noop => Effects::none(),

        // Confirm/Cancel: Policy is handled by dispatch; Select is handled here
        // (phase switching and confirm-shape construction live in the reducer).
        Intent::Confirm => select_confirm(state, ctx),
        Intent::Cancel => {
            if matches!(state.mode(), Mode::Select(_)) {
                state.pop_mode();
                state.select = None;
            }
            Effects::none()
        }

        // --- Extension --------------------------------------------------
        Intent::UpdateExtensionState(value) => {
            if let Some(frame) = state.modes.last_mut() {
                if let Mode::Extension { state: s, .. } = &mut frame.mode {
                    *s = value;
                }
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
        Dir::Down => ((idx + 1) % rows != 0 && idx + 1 < n).then(|| idx + 1),
        Dir::Up => (idx % rows != 0).then(|| idx - 1),
        Dir::Left => (idx >= rows).then(|| idx - rows),
        Dir::Right => (idx + rows < n).then(|| idx + rows),
    }
}

/// Move the focused entry cursor. In a Select it drives the results (below); in
/// File mode it drives the file panel, whose grid walls carry directory
/// semantics: left at the wall escapes to the parent, right at the Rows wall acts
/// on the entry (R2.1) — a directory is entered, a file cycles the content panel.
fn cursor_move(state: &mut AppState, dir: Dir) -> Effects {
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

/// Jump to the top/bottom of the focused entry list (Select results or file panel).
fn cursor_jump(state: &mut AppState, bottom: bool) -> Effects {
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
    } else {
        match dir {
            Dir::Down if !sel.results.is_empty() => {
                sel.selected = (sel.selected + 1).min(sel.results.len() - 1);
            }
            Dir::Up => sel.selected = sel.selected.saturating_sub(1),
            _ => {}
        }
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
fn parent_dir(state: &mut AppState) -> Effects {
    if let Some(parent) = state.cwd.parent().map(PathBuf::from) {
        state.pending_focus = Some(state.cwd.clone());
        enter_directory(state, parent)
    } else {
        Effects::none()
    }
}

/// Activate the cursor entry (Enter): enter the directory, or **open the file**
/// via the resolver (R2.3). Showing content is no longer an Enter behavior — that is `d`
/// (cycle_content) / `,`. "d = look, Enter = do."
fn activate(state: &mut AppState) -> Effects {
    let Some(entry) = state.current_entry() else {
        return Effects::none();
    };
    if entry.is_dir {
        enter_dir(state)
    } else {
        // Reuse the resolver Open path (empty path = cursor entry).
        Effects::emit(vec![Intent::Open { path: PathBuf::new() }])
    }
}

fn navigate_to(state: &mut AppState, path: PathBuf) -> Effects {
    // A directory: enter it. A file: enter its parent and focus the file.
    let (dir, focus) = if path.is_dir() {
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

fn push_mode(state: &mut AppState, mode: Mode, ctx: &dyn CommandLookup) -> Effects {
    // Fill the command palette's candidates from the live registry at push time.
    let mut mode = match mode {
        Mode::Select(spec)
            if spec.id == "command-palette"
                && matches!(&spec.source, SelectSource::Static(c) if c.is_empty()) =>
        {
            Mode::Select(crate::core::SelectSpec::command_palette(ctx.command_candidates()))
        }
        other => other,
    };
    // Fill the command Select's command-line skeleton from the resolver chain, so
    // the Command panel shows the actual external command (e.g. `mv …`, `cp -R …`).
    if let Mode::Select(spec) = &mut mode {
        if spec.id == "command" && spec.command_line.is_empty() {
            if let crate::core::OnConfirm::Resolve { template, .. } = &spec.on_confirm {
                spec.command_line = ctx.resolver_command(&template.op);
            }
        }
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

/// Start a copy (move=false) or move (move=true): push the single command Select
/// — the input is the destination, the candidate list is the option checkboxes.
fn start_transfer(state: &mut AppState, ctx: &dyn CommandLookup, is_move: bool) -> Effects {
    let targets = state.targets();
    if targets.is_empty() {
        return Effects::notice(Notice::error("nothing selected"));
    }
    let op = if is_move { "move" } else { "copy" };
    let spec = crate::core::SelectSpec::command(
        op_request(op, targets, None),
        crate::core::ResolveFill::Dst,
        option_candidates(ctx, op),
        None,
    );
    push_mode(state, Mode::Select(spec), ctx)
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

    // Enter in the Input phase switches to Navigate, uniformly.
    if has_input && sel.phase == SelectPhase::Input {
        if let Some(s) = &mut state.select {
            s.phase = SelectPhase::Navigate;
        }
        return Effects::none();
    }

    // Build the confirm shape.
    let shape = match spec.input {
        SelectInput::Path => ConfirmShape::InputOnly(sel.query.clone()),
        _ if sel.view.is_some() => {
            // Path results (file-search): confirm the current entry as a path.
            match sel.view.as_ref().and_then(|v| v.current()) {
                Some(e) => ConfirmShape::Single(crate::core::Candidate::path(e.path.clone())),
                None => return Effects::none(),
            }
        }
        _ => {
            // Marks (Space → Many) are valid only for Emit, never for Path input.
            // The command Select reads its marks (option toggles) directly below.
            let marks_allowed = matches!(spec.on_confirm, OnConfirm::Emit(_));
            if marks_allowed && !sel.marks.is_empty() {
                let items = sel
                    .results
                    .iter()
                    .filter(|c| sel.marks.contains(&c.label))
                    .cloned()
                    .collect();
                ConfirmShape::Many(items)
            } else if let Some(c) = sel.results.get(sel.selected) {
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
        OnConfirm::Navigate => match &shape {
            ConfirmShape::Single(c) => c
                .value
                .as_path()
                .map(|p| vec![Intent::NavigateTo(p.clone())])
                .unwrap_or_default(),
            _ => Vec::new(),
        },
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
        OnConfirm::Emit(template) => {
            let data = template.data.with(crate::core::KEY_ITEM, shape.to_value());
            vec![Intent::Extension(crate::core::ExtensionIntent {
                extension: template.extension,
                intent: template.intent,
                data,
            })]
        }
    };
    Effects::emit(follow)
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
                f.visible = false;
                state.focused_panel = "file".into();
            }
        }
    }
}

/// Scroll the function panel (shared offset for both the content and exec
/// frames; the content renderer clamps it to its own length).
fn scroll_target(state: &mut AppState, f: impl FnOnce(&mut usize)) {
    f(&mut state.function.scroll);
}

/// Columns the text/hex content scrolls horizontally per FuncLeft/FuncRight.
const HSCROLL_STEP: usize = 8;

/// Move to the next/previous match (wrapping) and bring its line into view.
fn search_step(state: &mut AppState, forward: bool) {
    let n = state.function.search.matches.len();
    if n == 0 {
        return;
    }
    let cur = &mut state.function.search.current;
    *cur = if forward { (*cur + 1) % n } else { (*cur + n - 1) % n };
    let line = state.function.search.matches[state.function.search.current].0;
    state.function.scroll = line;
}


fn focus_cycle(state: &mut AppState, forward: bool) {
    // The two focusable panels in File mode: file and (when visible) function.
    let panels: Vec<&str> = if state.function.visible {
        vec!["file", "function"]
    } else {
        vec!["file"]
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
    if state.function.visible && state.function.sublayout == SubLayout::Content {
        if let (Some(owner), Some(entry)) =
            (state.function.content_owner.clone(), state.current_entry())
        {
            fx.intents.push(Intent::LoadContent {
                owner,
                path: entry.path.clone(),
                offset: 0,
            });
        }
    }
    fx
}

fn resolve_plan(request: ResolverRequest) -> Effects {
    Effects::plan(Plan::ResolveAndRun { request })
}

/// Apply a raw key to the active text input. The function-panel search input takes
/// priority when open; otherwise it edits the Select query and re-filters.
fn raw_key(state: &mut AppState, key: Key) -> Effects {
    if state.function.search.input_active {
        readline_query(&mut state.function.search.query, &mut state.function.search.caret, key);
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

/// Re-rank candidates against the current query. FileWalk goes async via a
/// Search plan; Static/Commands rank locally and synchronously.
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
        }),
        SelectSource::PathCompletion => {
            // Complete against the entries of the directory in the typed path.
            let candidates = path_completions(&state.cwd, &query);
            apply_results(state, candidates);
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

fn apply_results(state: &mut AppState, results: Vec<crate::core::mode::Candidate>) {
    if let Some(sel) = &mut state.select {
        sel.results = results;
        if sel.selected >= sel.results.len() {
            sel.selected = sel.results.len().saturating_sub(1);
        }
    }
}

fn path_completions(cwd: &std::path::Path, query: &str) -> Vec<crate::core::mode::Candidate> {
    use crate::core::mode::Candidate;
    let base = if query.is_empty() {
        cwd.to_path_buf()
    } else {
        let p = PathBuf::from(query);
        let abs = if p.is_absolute() { p } else { cwd.join(p) };
        if query.ends_with('/') { abs } else { abs.parent().map(PathBuf::from).unwrap_or(abs) }
    };
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&base) {
        for e in rd.flatten() {
            out.push(Candidate::path(e.path()));
        }
    }
    out.sort_by(|a, b| a.label.cmp(&b.label));
    out
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
                if let Some(l) = last_match {
                    if hi == l + 1 {
                        score += 5;
                    }
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
        (Purpose::Refresh, Payload::Entries { path, entries }) => {
            if path == state.cwd {
                load_entries(state, entries);
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
        (Purpose::Scheme, Payload::Scheme(value)) => {
            Effects::notice(Notice::info(format!("scheme: {}", render_value(&value))))
        }
        _ => Effects::none(),
    }
}

/// Render a Scheme result value for a notice.
fn render_value(v: &crate::core::ExtensionValue) -> String {
    use crate::core::ExtensionValue::*;
    match v {
        Nil => "()".into(),
        Bool(b) => if *b { "#t".into() } else { "#f".into() },
        Int(i) => i.to_string(),
        String(s) => s.clone(),
        Path(p) => p.display().to_string(),
        List(items) => format!("({} items)", items.len()),
        Map(m) => format!("{{{} keys}}", m.len()),
    }
}

/// Install a fresh listing, dropping vanished selection paths and fixing the
/// cursor: a pending focus wins, else keep the previous entry, else the nearest
/// surviving neighbor by position (clamp).
fn load_entries(state: &mut AppState, entries: Vec<crate::core::state::Entry>) {
    let prev_path = state.current_entry().map(|e| e.path.clone());
    let pending = state.pending_focus.take();
    state.entries = entries;
    state.selection.retain(|p| state.entries.iter().any(|e| &e.path == p));
    let focus = pending.or(prev_path);
    if let Some(p) = focus {
        if let Some(i) = state.entries.iter().position(|e| e.path == p) {
            state.cursor = i;
        }
    }
    state.clamp_cursor();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::ExtensionValue;

    fn type_query(s: &mut AppState, q: &str) {
        for c in q.chars() {
            apply_intent(s, Intent::RawKeyEvent(Key::plain(KeyCode::Char(c))), &NoCommands);
        }
    }

    #[test]
    fn func_left_right_scroll_horizontally() {
        let mut s = AppState::new(std::env::temp_dir());
        s.function.visible = true;
        apply_intent(&mut s, Intent::FuncRight, &NoCommands);
        assert_eq!(s.function.hscroll, HSCROLL_STEP);
        apply_intent(&mut s, Intent::FuncLeft, &NoCommands);
        assert_eq!(s.function.hscroll, 0);
        apply_intent(&mut s, Intent::FuncLeft, &NoCommands);
        assert_eq!(s.function.hscroll, 0, "saturates at zero");
    }

    #[test]
    fn file_search_results_navigate_as_an_entry_grid() {
        use crate::core::{EntryList, Mode, SelectPhase, SelectSpec};
        let mut s = AppState::new(std::path::PathBuf::from("/d"));
        s.push_mode(Mode::Select(SelectSpec::file_search()));
        let entries = file_entries(3);
        let mut sel = SelectState::new(String::new(), true);
        sel.phase = SelectPhase::Navigate; // past the input phase
        sel.view = Some(EntryList { entries: entries.clone(), cursor: 0 });
        s.select = Some(sel);

        // The shared Cursor* intents drive the view (no NextResult/PrevResult).
        apply_intent(&mut s, Intent::CursorDown, &NoCommands);
        assert_eq!(s.select.as_ref().unwrap().view.as_ref().unwrap().cursor, 1);
        apply_intent(&mut s, Intent::CursorBottom, &NoCommands);
        assert_eq!(s.select.as_ref().unwrap().view.as_ref().unwrap().cursor, 2);
        apply_intent(&mut s, Intent::CursorUp, &NoCommands);
        assert_eq!(s.select.as_ref().unwrap().view.as_ref().unwrap().cursor, 1);

        // Confirm navigates to the current entry's path (file-search = Navigate).
        let fx = apply_intent(&mut s, Intent::Confirm, &NoCommands);
        match fx.intents.as_slice() {
            [Intent::NavigateTo(p)] => assert_eq!(p, &entries[1].path),
            other => panic!("expected NavigateTo(current entry), got {other:?}"),
        }
    }

    #[test]
    fn search_input_routes_through_raw_key() {
        // The `/` input editing is core (generic readline); the matching is the
        // extension's. Here we only verify the query buffer editing.
        let mut s = AppState::new(std::env::temp_dir());
        apply_intent(&mut s, Intent::FunctionSearchStart, &NoCommands);
        assert!(s.function.search.input_active);
        type_query(&mut s, "foo");
        assert_eq!(s.function.search.query, "foo");
        apply_intent(&mut s, Intent::RawKeyEvent(Key::plain(KeyCode::Backspace)), &NoCommands);
        assert_eq!(s.function.search.query, "fo");
    }

    #[test]
    fn search_submit_delegates_to_the_owning_extension() {
        // Submit closes the input and emits an extension intent carrying the
        // query; the owning extension matches (core never calls the matcher).
        let mut s = AppState::new(std::env::temp_dir());
        s.function.content_owner = Some("x".into());
        apply_intent(&mut s, Intent::FunctionSearchStart, &NoCommands);
        type_query(&mut s, "foo");
        let fx = apply_intent(&mut s, Intent::FunctionSearchSubmit, &NoCommands);
        assert!(!s.function.search.input_active);
        match fx.intents.as_slice() {
            [Intent::Extension(ei)] => {
                assert_eq!((ei.extension.as_str(), ei.intent.as_str()), ("x", "search"));
                assert_eq!(ei.data.as_str(), Some("foo"));
            }
            other => panic!("expected an (ext x search) intent, got {other:?}"),
        }
    }

    #[test]
    fn store_matches_then_navigate() {
        // The extension's matches are stored (SetSearchMatches); n/p navigate the
        // stored list (core, reading AppState) and jump the scroll.
        let mut s = AppState::new(std::env::temp_dir());
        apply_intent(&mut s, Intent::SetSearchMatches(vec![(0, 0, 1), (2, 0, 1), (5, 0, 1)]), &NoCommands);
        assert_eq!(s.function.search.matches.len(), 3);
        assert_eq!(s.function.search.current, 0);
        apply_intent(&mut s, Intent::FunctionSearchNext, &NoCommands);
        assert_eq!((s.function.search.current, s.function.scroll), (1, 2));
        apply_intent(&mut s, Intent::FunctionSearchPrev, &NoCommands);
        apply_intent(&mut s, Intent::FunctionSearchPrev, &NoCommands);
        assert_eq!((s.function.search.current, s.function.scroll), (2, 5), "prev wraps");
        apply_intent(&mut s, Intent::SetScroll(9), &NoCommands);
        assert_eq!(s.function.scroll, 9);
    }

    #[test]
    fn reset_content_view_clears_scroll_and_search() {
        let mut s = AppState::new(std::env::temp_dir());
        s.function.scroll = 5;
        s.function.hscroll = 3;
        s.function.search.matches = vec![(0, 0, 1)];
        apply_intent(&mut s, Intent::ResetContentView, &NoCommands);
        assert_eq!((s.function.scroll, s.function.hscroll), (0, 0));
        assert!(s.function.search.matches.is_empty());
    }

    #[test]
    fn refresh_reloads_visible_function_content() {
        // Refresh re-reads the directory AND, when the function panel shows
        // content, forces a LoadContent at offset 0 for the cursor path — so an
        // edit made in a suspended editor is re-read on return (the previewer
        // would otherwise skip a same-path re-read). With the panel hidden, only
        // the directory is re-read.
        let mut s = AppState::new(std::path::PathBuf::from("/d"));
        s.entries = file_entries(2);
        s.cursor = 1;
        s.function.visible = true;
        s.function.sublayout = SubLayout::Content;
        s.function.content_owner = Some("preview".into());
        let fx = apply_intent(&mut s, Intent::Refresh, &NoCommands);
        assert!(matches!(fx.plans.as_slice(), [Plan::ReadDir { .. }]), "re-reads the directory");
        match fx.intents.as_slice() {
            [Intent::LoadContent { owner, path, offset }] => {
                assert_eq!(owner, "preview");
                assert_eq!(path, &s.entries[1].path);
                assert_eq!(*offset, 0, "forced reload from the start bypasses the owner cache");
            }
            other => panic!("expected a forced LoadContent, got {other:?}"),
        }

        // Hidden panel: no content reload.
        s.function.visible = false;
        let fx = apply_intent(&mut s, Intent::Refresh, &NoCommands);
        assert!(fx.intents.is_empty(), "no content reload when the panel is hidden");
    }

    #[test]
    fn show_function_content_populates_and_focuses_the_panel() {
        use crate::core::PanelContent;
        let mut s = AppState::new(std::env::temp_dir());
        let content = PanelContent::Lines { lines: vec!["hi".into()], styles: vec![Vec::new()] };
        apply_intent(
            &mut s,
            Intent::ShowFunctionContent { owner: "ext".into(), content },
            &NoCommands,
        );
        assert!(s.function.visible);
        assert_eq!(s.function.sublayout, SubLayout::Content);
        assert_eq!(s.function.content_owner.as_deref(), Some("ext"));
        assert_eq!(s.focused_panel, "function");
        assert!(s.function.content.is_some());
        apply_intent(&mut s, Intent::ScrollDown, &NoCommands);
        assert_eq!(s.function.scroll, 1);
    }

    #[test]
    fn update_function_view_stores_opaque_state() {
        let mut s = AppState::new(std::env::temp_dir());
        apply_intent(&mut s, Intent::UpdateFunctionView(ExtensionValue::Int(7)), &NoCommands);
        assert_eq!(s.function.ext_view, ExtensionValue::Int(7));
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
                gid: 0,
                modified: None,
                created: None,
                accessed: None,
                symlink_target: None,
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
    fn list_layout_cycles_rows_columns_grid() {
        use crate::core::ListLayout::*;
        assert_eq!(Rows.next(), Columns);
        assert_eq!(Columns.next(), Grid);
        assert_eq!(Grid.next(), Rows);
        let mut s = AppState::new(std::env::temp_dir());
        mv(&mut s, Intent::CycleListLayout);
        assert_eq!(s.list_layout, Columns);
    }

    #[test]
    fn rows_fallback_wasd_clamps() {
        // Unmeasured geometry (rows == 0) falls back to single-column behavior:
        // w/s move ∓1 and clamp at the first/last entry. (d/Enter on a file are
        // covered by the Revision 2 tests below.)
        let mut s = AppState::new(std::path::PathBuf::from("/d"));
        s.entries = file_entries(3);
        mv(&mut s, Intent::CursorDown);
        assert_eq!(s.cursor, 1);
        mv(&mut s, Intent::CursorDown);
        mv(&mut s, Intent::CursorDown);
        assert_eq!(s.cursor, 2, "clamps at the last entry");
        mv(&mut s, Intent::CursorUp);
        assert_eq!(s.cursor, 1);
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

    #[test]
    fn grid_right_wall_clamps() {
        // Rightmost existing cell: CursorRight clamps (no move, no side effect) in Grid.
        let mut s = grid_state(7, 3, 3);
        s.cursor = 6; // col2 (rightmost)
        mv(&mut s, Intent::CursorRight);
        assert_eq!(s.cursor, 6, "grid right wall clamps");
        assert!(!s.function.visible, "grid right wall does nothing");
    }

    // --- Revision 2: d shows content, Enter opens, search focus ----------------

    #[test]
    fn rows_d_on_file_cycles_content_without_focus() {
        // Rows + file: d shows the content (visible, Content), focus stays on file,
        // then cycles the ratio without ever hiding.
        let mut s = AppState::new(std::path::PathBuf::from("/d"));
        s.entries = file_entries(3);
        s.cursor = 1; // a file
        assert!(!s.function.visible);
        mv(&mut s, Intent::CursorRight); // d: show content
        assert!(s.function.visible);
        assert_eq!(s.function.sublayout, SubLayout::Content);
        assert_eq!(s.focused_panel, "file", "content cycle does not steal focus");
        let r0 = s.function.ratio;
        mv(&mut s, Intent::CursorRight); // d: advance ratio
        assert_ne!(s.function.ratio, r0, "second d advances the ratio");
        // Cycle the full ring; never hides.
        mv(&mut s, Intent::CursorRight);
        mv(&mut s, Intent::CursorRight);
        assert!(s.function.visible, "d never hides the content");
    }

    #[test]
    fn cycle_content_restores_content_from_exec() {
        let mut s = AppState::new(std::path::PathBuf::from("/d"));
        s.entries = file_entries(2);
        s.cursor = 0;
        s.function.visible = true;
        s.function.sublayout = SubLayout::Exec;
        mv(&mut s, Intent::CursorRight); // d on a file
        assert_eq!(s.function.sublayout, SubLayout::Content, "d forces Content back");
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

    #[test]
    fn enter_on_dir_enters_it() {
        let mut s = AppState::new(std::path::PathBuf::from("/d"));
        let mut es = file_entries(1);
        es[0].is_dir = true;
        es[0].path = std::path::PathBuf::from("/d/sub");
        s.entries = es;
        s.cursor = 0;
        apply_intent(&mut s, Intent::Activate, &NoCommands);
        assert_eq!(s.cwd, std::path::PathBuf::from("/d/sub"), "Enter on a dir enters it");
    }

    #[test]
    fn search_start_focuses_function_submit_and_cancel_release() {
        let mut s = AppState::new(std::path::PathBuf::from("/d"));
        s.function.content_owner = Some("x".into());
        mv(&mut s, Intent::FunctionSearchStart);
        assert_eq!(s.focused_panel, "function", "/ borrows focus for query input");
        assert!(s.function.search.input_active);
        mv(&mut s, Intent::FunctionSearchSubmit);
        assert_eq!(s.focused_panel, "file", "Enter releases focus to the file list");
        // Cancel also releases.
        mv(&mut s, Intent::FunctionSearchStart);
        assert_eq!(s.focused_panel, "function");
        mv(&mut s, Intent::FunctionSearchCancel);
        assert_eq!(s.focused_panel, "file");
    }

    // --- cmdops: target + options + run ----------------------------------

    /// A lookup that declares two options for `copy`, none otherwise.
    struct OptCtx;
    impl CommandLookup for OptCtx {
        fn intent_of(&self, _n: &str) -> Option<Intent> {
            None
        }
        fn command_candidates(&self) -> Vec<crate::core::Candidate> {
            Vec::new()
        }
        fn resolver_options(&self, op: &str) -> Vec<(String, String)> {
            if op == "copy" {
                vec![("-v".into(), "verbose".into()), ("-n".into(), "no-clobber".into())]
            } else {
                Vec::new()
            }
        }
        fn resolver_command(&self, op: &str) -> Vec<crate::core::CmdToken> {
            use crate::core::CmdToken::{Dst, Lit, Opts, Paths};
            match op {
                "copy" => vec![Lit("cp".into()), Lit("-R".into()), Opts, Paths, Dst],
                "move" => vec![Lit("mv".into()), Opts, Paths, Dst],
                _ => Vec::new(),
            }
        }
    }

    #[test]
    fn command_select_carries_the_real_executable_skeleton() {
        use crate::core::CmdToken;
        let mut s = AppState::new(std::path::PathBuf::from("/d"));
        s.entries = file_entries(1);
        s.cursor = 0;
        apply_intent(&mut s, Intent::StartMove, &OptCtx);
        // The Command panel reads this; it must begin with the actual `mv`, not `move`.
        assert_eq!(
            select_spec(&s).command_line.first(),
            Some(&CmdToken::Lit("mv".into())),
            "the resolved skeleton names the real executable",
        );
    }

    fn select_spec(s: &AppState) -> &crate::core::SelectSpec {
        match s.mode() {
            Mode::Select(spec) => spec,
            other => panic!("expected Select, got {other:?}"),
        }
    }

    #[test]
    fn copy_without_options_is_a_command_select_with_no_checkboxes() {
        let mut s = AppState::new(std::path::PathBuf::from("/d"));
        s.entries = file_entries(2);
        s.cursor = 0;
        apply_intent(&mut s, Intent::StartCopy, &NoCommands);
        let spec = select_spec(&s);
        assert_eq!(spec.id, "command");
        assert!(
            matches!(spec.source, crate::core::mode::SelectSource::Static(ref c) if c.is_empty()),
            "no declared options → empty checkbox list",
        );
    }

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

    #[test]
    fn copy_select_is_one_screen_with_option_checkboxes() {
        // The single command Select: Static options + Path input, no flag-picker.
        let mut s = AppState::new(std::path::PathBuf::from("/d"));
        s.entries = file_entries(1);
        s.cursor = 0;
        apply_intent(&mut s, Intent::StartCopy, &OptCtx);
        let spec = select_spec(&s);
        assert_eq!(spec.id, "command");
        assert!(matches!(spec.input, crate::core::SelectInput::Path), "input is the destination");
        assert!(
            matches!(spec.source, crate::core::mode::SelectSource::Static(ref c) if c.len() == 2),
            "candidate list is the two declared option checkboxes",
        );
    }

    #[test]
    fn command_dest_input_does_not_filter_the_options() {
        use crate::core::{Candidate, ExtensionValue, ResolveFill};
        let mut s = AppState::new(std::path::PathBuf::from("/d"));
        let opts = vec![
            Candidate::new("-v  verbose", ExtensionValue::String("-v".into())),
            Candidate::new("-n  no-clobber", ExtensionValue::String("-n".into())),
        ];
        let spec = crate::core::SelectSpec::command(
            op_request("copy", vec![std::path::PathBuf::from("/d/a")], None),
            ResolveFill::Dst,
            opts,
            None,
        );
        s.push_mode(Mode::Select(spec));
        // A destination that fuzzy-matches no option must NOT narrow the list.
        s.select = Some(SelectState::new("/some/where/else".into(), true));
        refilter(&mut s);
        assert_eq!(
            s.select.as_ref().unwrap().results.len(),
            2,
            "the dest input is free text, not a filter over the option checkboxes",
        );
    }

    #[test]
    fn command_confirm_runs_with_typed_dst_and_marked_options() {
        use crate::core::{Candidate, ExtensionValue, ResolveFill};
        let mut s = AppState::new(std::path::PathBuf::from("/d"));
        let template = op_request("copy", vec![std::path::PathBuf::from("/d/a")], None);
        let opt = Candidate::new("-v  verbose", ExtensionValue::String("-v".into()));
        let spec = crate::core::SelectSpec::command(template, ResolveFill::Dst, vec![opt.clone()], None);
        s.push_mode(Mode::Select(spec));
        // Typed destination + one option toggled (marked).
        let mut sel = SelectState::new("/dest".into(), true);
        sel.phase = SelectPhase::Navigate; // past the input phase
        sel.results = vec![opt];
        sel.marks.insert("-v  verbose".into());
        s.select = Some(sel);

        let fx = apply_intent(&mut s, Intent::Confirm, &NoCommands);
        match fx.intents.as_slice() {
            [Intent::RunResolver(r)] => {
                assert_eq!(r.op, "copy");
                assert_eq!(r.dst.as_deref(), Some("/dest"), "typed input → dst");
                assert_eq!(r.opts, vec!["-v".to_string()], "marked option → opts");
                assert_eq!(r.paths, vec![std::path::PathBuf::from("/d/a")]);
            }
            other => panic!("expected RunResolver(copy), got {other:?}"),
        }
    }
}
