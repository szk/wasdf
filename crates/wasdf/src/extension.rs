//! Extensions: the single Extension trait and the registry. The bundled list
//! below is the only place bundled extensions are named — the zero-edit
//! acceptance test (searching the tree for an extension id outside its own
//! directory matches only this list).

pub mod archive;
pub mod loader;
pub mod preview;
pub mod registry;

use std::path::Path;

use ratatui::text::Line;

use crate::core::state::FunctionPanelState;
use crate::core::{AppState, ExtensionIntent, Intent, ReadResult};
use crate::script::condition::Conditions;
use crate::script::keymap::Binding;
use crate::services::command::CommandDef;
use crate::services::resolver::ResolverEntry;

pub use registry::ExtensionRegistry;

/// What core hands an extension when asking it to render the function panel.
/// The extension reads `func` (its content + view state) and produces a
/// [`FunctionDraw`] sized for `width`×`height`.
pub struct FunctionRenderCtx<'a> {
    pub width: u16,
    pub height: u16,
    pub focused: bool,
    pub func: &'a FunctionPanelState,
}

/// The drawable an extension returns for the function panel. Core owns only the
/// chrome around it: it vertically scrolls `lines` by `scroll`, blits them,
/// draws the border/`title` and a scrollbar (when `total` exceeds the viewport),
/// and an optional bottom-row `prompt` (e.g. a search input). The extension owns
/// everything inside the lines (syntax color, search highlight, h-scroll, line
/// numbers).
///
/// `total` (content rows) and `width` (content columns) are the content's
/// intrinsic dimensions the extension reports — the lines themselves are already
/// windowed/h-scrolled, so core cannot recover them. Core pairs each with the
/// viewport size to bound scrolling and discard over-scroll input; the extension
/// owns the content, core owns the viewport and the kernel-owned offsets.
pub struct FunctionDraw {
    pub lines: Vec<Line<'static>>,
    pub title: String,
    pub scroll: usize,
    pub total: usize,
    /// Content width in columns (longest line). `0` for content that does not
    /// horizontally scroll (e.g. images).
    pub width: usize,
    pub prompt: Option<Line<'static>>,
}

/// One interface, collected once at registration. Methods default to empty so
/// an extension declares only what it provides.
pub trait Extension: Send {
    fn id(&self) -> &str;

    /// Palette commands contributed by the extension.
    fn commands(&self) -> Vec<CommandDef> {
        Vec::new()
    }

    /// Keymaps (own modes and entry bindings into core modes), Extension layer.
    fn keymaps(&self) -> Vec<Binding> {
        Vec::new()
    }

    /// The extension's declarative Scheme source, evaluated in the resident Scheme session
    /// at registration. The `what/when` of the extension — keymaps and the like
    /// declared as data — as opposed to `keymaps()`, the native `how`. Returns a
    /// quoted keymap-group form `(quote ((mode panel (binding…)) …))`, merged
    /// into the Extension layer (see EXTENSION.md, doc/SCHEME.md).
    fn scheme_source(&self) -> Option<String> {
        None
    }

    /// Resolver entries appended to the chain.
    fn resolver_entries(&self) -> Vec<ResolverEntry> {
        Vec::new()
    }

    /// Register `:when` predicates into the evaluator.
    fn register_conditions(&self, _conds: &mut Conditions) {}

    /// Whether this extension produces content for the function panel. When the
    /// cursor moves the kernel issues a `Plan::Read` for it and delivers the
    /// result to `accept_content`.
    fn provides_function_content(&self) -> bool {
        false
    }

    /// Receive the raw read result core fetched off-thread for `path` (a
    /// `Plan::Read` this extension's provider role triggered). The extension
    /// decodes it (MIME, highlight, …) and stashes it internally (interior
    /// mutability); `render_function` later renders from that stash. Runs on the
    /// main thread; core retains nothing.
    fn accept_content(&self, _path: &Path, _result: &ReadResult) {}

    /// Render this extension's function-panel content for the given viewport.
    /// The extension owns the drawing (windowing, syntax color, search
    /// highlight, h-scroll, line numbers); core only blits the result and draws
    /// chrome. `None` means "nothing to draw" (core shows a placeholder).
    fn render_function(&self, _ctx: &FunctionRenderCtx) -> Option<FunctionDraw> {
        None
    }

    /// Handle an extension intent addressed to this extension.
    fn handle_intent(&self, _intent: &ExtensionIntent, _state: &AppState) -> Vec<Intent> {
        Vec::new()
    }

    /// React to the cursor (or file-search selection) changing. The kernel calls
    /// this on **every** registered extension when the cursor target or panel
    /// visibility changes, and re-dispatches the returned intents. This is the
    /// generic, multi-subscriber cursor-changed event: the content provider emits
    /// `LoadContent` for the new path; other extensions may push their own content
    /// or react however they like. Read-only `state`; default is no reaction.
    fn on_cursor_changed(&self, _state: &AppState) -> Vec<Intent> {
        Vec::new()
    }
}

/// Construct the bundled extensions. The only place they are named.
pub fn bundled() -> Vec<Box<dyn Extension>> {
    vec![Box::new(preview::PreviewExtension::default()), Box::new(archive::ArchiveExtension)]
}
