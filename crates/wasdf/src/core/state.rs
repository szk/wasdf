//! AppState — everything that affects behavior. Mutated by the reducer only.

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::SystemTime;

use crate::core::event::{ExecOutput, PanelContent};
use crate::core::mode::{Mode, SubLayout};

/// A single directory entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
    pub is_symlink: bool,
    pub size: u64,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub modified: Option<SystemTime>,
    pub created: Option<SystemTime>,
    pub accessed: Option<SystemTime>,
    pub symlink_target: Option<PathBuf>,
}

impl Entry {
    pub fn is_hidden(&self) -> bool {
        self.name.starts_with('.')
    }
}

/// A navigable list of entries with a cursor: the single "entry list + cursor"
/// value shared by the file panel (the current directory) and any path-valued
/// Select (e.g. file-search results). Both render and navigate through the same
/// code, so a future Virtual File System only has to produce an `EntryList`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
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

/// Less-style incremental search over the text/hex content. Reducer-managed.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PanelSearch {
    /// True while the `/` input line is open and keys edit the query.
    pub input_active: bool,
    pub query: String,
    /// Caret byte offset within the query (readline editing).
    pub caret: usize,
    /// Match positions as (line index, byte start, byte end) within the content
    /// text lines, in document order.
    pub matches: Vec<(usize, usize, usize)>,
    /// Index into `matches` of the current match (the one `n`/`p` move from).
    pub current: usize,
}

/// Function-panel view state. Reducer-managed, lives in AppState.
#[derive(Debug, Clone, PartialEq)]
pub struct FunctionPanelState {
    pub visible: bool,
    pub sublayout: SubLayout,
    pub ratio: Ratio,
    pub scroll: usize,
    /// Horizontal scroll offset (columns) for the text/hex content.
    pub hscroll: usize,
    pub show_line_numbers: bool,
    /// Generic extension content shown in the Content frame (the Phase-B push
    /// path), and the id of the extension that owns the current content. The
    /// owning extension renders via its hook; this is its push-style buffer.
    pub content: Option<PanelContent>,
    pub content_owner: Option<String>,
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
            scroll: 0,
            hscroll: 0,
            show_line_numbers: false,
            content: None,
            content_owner: None,
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

/// A single mode frame: the mode plus the generation id assigned when pushed.
#[derive(Debug, Clone, PartialEq)]
pub struct ModeFrame {
    pub mode: Mode,
    pub generation: u64,
}

/// The whole behavior-affecting application state.
#[derive(Debug, Clone, PartialEq)]
pub struct AppState {
    pub cwd: PathBuf,
    pub entries: Vec<Entry>,
    pub cursor: usize,
    pub selection: HashSet<PathBuf>,
    pub show_hidden: bool,
    /// File-panel arrangement, cycled by `v`.
    pub list_layout: ListLayout,
    /// Last-rendered file-list geometry; kernel-refreshed post-draw, reducer-read.
    pub list_geom: ListGeom,
    pub function: FunctionPanelState,
    pub select: Option<SelectState>,
    pub focused_panel: String,
    pub modes: Vec<ModeFrame>,
    pub next_generation: u64,
    pub quit: bool,
    /// A path to place the cursor on after the next listing loads (e.g. the
    /// directory we came from on CursorLeft, or a navigated-to file).
    pub pending_focus: Option<PathBuf>,
}

impl AppState {
    pub fn new(cwd: PathBuf) -> Self {
        AppState {
            cwd,
            entries: Vec::new(),
            cursor: 0,
            selection: HashSet::new(),
            show_hidden: false,
            list_layout: ListLayout::default(),
            list_geom: ListGeom::default(),
            function: FunctionPanelState::default(),
            select: None,
            focused_panel: "file".into(),
            modes: vec![ModeFrame { mode: Mode::File, generation: 0 }],
            next_generation: 1,
            quit: false,
            pending_focus: None,
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
        self.focused_panel = "file".into();
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
