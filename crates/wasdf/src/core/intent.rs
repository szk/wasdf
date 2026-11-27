//! The core Intent enum — a closed set. Extension features never add a core
//! variant; they travel as [`Intent::Extension`] carrying an [`ExtensionIntent`].

use std::path::PathBuf;

use crate::core::event::{PanelView, ResolverRequest};
use crate::core::extension_value::ExtensionValue;
use crate::core::mode::{Mode, SubLayout};
use crate::core::state::InputCapture;

/// A device-independent key, owned by core so the codec can map crossterm key
/// events to and from string names without core depending on crossterm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Key {
    pub code: KeyCode,
    pub mods: Mods,
    pub kind: KeyKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeyCode {
    Char(char),
    Enter,
    Esc,
    Tab,
    BackTab,
    Backspace,
    Delete,
    Insert,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    /// F1–F35. Not folded into `Char` because a function key has no character
    /// and its escape encoding is entirely separate.
    F(u8),
}

/// Press / repeat / release. Legacy terminals report presses only; the other
/// two arrive under the Kitty keyboard protocol's `REPORT_EVENT_TYPES`, and a
/// PTY child that asked for them expects to see them.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeyKind {
    #[default]
    Press,
    Repeat,
    Release,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Mods {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    /// Cmd on macOS, Win elsewhere. Reported only under the Kitty keyboard
    /// protocol; always false on a legacy terminal.
    pub sup: bool,
    pub hyper: bool,
    pub meta: bool,
}

impl Mods {
    pub const NONE: Mods = Mods {
        ctrl: false,
        shift: false,
        alt: false,
        sup: false,
        hyper: false,
        meta: false,
    };
    pub const CTRL: Mods = Mods { ctrl: true, ..Mods::NONE };

    pub fn is_none(self) -> bool {
        self == Mods::NONE
    }
}

impl Key {
    pub fn new(code: KeyCode, mods: Mods) -> Self {
        Key { code, mods, kind: KeyKind::Press }
    }

    pub fn plain(code: KeyCode) -> Self {
        Key { code, mods: Mods::NONE, kind: KeyKind::Press }
    }

    /// A bare printable character with no modifiers (ignoring shift, which is
    /// already folded into the character itself).
    pub fn printable_char(self) -> Option<char> {
        match self.code {
            KeyCode::Char(c) if !self.mods.ctrl && !self.mods.alt => Some(c),
            _ => None,
        }
    }
}

/// An extension intent: addressed to an extension by id, carrying a structured
/// payload. The reserved `resolver` key turns it into a kernel-executed plan;
/// the reserved `item` key carries a confirm shape from an Emit Select.
#[derive(Debug, Clone, PartialEq)]
pub struct ExtensionIntent {
    pub extension: String,
    pub intent: String,
    pub data: ExtensionValue,
}

/// The closed core intent set. See the intent catalog in ARCHITECTURE.md.
#[derive(Debug, Clone, PartialEq)]
pub enum Intent {
    // Navigation
    CursorUp,
    CursorDown,
    CursorTop,
    CursorBottom,
    CursorLeft,
    CursorRight,
    /// Activate the entry under the cursor: enter the directory, or open the file
    /// via the resolver. Layout-independent (bound to Enter). "d = look, Enter =
    /// do" — viewing a file is `d` (cycle the content panel) / `,`, not Activate.
    Activate,
    NavigateTo(PathBuf),
    /// Toggle between the current real directory and the VFS root (`@`). The
    /// first press from a real path jumps to `@` and remembers where it came
    /// from; pressing it again returns there.
    ToggleVfsRoot,

    // Function cursor (operate the function panel from the file panel)
    FuncUp,
    FuncDown,
    FuncLeft,
    FuncRight,

    // Selection (a path set)
    ToggleSelect,
    SelectAll,
    ClearSelection,
    /// Pop one level of the file-search filter stack, restoring the entries the
    /// panel showed before it (the real directory listing, or the parent filter's
    /// own matches for a nested `f`). A no-op with no filter active — bound only
    /// while `has-filter` holds (see keymap.scm).
    ClearFilter,
    /// Shallow filter (`f` on a virtual directory — see doc/UI.md):
    /// push a Select ranking the file panel's *current* `entries` in place,
    /// synchronously, no I/O. The reducer takes the snapshot (this intent
    /// carries none — the keymap codec builds intents without state). Commits
    /// through the same path as deep file-search (`commit_file_search`).
    FilterListing,
    /// Shallow refine (`f` inside a Select whose `view` already holds
    /// results — deep file-search or an earlier filter/refine): replace the
    /// current Select in place with a fresh shallow filter over the frozen
    /// result set. No refine-undo stack; Esc cancels the whole Select.
    SelectRefine,

    // External commands (resolved + executed)
    /// Run a resolver operation — the generic "build a command line and execute".
    /// Copy / move / rename / mkdir / touch / delete are all `RunResolver` with the
    /// op key; the destructive→Policy gate keys on the entry's destructive flag.
    RunResolver(ResolverRequest),
    Open { path: PathBuf },
    Edit { path: PathBuf },
    Execute { argv: Vec<String> },

    // Initiators (expanded by the File mode handler)
    DeleteSelected,
    StartEdit,

    // Mode stack
    PushMode(Box<Mode>),
    PopMode,

    // Function panel view state
    SetSubLayout(SubLayout),
    CycleFunctionPanel,
    HideFunctionPanel,
    /// Show (or replace) the function panel's content, owned by an extension.
    /// A generic kernel mechanism: any extension produces a logical `PanelView`;
    /// core lays it out and renders it. Not a per-feature variant.
    ShowFunctionContent { owner: String, view: PanelView },
    /// Replace the opaque per-extension function-panel view state. Generic.
    UpdateFunctionView(ExtensionValue),
    /// Load a chunk of the given path (bytes from `offset`, or entries) for the
    /// named content owner — the generic cursor-follow content read. The kernel
    /// issues a `Plan::Read` and hands the result to the owner's `accept_content`.
    /// A content extension emits this from `on_cursor_changed`, advancing `offset`
    /// to page through a large file; it is not tied to any one extension.
    LoadContent { owner: String, path: PathBuf, offset: u64 },
    /// Deep search (recursive walk + matcher ranking) from `root`, addressed
    /// to `owner` — the extension-facing counterpart of the file panel's own
    /// `f` (see doc/UI.md). The kernel issues an owner-addressed
    /// `Plan::Search`; the completion arrives as a generic content Read
    /// (`Content::Dir`) and is handed to `owner`'s `accept_content`, same
    /// as `LoadContent`. Not tied to any one extension.
    LoadSearch { owner: String, root: PathBuf, query: String },
    /// Set (or clear) the extension currently hosting an interactive form in the
    /// function panel. `Some(id)` shows + focuses the panel and routes its render
    /// and key-gating to that extension; `None` closes the form. Generic seam.
    SetFunctionForm(Option<String>),
    /// Like `SetFunctionForm`, but passive: shows/hides the extension's form
    /// (`form_owner`, `visible`) without touching `focused_panel`. For a form
    /// that follows the file cursor (like content's `LoadContent`, but write-
    /// capable and needing raw `Frame` access via `render_form`) — the file
    /// panel keeps focus, so its own wasd/hjkl bindings keep working, exactly
    /// as `FuncUp`/`FuncDown`/`FuncLeft`/`FuncRight` already let it drive
    /// scrolling a passively-followed content panel. An extension key-gates
    /// its own edit bindings on `form_owner`, scoped to the **file** panel
    /// (not `function`), so they fire without a focus switch. Generic seam.
    HostFunctionForm(Option<String>),
    /// Open a generic function-panel text input with the given `purpose` tag,
    /// seeded with `seed`. The kernel then raw-routes keys into the buffer; on
    /// Enter/Esc the reducer hands the text back to the `form_owner`. Generic.
    /// `multiline`: when false (the common case), Enter submits immediately,
    /// same as always. When true, Enter inserts `\n` instead — submission
    /// then happens on an Enter pressed while the current line is already
    /// empty (a blank line terminates the batch, the same convention
    /// `InputKind::List`'s row editing uses, adapted for free text instead of
    /// discrete rows). `purpose` stays fully opaque to core either way — only
    /// this flag is structurally interpreted, not the tag string itself.
    BeginInput { purpose: String, seed: String, multiline: bool },

    // Function-panel scroll
    ScrollUp,
    ScrollDown,
    ScrollTop,
    ScrollBottom,
    PageUp,
    PageDown,
    /// Set the vertical scroll offset directly (generic; used by an extension to
    /// jump the view to a search match).
    SetScroll(usize),

    // Content (text/hex) less-style search
    FunctionSearchStart,
    FunctionSearchSubmit,
    FunctionSearchNext,
    FunctionSearchPrev,
    FunctionSearchCancel,
    /// Store the match set an extension computed (line, byte start, byte end).
    /// Match state lives in core because `:when` predicates gate keys on it.
    SetSearchMatches(Vec<(usize, usize, usize)>),
    /// Reset the content view state (scroll/h-scroll/search) on a new target.
    ResetContentView,

    // Dialog (phase-interpreted in Select; allow/deny in Policy)
    Confirm,
    Cancel,

    // InputForm's `List` kind row management (browsing only — see
    // "input-form-list-browsing" in keymap.scm); a no-op in any other mode
    // or InputForm kind.
    AddItem,
    RemoveItem,

    // Select (results navigate via the shared Cursor* intents)
    RawKeyEvent(Key),

    // Panel focus
    FocusNextPanel,
    FocusPrevPanel,
    FocusPanel(String),

    // View toggles
    ToggleDotFiles,
    ToggleLineNumbers,
    /// Cycle the file-list layout: Rows → Columns → Grid → Rows.
    CycleListLayout,

    // Process (Exec frame only)
    KillProcess,

    // Misc
    Refresh,
    Quit,
    /// Quit and hand the directory on screen back to the calling shell.
    QuitCd,
    Noop,
    /// Show a transient user-facing notice. A generic kernel mechanism: any
    /// extension (or core) emits a message string; the kernel displays it via the
    /// standard notice channel. Not tied to any one feature.
    Notify(String),
    /// A structured write to the settings store's `custom` layer (the settings
    /// UI, extension editors). Runs as a store call; the change set updates
    /// the projections and the Persister writes custom.scm.
    Configure(crate::core::ConfigWrite),
    /// Evaluate `expr` in the resident Scheme session (the `session` layer) on
    /// behalf of `owner`; the printed value or error comes back to `owner`'s
    /// `accept_content` as `Content::Value`.
    Eval { owner: String, expr: String },

    /// Install or clear an extension's claim on the raw keyboard
    /// ([`crate::core::state::InputCapture`]). Generic: any extension that has
    /// to drive something keystroke-by-keystroke uses it, and core keeps the
    /// escapes so the claim can always be broken.
    SetInputCapture(Option<Box<InputCapture>>),

    // Extension
    Extension(ExtensionIntent),
    UpdateExtensionState(ExtensionValue),
}
