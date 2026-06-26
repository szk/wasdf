//! The core Intent enum — a closed set. Extension features never add a core
//! variant; they travel as [`Intent::Extension`] carrying an [`ExtensionIntent`].

use std::path::PathBuf;

use crate::core::event::{PanelContent, ResolverRequest};
use crate::core::extension_value::ExtensionValue;
use crate::core::mode::{Mode, SubLayout};

/// A device-independent key, owned by core so the codec can map crossterm key
/// events to and from string names without core depending on crossterm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Key {
    pub code: KeyCode,
    pub mods: Mods,
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
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Mods {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}

impl Mods {
    pub const NONE: Mods = Mods { ctrl: false, shift: false, alt: false };
    pub const CTRL: Mods = Mods { ctrl: true, shift: false, alt: false };

    pub fn is_none(self) -> bool {
        self == Mods::NONE
    }
}

impl Key {
    pub fn new(code: KeyCode, mods: Mods) -> Self {
        Key { code, mods }
    }

    pub fn plain(code: KeyCode) -> Self {
        Key { code, mods: Mods::NONE }
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

    // Function cursor (operate the function panel from the file panel)
    FuncUp,
    FuncDown,
    FuncLeft,
    FuncRight,

    // Selection (a path set)
    ToggleSelect,
    SelectAll,
    ClearSelection,

    // External commands (resolved + executed)
    /// Run a resolver operation — the generic "build a command line and execute".
    /// Copy / move / rename / mkdir / touch / delete are all `RunResolver` with the
    /// op key; the destructive→Policy gate keys on the entry's destructive flag.
    RunResolver(ResolverRequest),
    Open { path: PathBuf },
    Edit { path: PathBuf },
    Execute { argv: Vec<String> },

    // Initiators (expanded by the File mode handler)
    StartCopy,
    StartMove,
    StartRename,
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
    /// A generic kernel mechanism: any extension produces `PanelContent`; the
    /// kernel stores and renders it. Not a per-feature variant.
    ShowFunctionContent { owner: String, content: PanelContent },
    /// Replace the opaque per-extension function-panel view state. Generic.
    UpdateFunctionView(ExtensionValue),
    /// Load a chunk of the given path (bytes from `offset`, or entries) for the
    /// named content owner — the generic cursor-follow content read. The kernel
    /// issues a `Plan::Read` and hands the result to the owner's `accept_content`.
    /// A content extension emits this from `on_cursor_changed`, advancing `offset`
    /// to page through a large file; it is not tied to any one extension.
    LoadContent { owner: String, path: PathBuf, offset: u64 },

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
    Noop,

    // Extension
    Extension(ExtensionIntent),
    UpdateExtensionState(ExtensionValue),
}
