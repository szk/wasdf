//! The async round-trip types: Plan (a closed set of six), AsyncResult (one
//! schema), and the AppEvent feeding the event loop.

use std::path::PathBuf;

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
pub enum Content {
    /// A byte chunk starting at `offset`; `eof` marks the last chunk (no bytes
    /// beyond it). The owning extension assembles consecutive chunks itself.
    Bytes { offset: u64, bytes: Vec<u8>, eof: bool },
    Dir { entries: Vec<Entry> },
    /// A Scheme evaluation's printed value, or its error (`Intent::Eval`).
    Value(Result<String, String>),
}

/// The closed set of plans — the only way side effects leave the pipeline.
#[derive(Debug, Clone, PartialEq)]
pub enum Plan {
    ReadDir {
        path: PathBuf,
        show_hidden: bool,
    },
    /// Recursive walk + matcher ranking (file-search, deep — see
    /// SEARCH_RENEW.md). `owner: None` completes as `(Purpose::Search,
    /// Payload::Entries)`, driving a Select's own `view` (today's file-search).
    /// `owner: Some(id)` completes as a generic content Read instead
    /// (`Purpose::Content`, `Payload::Read{owner, path: root, Content::Dir}`)
    /// — the extension-facing counterpart, addressed like `Plan::Read`,
    /// routed to `id`'s `accept_content` with zero new kernel plumbing.
    Search {
        root: PathBuf,
        query: String,
        show_hidden: bool,
        owner: Option<String>,
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
    /// Evaluate in the settings store's session (`session` layer current). The
    /// result goes to `owner` when there is one.
    EvalScheme {
        expr: String,
        owner: Option<String>,
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

/// Layout-free logical content for the function panel. An extension describes
/// *what* to show — sections, key/value fields, and spans tagged with a theme
/// role or an explicit color — and **core owns all layout**: label-column
/// alignment, line wrapping / horizontal scroll, vertical scroll, line numbers,
/// and theming. Extensions never assemble pre-styled lines. A dynamic extension
/// emits this as scheme data over the ABI (`(document (field …) (line …) …)`),
/// which the codec decodes into a `PanelView`.
#[derive(Debug, Clone, PartialEq)]
pub enum PanelView {
    /// A sequence of logical blocks core lays out top-to-bottom.
    Document(Vec<Block>),
    /// A decoded image as packed RGB8 (width*height*3). Bundled-only across the ABI.
    Image { width: u32, height: u32, rgb: Vec<u8> },
}

/// One logical block of a [`PanelView::Document`].
#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    /// A key/value row; core aligns the `label` column across the document.
    Field { label: String, value: Vec<TextSpan> },
    /// One logical line of rich text.
    Line(Vec<TextSpan>),
    /// A section heading (core renders it emphasized).
    Heading(Vec<TextSpan>),
    /// A full-width horizontal separator.
    Rule,
    /// A decoded image as packed RGB8 (width*height*3), blitted inline at the
    /// panel width. Same payload as [`PanelView::Image`]; this variant is the
    /// *inline* form, for documents that mix prose and pictures (markdown).
    /// Core picks the row count from the panel width — see [`Block::rows`].
    /// Bundled-only across the ABI.
    Image { width: u32, height: u32, rgb: Vec<u8> },
}

impl Block {
    /// Panel rows this block occupies once laid out `cols` columns wide (the
    /// text area — subtract [`LINE_NUMBER_WIDTH`] first when the gutter is on).
    /// Every block is a single row except an image, which is blitted to its
    /// aspect ratio and bounded by `max_rows`.
    ///
    /// Producers need this too, not just the renderer: anything that reports
    /// *row* indices back to core (a search's match lines) has to agree with
    /// the layout about an image's footprint.
    pub fn rows(&self, cols: u16, max_rows: u16) -> u16 {
        match self {
            // Terminal cells run about twice as tall as they are wide.
            Block::Image { width, height, .. } => {
                if *width == 0 || *height == 0 || cols == 0 || max_rows == 0 {
                    return 0;
                }
                let rows =
                    (u64::from(*height) * u64::from(cols)).div_ceil(u64::from(*width) * 2);
                rows.clamp(1, u64::from(max_rows)) as u16
            }
            _ => 1,
        }
    }
}

/// Columns the function panel reserves for the line-number gutter when it is
/// shown. Core's line painter lays out the number in this width.
pub const LINE_NUMBER_WIDTH: u16 = 5;

/// A run of text with a logical style; core resolves the style at render time.
#[derive(Debug, Clone, PartialEq)]
pub struct TextSpan {
    pub text: String,
    pub style: SpanStyle,
    /// Font emphasis, orthogonal to the color in `style`. Markdown's `**bold**`
    /// / `*italic*` carry no color of their own, so emphasis cannot be folded
    /// into [`SpanStyle`].
    pub emph: Emph,
}

impl TextSpan {
    /// A span in the default (unstyled) text color.
    pub fn plain(text: impl Into<String>) -> Self {
        TextSpan { text: text.into(), style: SpanStyle::Default, emph: Emph::default() }
    }

    /// A span in `style`, unemphasized.
    pub fn styled(text: impl Into<String>, style: SpanStyle) -> Self {
        TextSpan { text: text.into(), style, emph: Emph::default() }
    }
}

/// Font emphasis on a [`TextSpan`]. Default is "no emphasis"; core maps each
/// flag onto the corresponding ratatui `Modifier` at render time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Emph {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub dim: bool,
}

/// How a [`TextSpan`] is colored. `Role` is resolved against the active theme so
/// extension content re-themes automatically; `Rgb` is a literal escape hatch
/// (e.g. syntax-highlight colors that are not theme roles).
#[derive(Debug, Clone, PartialEq)]
pub enum SpanStyle {
    Default,
    Role(String),
    Rgb { fg: (u8, u8, u8), bg: Option<(u8, u8, u8)> },
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
    Read { owner: String, path: PathBuf, result: Content },
    OpDone(OpDone),
    Exec(ExecOutput),
    /// A settings-store evaluation: its printed value (or error) for `owner`,
    /// and the change set in its printed form, which the kernel decodes and
    /// applies to the projections (core treats it as opaque text).
    Scheme { owner: Option<String>, value: Result<String, String>, changes: String },
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

#[cfg(test)]
mod tests {
    use super::*;

    fn image(width: u32, height: u32) -> Block {
        Block::Image { width, height, rgb: vec![0; (width * height * 3) as usize] }
    }

    #[test]
    fn every_block_but_an_image_is_one_row() {
        for b in [
            Block::Line(vec![TextSpan::plain("x")]),
            Block::Heading(vec![TextSpan::plain("x")]),
            Block::Field { label: "k".into(), value: vec![TextSpan::plain("v")] },
            Block::Rule,
        ] {
            assert_eq!(b.rows(80, 24), 1, "{b:?}");
        }
    }

    #[test]
    fn an_image_keeps_its_aspect_ratio_at_two_pixel_rows_per_cell() {
        // 40 wide over 80 columns of a 2:1 cell: 80 * (40/40) / 2.
        assert_eq!(image(40, 40).rows(80, 200), 40);
        assert_eq!(image(40, 20).rows(80, 200), 20);
        // A partial row still needs a whole cell.
        assert_eq!(image(40, 41).rows(80, 200), 41);
    }

    #[test]
    fn an_image_is_bounded_by_max_rows_and_never_vanishes_to_zero() {
        assert_eq!(image(40, 400).rows(80, 24), 24);
        assert_eq!(image(400, 1).rows(80, 24), 1);
    }

    #[test]
    fn a_degenerate_image_occupies_no_rows() {
        assert_eq!(image(40, 40).rows(0, 24), 0);
        assert_eq!(image(40, 40).rows(80, 0), 0);
        assert_eq!(image(0, 40).rows(80, 24), 0);
    }
}
