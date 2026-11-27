//! AppState — everything that affects behavior. Mutated by the reducer only.

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::SystemTime;

use crate::core::event::{ExecOutput, PanelView};
use crate::core::intent::{ExtensionIntent, Intent, Key};
use crate::core::mode::{Mode, SubLayout};
use crate::core::scroll::Scroll;

/// The id stored in [`AppState::focused_panel`] for the file panel (the main
/// directory list). The single source of truth for this id, shared by the
/// reducer's focus control and the UI's focus recognition.
pub const PANEL_FILE: &str = "file";
/// The id stored in [`AppState::focused_panel`] for the function panel (the
/// right-hand renderer container).
pub const PANEL_FUNCTION: &str = "function";

/// One ancestor level of the current directory: the directories beside it and
/// which one is on the path.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TrailColumn {
    pub names: Vec<String>,
    pub index: usize,
}

/// A single directory entry.
///
/// `Eq` is intentionally absent: `activation` carries `ExtensionValue` which
/// contains a `BTreeMap` that is only `PartialEq`. Nothing in the codebase
/// requires `Entry: Eq` (selection uses `HashSet<PathBuf>` keyed on paths).
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
    pub is_symlink: bool,
    pub size: u64,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    /// The owner's and group's names, resolved where the entry is read (a
    /// kernel task), so render never asks the user database. Empty when the
    /// id has no name — render shows the number then.
    pub owner: std::sync::Arc<str>,
    pub group: std::sync::Arc<str>,
    pub modified: Option<SystemTime>,
    pub created: Option<SystemTime>,
    pub accessed: Option<SystemTime>,
    pub symlink_target: Option<PathBuf>,
    /// What activating this virtual leaf does. `Some` → `Activate` forwards
    /// this as `Intent::Extension` without interpreting it. `None` for all
    /// real-FS entries and for virtual directory nodes.
    pub activation: Option<ExtensionIntent>,
}

impl Entry {
    pub fn is_hidden(&self) -> bool {
        self.name.starts_with('.')
    }

    /// Construct a virtual directory entry (no real I/O metadata).
    pub fn virtual_dir(path: PathBuf, name: String) -> Self {
        Entry {
            path,
            name,
            is_dir: true,
            is_symlink: false,
            size: 0,
            mode: 0,
            uid: 0,
            gid: 0,
            owner: "".into(),
            group: "".into(),
            modified: None,
            created: None,
            accessed: None,
            symlink_target: None,
            activation: None,
        }
    }

    /// Construct a virtual leaf entry (no real I/O metadata; caller sets `activation`).
    pub fn virtual_leaf(path: PathBuf, name: String) -> Self {
        Entry {
            path,
            name,
            is_dir: false,
            is_symlink: false,
            size: 0,
            mode: 0,
            uid: 0,
            gid: 0,
            owner: "".into(),
            group: "".into(),
            modified: None,
            created: None,
            accessed: None,
            symlink_target: None,
            activation: None,
        }
    }
}

/// A navigable list of entries with a cursor: the single "entry list + cursor"
/// value shared by the file panel (the current directory) and any path-valued
/// Select (e.g. file-search results). Both render and navigate through the same
/// code, so a Virtual File System only has to produce an `EntryList`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EntryList {
    pub entries: Vec<Entry>,
    pub cursor: usize,
}

impl EntryList {
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    pub fn current(&self) -> Option<&Entry> {
        self.entries.get(self.cursor)
    }
    /// Clamp the cursor into range after the listing changes.
    pub fn clamp(&mut self) {
        if self.entries.is_empty() {
            self.cursor = 0;
        } else if self.cursor >= self.entries.len() {
            self.cursor = self.entries.len() - 1;
        }
    }
}

/// How the file panel arranges entries. Reducer-managed; cycled by `v`.
/// Rows is the `cols = 1` degenerate of the column-major multi-column layouts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ListLayout {
    /// One entry per line, full width (the default / classic view).
    #[default]
    Rows,
    /// `ls -C` style: column-major, as many columns as fit, each sized to its own
    /// entries (per-column variable width).
    Columns,
    /// Uniform square-ish cells (rows ≈ cols), column-major.
    Grid,
}

impl ListLayout {
    /// Cycle Rows → Columns → Grid → Rows.
    pub fn next(self) -> Self {
        match self {
            ListLayout::Rows => ListLayout::Columns,
            ListLayout::Columns => ListLayout::Grid,
            ListLayout::Grid => ListLayout::Rows,
        }
    }
}

/// Last-rendered file-list geometry (columns × visual rows), refreshed by the
/// kernel after each draw so the pure reducer can move the cursor in 2-D without
/// seeing the viewport — the same render→behavior seam as the function-panel
/// scroll clamp. `rows == 0` means "not yet measured": navigation then falls back
/// to single-column (Rows) behavior. In the Rows layout it is `(1, n)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListGeom {
    pub cols: usize,
    pub rows: usize,
}

impl Default for ListGeom {
    fn default() -> Self {
        ListGeom { cols: 1, rows: 0 }
    }
}

/// The file:function split ratio, always kept and shared across modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ratio {
    R2_1,
    R1_1,
    R1_2,
}

impl Ratio {
    /// (left, right) weights for the file:function split.
    pub fn weights(self) -> (u32, u32) {
        match self {
            Ratio::R2_1 => (2, 1),
            Ratio::R1_1 => (1, 1),
            Ratio::R1_2 => (1, 2),
        }
    }
}

/// A generic text input an extension has requested in the function panel.
/// Core owns the buffer (it gates key routing, so it must be in `AppState`);
/// the extension owns what the submitted text *means*, disambiguated by
/// `purpose` (opaque to core). Single-line by default; `multiline` (set from
/// `Intent::BeginInput`) is the one structurally-interpreted flag — see its
/// doc comment for the blank-line-submits convention.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FunctionInput {
    /// The current text (embedded `\n`s when `multiline`) and caret byte
    /// offset (generic readline editing).
    pub query: String,
    pub caret: usize,
    /// An extension-defined tag telling the owner which input this is (e.g.
    /// "filter" / "prompt" / "builder-edit"). Core never interprets it.
    pub purpose: String,
    pub multiline: bool,
}

/// Less-style incremental search over the text/hex content. Reducer-managed.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PanelSearch {
    /// True while the `/` input line is open and keys edit the query.
    pub input_active: bool,
    pub query: String,
    /// Caret byte offset within the query (readline editing).
    pub caret: usize,
    /// Match positions in logical coordinates — (block index, byte start, byte
    /// end) within the block's text (`Block::text`) — sorted. Core maps them
    /// to panel rows at layout (doc/EXTENSION.md).
    pub matches: Vec<(usize, usize, usize)>,
    /// Index into `matches` of the current match (the one `n`/`p` move from).
    pub current: usize,
    /// Bring the current match into view at the next layout. The reducer
    /// cannot: which row a block lands on depends on the panel's geometry.
    pub reveal: bool,
    /// What the open input is for, shown keyless at the head of the help row.
    /// The same idea as `SelectSpec::input_hint`, but this input is a panel
    /// state rather than a mode, so it carries its own (doc/HELP.md).
    pub hint: String,
}

/// Function-panel view state. Reducer-managed, lives in AppState.
#[derive(Debug, Clone, PartialEq)]
pub struct FunctionPanelState {
    pub visible: bool,
    pub sublayout: SubLayout,
    pub ratio: Ratio,
    /// Vertical / horizontal content scroll (shared by the Content and Exec
    /// frames). Kernel-owned: advanced blindly, clamped post-draw via [`Scroll`].
    pub scroll: Scroll,
    pub hscroll: Scroll,
    pub show_line_numbers: bool,
    /// Generic extension content shown in the Content frame (the Phase-B push
    /// path), and the id of the extension that owns the current content. The
    /// owning extension renders via its hook; this is its push-style buffer.
    pub content: Option<std::sync::Arc<PanelView>>,
    pub content_owner: Option<String>,
    /// The id of the extension currently hosting an **interactive form** in the
    /// panel (e.g. the CmdOp builder), distinct from the passive cursor-follow
    /// `content_owner`. When `Some`, render dispatches the whole inner rect to that
    /// extension's `render_form`, and its keys are gated by a `:when` predicate
    /// reading this field — the generic seam that lets an extension own the panel
    /// without core knowing what it is.
    pub form_owner: Option<String>,
    /// A generic text-input (readline) an extension has requested: while `Some`,
    /// the kernel routes every key as a raw event and the reducer edits the buffer;
    /// Enter/Esc hand the text back to `form_owner` as an extension intent. Reused
    /// by any form extension (the CmdOp filter / dest prompt / builder edit).
    pub input: Option<FunctionInput>,
    /// Opaque per-extension view state for the active function content, written
    /// via `UpdateFunctionView`. The owning extension manages its shape; the
    /// kernel only stores it (and a bundled extension can read it in
    /// `handle_intent`).
    pub ext_view: crate::core::ExtensionValue,
    /// Less-style search state over the current text/hex content.
    pub search: PanelSearch,
    /// Captured Execute output.
    pub exec: ExecOutput,
}

impl Default for FunctionPanelState {
    fn default() -> Self {
        FunctionPanelState {
            visible: false,
            sublayout: SubLayout::Content,
            ratio: Ratio::R2_1,
            scroll: Scroll::default(),
            hscroll: Scroll::default(),
            show_line_numbers: false,
            content: None,
            content_owner: None,
            form_owner: None,
            input: None,
            ext_view: crate::core::ExtensionValue::Nil,
            search: PanelSearch::default(),
            exec: ExecOutput { lines: Vec::new(), finished: true, exit: None },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectPhase {
    Input,
    Navigate,
}

/// Runtime Select state. Lives in AppState; mutated only by the reducer.
#[derive(Debug, Clone, PartialEq)]
pub struct SelectState {
    pub query: String,
    /// Caret byte offset within the query (readline editing).
    pub caret: usize,
    pub phase: SelectPhase,
    pub results: Vec<crate::core::mode::Candidate>,
    pub selected: usize,
    /// Marks keyed by candidate label (stable across re-ranking).
    pub marks: HashSet<String>,
    /// Path-valued results (file-search) as a navigable entry list, rendered and
    /// navigated through the same code as the file panel. `None` for token-valued
    /// Selects (palette / options / extension static), which use `results`.
    pub view: Option<EntryList>,
}

impl SelectState {
    pub fn new(initial_query: String, has_input: bool) -> Self {
        let caret = initial_query.len();
        SelectState {
            query: initial_query,
            caret,
            phase: if has_input { SelectPhase::Input } else { SelectPhase::Navigate },
            results: Vec::new(),
            selected: 0,
            marks: HashSet::new(),
            view: None,
        }
    }
}

/// Runtime state for a `Mode::InputForm` popup: one variant per
/// [`crate::core::InputKind`]. `push_mode` decodes `spec.initial` into this
/// shape; `input_form_confirm` encodes it back into the `KEY_ITEM` wire
/// string (see `core::reducer::{decode_input_state, encode_input_value}`).
#[derive(Debug, Clone, PartialEq)]
pub enum InputFormState {
    /// `cursor` is a **char index** (not a byte offset), matching
    /// `tui_input::Input::cursor()`'s convention, since the reducer
    /// reconstructs a transient `tui_input::Input` from these two fields on
    /// every keystroke.
    Text { value: String, cursor: usize },
    Bool { value: bool },
    Choice { options: Vec<String>, selected: usize },
    /// `typed`, when non-empty, is an in-progress digit buffer (may start with
    /// `-`) that overrides how the next digit/Backspace is interpreted; any
    /// arrow/Home/End key clears it and adjusts `value` directly instead —
    /// needed because some `Int` options span millions (e.g. matcher tuning),
    /// far too wide for arrow-only ±1/±10 stepping to reach.
    Int { value: i64, min: i64, max: i64, typed: String },
    /// `editing`, when `Some((index, buf, cursor))`, is a nested text edit of
    /// one item (index `usize::MAX` means a new not-yet-appended item); `None`
    /// means the row-browsing phase (mirrors `SelectPhase` conceptually).
    List { items: Vec<String>, selected: usize, editing: Option<(usize, String, usize)> },
}

/// A single mode frame: the mode plus the generation id assigned when pushed.
#[derive(Debug, Clone, PartialEq)]
pub struct ModeFrame {
    pub mode: Mode,
    pub generation: u64,
}

/// A saved filter level: the file panel's entries/cursor/active query exactly
/// as they were *before* a file-search query was committed on top of them.
/// Pushed by `commit_file_search`, popped one level at a time by
/// `Intent::ClearFilter` (Esc) — the undo stack behind nested `f` filtering.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FilterFrame {
    pub entries: Vec<Entry>,
    pub cursor: usize,
    pub query: Option<String>,
}

/// How many filter levels `filter_stack` may hold at once — caps nested `f`
/// (each level is a full re-walk from `cwd`, not a narrowing of the current
/// matches, so an unbounded stack would only bound memory, not usefulness).
pub const MAX_FILTER_DEPTH: usize = 5;

/// The whole behavior-affecting application state.
#[derive(Debug, Clone, PartialEq)]
pub struct AppState {
    pub cwd: PathBuf,
    pub entries: Vec<Entry>,
    /// The nav panel's ancestor columns for `cwd`, root first, read with the
    /// listing (doc/ARCHITECTURE.md, Render Contract).
    pub trail: Vec<TrailColumn>,
    pub cursor: usize,
    pub selection: HashSet<PathBuf>,
    pub show_hidden: bool,
    /// File-panel arrangement, cycled by `v`.
    pub list_layout: ListLayout,
    /// Last-rendered file-list geometry; kernel-refreshed post-draw, reducer-read.
    pub list_geom: ListGeom,
    pub function: FunctionPanelState,
    pub select: Option<SelectState>,
    /// Runtime state for a `Mode::InputForm` popup: the typed value and
    /// cursor (char index, matching `tui_input::Input`'s convention). `None`
    /// whenever `Mode::InputForm` isn't on top of the stack.
    pub input_form: Option<InputFormState>,
    pub focused_panel: String,
    pub modes: Vec<ModeFrame>,
    pub next_generation: u64,
    pub quit: bool,
    /// A path to place the cursor on after the next listing loads (e.g. the
    /// directory we came from on CursorLeft, or a navigated-to file).
    pub pending_focus: Option<PathBuf>,
    /// Where to return to when toggling back out of the VFS root via `@`.
    pub root_return: Option<PathBuf>,
    /// Filter levels beneath the currently shown listing, innermost last.
    /// Non-empty ⇒ `entries` holds file-search matches rather than a real
    /// directory listing — every other File-mode key (copy, move, rename,
    /// delete, bookmark, hjkl scroll, …) still reads `entries`/`cursor`
    /// directly, so nothing about them needs to know a filter is active.
    /// `Intent::ClearFilter` (Esc) pops one level; any real directory
    /// (re)load clears the whole stack (see `load_entries`).
    pub filter_stack: Vec<FilterFrame>,
    /// The query text that produced the currently shown listing, when
    /// filtered — `None` exactly when `filter_stack` is empty.
    pub filter_query: Option<String>,
    /// An extension's claim on the raw keyboard, if any. See [`InputCapture`].
    pub input_capture: Option<InputCapture>,
}

/// An extension's claim on the raw keyboard. While one is installed,
/// `handle_key` hands keys to `owner` instead of acting on them — except for
/// the two escapes below, which **core** honours on its own.
///
/// Both escapes are data core owns rather than a protocol the extension must
/// implement, and that is the point: an extension that panics, deadlocks, or
/// simply mishandles a key cannot leave the user with no way back. Nothing
/// here is specific to any one extension.
#[derive(Debug, Clone, PartialEq)]
pub struct InputCapture {
    /// The extension receiving captured keys, as `intent = "key"`.
    pub owner: String,
    /// The hard release key, checked before anything else. Core clears the
    /// capture on it unconditionally, then delivers `intent = "prefix"` so a
    /// healthy extension can react — but the release has already happened
    /// whether or not it answers.
    pub release: Key,
    /// Intents that also end the capture. Core resolves each key through the
    /// keymap as usual; a hit here clears the capture **and dispatches the
    /// intent**, so the action the user pressed the key for still happens.
    /// Declared as intents rather than keys so it follows the user's keymap.
    /// Empty ⇒ only `release` escapes, and the keymap is never consulted.
    pub escape: Vec<Intent>,
}

impl AppState {
    pub fn new(cwd: PathBuf) -> Self {
        AppState {
            cwd,
            entries: Vec::new(),
            trail: Vec::new(),
            cursor: 0,
            selection: HashSet::new(),
            show_hidden: false,
            list_layout: ListLayout::default(),
            list_geom: ListGeom::default(),
            function: FunctionPanelState::default(),
            select: None,
            input_form: None,
            focused_panel: PANEL_FILE.into(),
            modes: vec![ModeFrame { mode: Mode::File, generation: 0 }],
            next_generation: 1,
            quit: false,
            pending_focus: None,
            root_return: None,
            filter_stack: Vec::new(),
            filter_query: None,
            input_capture: None,
        }
    }

    /// The topmost mode.
    pub fn mode(&self) -> &Mode {
        &self.modes.last().expect("mode stack is never empty").mode
    }

    pub fn mode_generation(&self) -> u64 {
        self.modes.last().map(|f| f.generation).unwrap_or(0)
    }

    /// The full mode id of the topmost mode.
    pub fn mode_id(&self) -> String {
        self.mode().id()
    }

    pub fn current_entry(&self) -> Option<&Entry> {
        self.entries.get(self.cursor)
    }

    /// The path the cursor is logically on: the File-mode cursor entry, or (while
    /// browsing the file-search picker) the currently highlighted search hit.
    /// `None` in any other mode. The single mode-dispatch every "what's under the
    /// cursor" query shares — the cursor-changed broadcast, a dynamic extension's
    /// cursor hook, and the content provider's follow target all read this
    /// instead of repeating the same `Mode::File` / `Mode::Select("file-search")`
    /// match.
    pub fn cursor_path(&self) -> Option<PathBuf> {
        match self.mode() {
            Mode::File => self.current_entry().map(|e| e.path.clone()),
            // Any Select with an entry-grid view — deep file-search, a
            // shallow filter/refine (SEARCH_RENEW.md) — not just file-search
            // by id, so cursor-follow (theme editor, preview) works
            // identically inside every one of them.
            Mode::Select(_) => self
                .select
                .as_ref()
                .and_then(|s| s.view.as_ref())
                .and_then(|v| v.current())
                .map(|e| e.path.clone()),
            _ => None,
        }
    }

    /// The operation target list: the selection if non-empty, else the cursor
    /// entry. See RESOLVER.md.
    pub fn targets(&self) -> Vec<PathBuf> {
        if !self.selection.is_empty() {
            let mut v: Vec<PathBuf> = self.entries
                .iter()
                .filter(|e| self.selection.contains(&e.path))
                .map(|e| e.path.clone())
                .collect();
            if v.is_empty() {
                v = self.selection.iter().cloned().collect();
            }
            v
        } else if let Some(e) = self.current_entry() {
            vec![e.path.clone()]
        } else {
            Vec::new()
        }
    }

    /// Push a mode, assigning it a fresh generation.
    pub fn push_mode(&mut self, mode: Mode) -> u64 {
        let generation = self.next_generation;
        self.next_generation += 1;
        self.modes.push(ModeFrame { mode, generation });
        generation
    }

    /// Pop the topmost mode (never the base File mode).
    pub fn pop_mode(&mut self) {
        if self.modes.len() > 1 {
            self.modes.pop();
        }
    }

    /// Replace the topmost mode (used when a Select replaces a Select).
    pub fn replace_mode(&mut self, mode: Mode) -> u64 {
        let generation = self.next_generation;
        self.next_generation += 1;
        if self.modes.len() > 1 {
            self.modes.pop();
        }
        self.modes.push(ModeFrame { mode, generation });
        generation
    }

    /// Collapse the mode stack back to the base File mode and drop any Select
    /// state. Used after a suspended child (the editor) returns: the edit is
    /// over, so we land on the file list regardless of what ran — and any stray
    /// bytes the child left in stdin can't strand us in a half-opened mode.
    pub fn reset_to_file(&mut self) {
        self.modes.truncate(1);
        self.select = None;
        self.focused_panel = PANEL_FILE.into();
    }

    /// Clamp the cursor into range after a listing change.
    pub fn clamp_cursor(&mut self) {
        if self.entries.is_empty() {
            self.cursor = 0;
        } else if self.cursor >= self.entries.len() {
            self.cursor = self.entries.len() - 1;
        }
    }
}
