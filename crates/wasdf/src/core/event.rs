//! The async round-trip types: Plan (a closed set of six), AsyncResult (one
//! schema), and the AppEvent feeding the event loop.

use std::path::PathBuf;

use crate::core::extension_value::ExtensionValue;
use crate::core::intent::Key;
use crate::core::state::Entry;

/// A resolver request: an operation key plus the argument slots the resolver
/// expands into argv. Defined in core so Plan need not depend on services.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolverRequest {
    pub op: String,
    pub src: Option<PathBuf>,
    pub dst: Option<String>,
    pub path: Option<String>,
    pub paths: Vec<PathBuf>,
    /// Chosen command-line options (literal tokens like `-r`, `-v`), spliced into
    /// argv at the `opts` placeholder. Selected in the TUI; empty by default.
    pub opts: Vec<String>,
    /// A human label for completion notification.
    pub label: String,
}

/// The raw result of a generic content read, handed to the owning extension's
/// `accept_content`. The kernel reads bytes (files) or entries (directories)
/// off-thread; the *extension* decodes them into its own content. Both arms are
/// core types so the read stays content-agnostic.
#[derive(Debug, Clone, PartialEq)]
pub enum ReadResult {
    /// A byte chunk starting at `offset`; `eof` marks the last chunk (no bytes
    /// beyond it). The owning extension assembles consecutive chunks itself.
    Bytes { offset: u64, bytes: Vec<u8>, eof: bool },
    Dir { entries: Vec<Entry> },
}

/// The closed set of plans — the only way side effects leave the pipeline.
#[derive(Debug, Clone, PartialEq)]
pub enum Plan {
    ReadDir {
        path: PathBuf,
        show_hidden: bool,
    },
    /// Recursive walk + matcher ranking (file-search).
    Search {
        root: PathBuf,
        query: String,
        show_hidden: bool,
    },
    /// Read `path` for an extension's function-panel content (generic: a byte
    /// chunk from `offset` for a file, entries for a directory). The extension
    /// decodes the result and pages further chunks by re-issuing with a new
    /// offset.
    Read {
        owner: String,
        path: PathBuf,
        offset: u64,
    },
    ResolveAndRun {
        request: ResolverRequest,
    },
    Execute {
        argv: Vec<String>,
    },
    Suspend {
        argv: Vec<String>,
    },
    EvalScheme {
        expr: String,
    },
}

/// The fixed purpose namespace. Never extended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Purpose {
    Content,
    Search,
    Resolver,
    Execute,
    Scheme,
    Refresh,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AsyncStatus {
    Ok,
    Cancelled,
    Failed(String),
    StaleDiscarded,
}

/// A styled run within a content line: a byte length and its 24-bit colors. Runs
/// tile a line left-to-right and their lengths sum to the line's byte length; an
/// empty run list means the line is unstyled (plain). `bg` is optional so an
/// extension can express backgrounds (e.g. highlights) — the extension owns all
/// color; core's blitter only paints what the runs say.
#[derive(Debug, Clone, PartialEq)]
pub struct StyleRun {
    pub len: usize,
    pub fg: (u8, u8, u8),
    pub bg: Option<(u8, u8, u8)>,
}

/// Extension-agnostic content rendered into the function panel. Any extension
/// can produce it (synchronously via [`crate::core::Intent::ShowFunctionContent`]
/// or, later, asynchronously). The kernel renders it generically: text, hex, and
/// directory listings as `Lines`, images as `Image`.
#[derive(Debug, Clone, PartialEq)]
pub enum PanelContent {
    /// Styled text. `styles[i]` are the highlight runs for `lines[i]` (empty = plain).
    Lines { lines: Vec<String>, styles: Vec<Vec<StyleRun>> },
    /// A decoded image as packed RGB8 (width*height*3). Bundled-only across the ABI.
    Image { width: u32, height: u32, rgb: Vec<u8> },
}

/// Completion of a background resolver/open operation.
#[derive(Debug, Clone, PartialEq)]
pub struct OpDone {
    pub label: String,
    pub success: bool,
    pub message: Option<String>,
}

/// Captured output of an Execute plan.
#[derive(Debug, Clone, PartialEq)]
pub struct ExecOutput {
    pub lines: Vec<String>,
    pub finished: bool,
    pub exit: Option<i32>,
}

/// The single AsyncResult payload union.
#[derive(Debug, Clone, PartialEq)]
pub enum Payload {
    Entries { path: PathBuf, entries: Vec<Entry> },
    /// A content read completion: the owning extension and the path, plus the raw
    /// read result it should decode. The kernel routes this to the extension.
    Read { owner: String, path: PathBuf, result: ReadResult },
    OpDone(OpDone),
    Exec(ExecOutput),
    Scheme(ExtensionValue),
    None,
}

/// One schema for every async completion. Do not add fields.
#[derive(Debug, Clone, PartialEq)]
pub struct AsyncResult {
    pub request_id: u64,
    pub purpose: Purpose,
    pub mode_generation: u64,
    pub status: AsyncStatus,
    pub payload: Payload,
}

/// Events driving the event loop.
#[derive(Debug, Clone, PartialEq)]
pub enum AppEvent {
    Key(Key),
    Resize(u16, u16),
    Async(AsyncResult),
    Tick,
}
