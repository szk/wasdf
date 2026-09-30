//! Extensions: the single Extension trait and the registry. The bundled list
//! below is the only place bundled extensions are named — the zero-edit
//! acceptance test (searching the tree for an extension id outside its own
//! directory matches only this list).

pub mod archive;
pub mod bookmarks;
pub mod loader;
pub mod lscolors;
pub mod preview;
pub mod pty;
pub mod repl;
pub mod registry;
pub mod settings;
pub mod theme; // bundled id: "theme"

use std::path::Path;
use std::sync::Arc;

use ratatui::style::Style;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::Line;

use crate::core::state::FunctionPanelState;
use crate::core::{AppState, Entry, ExtensionIntent, Intent, OptionStore, PanelView, Content, Theme};
use crate::script::condition::Conditions;
use crate::script::keymap::Binding;
use crate::services::command::CommandDef;
use crate::services::resolver::{ResolverEntry, ResolverChain};
use crate::vfs::VfsProvider;

pub use registry::ExtensionRegistry;

/// What core hands an extension when asking it for its function-panel content.
/// The extension reads `func` (its view state) and the viewport `width`×`height`
/// and returns a logical [`PanelView`]; core lays it out.
pub struct FunctionRenderCtx<'a> {
    pub width: u16,
    pub height: u16,
    pub focused: bool,
    pub func: &'a FunctionPanelState,
    /// The file panel's live operation targets (`AppState::targets`) — the current
    /// selection or cursor entry. A cross-panel form (the CmdOp builder) reads them
    /// to show the command's sources; content extensions ignore them.
    pub sources: &'a [std::path::PathBuf],
    pub theme: &'a Theme,
}

/// Core's **internal** laid-out drawable for the function panel: the product of
/// laying out a logical [`PanelView`] (see `ui/middle/function.rs`), consumed by
/// the panel blitter. It is no longer produced by extensions — they return a
/// `PanelView` and core owns all layout — but it remains the seam between the
/// layout step and the blit/chrome step: core vertically scrolls `lines` by
/// `scroll`, draws the border/`title`, a scrollbar (when `total` exceeds the
/// viewport), and an optional bottom-row `prompt` (the search input).
///
/// `total` (content rows) and `width` (content columns) are the laid-out content's
/// intrinsic dimensions; core pairs each with the viewport to bound scrolling and
/// discard over-scroll input.
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

/// An extension's About card — its display name, version, and a one-line
/// description — shown under `@/settings/Extensions/<id>` alongside its settings.
#[derive(Debug, Clone)]
pub struct About {
    /// Human-facing name (defaults to the id).
    pub name: String,
    /// Version string (defaults to the app version).
    pub version: String,
    /// One-line description of what the extension does.
    pub description: String,
}

impl About {
    /// An About card with the app version and the given name/description.
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        About {
            name: name.into(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            description: description.into(),
        }
    }
}

/// One interface, collected once at registration. Methods default to empty so
/// an extension declares only what it provides.
pub trait Extension: Send {
    fn id(&self) -> &str;

    /// The extension's About / version card, shown under its settings screen.
    /// Defaults to the id, the app version, and no description; extensions
    /// override to describe themselves.
    fn about(&self) -> About {
        About {
            name: self.id().to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            description: String::new(),
        }
    }

    /// Palette commands contributed by the extension.
    fn commands(&self) -> Vec<CommandDef> {
        Vec::new()
    }

    /// Keymaps (own modes and entry bindings into core modes), Extension layer.
    fn keymaps(&self) -> Vec<Binding> {
        Vec::new()
    }

    /// Called once after the option store is fully built (all extensions registered).
    /// Extensions that need to read their own options stash the store here; the
    /// default no-op leaves existing extensions unaffected.
    fn bind_store(&self, _store: &Arc<OptionStore>) {}

    /// Hand the extension a clone of the event-loop sender, so one that owns an
    /// I/O source of its own — a PTY reader thread, a file watcher — can ask for
    /// a repaint when its state changes out of band. Send `AppEvent::Tick`: the
    /// loop already treats it as "redraw, nothing changed", so this needs no new
    /// event, plan, or purpose.
    ///
    /// Bundled-only: `Sender<AppEvent>` does not cross the C ABI, so the dynamic
    /// loader never calls it. Called once at registration, beside `bind_store`.
    fn bind_events(&self, _tx: std::sync::mpsc::Sender<crate::core::AppEvent>) {}

    /// Called once at registration with the resolver chain, for extensions that
    /// build external command lines (option/skeleton lookup) — e.g. the CmdOp
    /// command builder, which has no `CmdCtx` inside `handle_intent`. Default no-op.
    fn bind_services(&self, _resolver: &Arc<ResolverChain>) {}

    /// The extension's declarative Scheme source, evaluated in the resident Scheme session
    /// at registration. The `what/when` of the extension — keymaps and the like
    /// declared as data — as opposed to `keymaps()`, the native `how`. Returns a
    /// quoted keymap-group form `(quote ((mode panel (binding…)) …))`, merged
    /// into the Extension layer (see EXTENSION.md, doc/SCHEME.md).
    fn scheme_source(&self) -> Option<String> {
        None
    }

    /// Scheme source declaring this extension's options. Returns a quoted list of
    /// `(option key category kind default label)` forms:
    ///
    /// ```scheme
    /// (quote (
    ///   (option "ext.tab-width" "Ext" (int 1 8) "4" "tab-width")))
    /// ```
    ///
    /// Each declared option is tagged with this extension's id and automatically
    /// appears under `@/settings/Extensions/<id>` (its own per-extension settings
    /// screen, generated dynamically), persists to `config.scm`, and is readable
    /// via the shared `OptionStore` (bundled) or a `wasdf_sdk::Settings` snapshot
    /// (dynamic) — with zero edits to core. Default `None` leaves no options.
    fn option_scheme(&self) -> Option<String> {
        None
    }

    /// Scheme code loaded into the settings store's session at registration,
    /// before `config.scm` runs: convenience procedures for users, built on the
    /// store procedures (doc/SCHEME.md, Preludes). It may only define; a name
    /// already taken is reported and that definition skipped.
    fn prelude(&self) -> Option<String> {
        None
    }

    /// Resolver entries appended to the chain.
    fn resolver_entries(&self) -> Vec<ResolverEntry> {
        Vec::new()
    }

    /// VFS providers contributed by this extension (its own mount, typically
    /// named after `id()`). Added after the embedded filesystem default, before
    /// the user layer.
    fn vfs_providers(&self) -> Vec<Box<dyn VfsProvider>> {
        Vec::new()
    }

    /// Per-entry color override for the file-list grid (PANEL_FILE and any
    /// path-valued Select sharing the same renderer). `None` means "no
    /// opinion" — the renderer falls back to its built-in dir/symlink/default
    /// coloring. `ExtensionRegistry::entry_style` takes the first `Some` in
    /// registration order (mirrors `provider()`'s first-wins contract).
    fn entry_style(&self, _entry: &Entry) -> Option<Style> {
        None
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
    /// mutability); `view` later renders from that stash. Runs on the
    /// main thread; core retains nothing.
    fn accept_content(&self, _path: &Path, _result: &Content) {}

    /// Produce this extension's function-panel content as a logical [`PanelView`]
    /// (sections / fields / lines of role- or color-styled spans). **Core owns the
    /// layout** — label alignment, line wrapping / h-scroll, vertical scroll, line
    /// numbers, theming, and search-match highlighting; the extension only
    /// describes *what* to show. `None` means "nothing to draw" (core shows a
    /// placeholder). This is the single pull-render path, replacing the old
    /// extension-assembled `FunctionDraw`.
    fn view(&self, _ctx: &FunctionRenderCtx) -> Option<PanelView> {
        None
    }

    /// Handle an extension intent addressed to this extension.
    fn handle_intent(&self, _intent: &ExtensionIntent, _state: &AppState) -> Vec<Intent> {
        Vec::new()
    }

    /// Draw an **interactive form** directly into the function panel's inner rect
    /// (the deferred per-panel-renderer surface, bundled-only — a `Frame` does not
    /// cross the ABI). Called by core when `form_owner` names this extension.
    /// Returns the panel title; core wraps the border/title/open-bottom chrome and
    /// owns the rect, so the interior is the extension's and the chrome is core's.
    /// The extension self-clamps any scroll it owns (it has `ctx` + interior
    /// mutability). `None` means "not hosting a form".
    fn render_form(&self, _frame: &mut Frame, _inner: Rect, _ctx: &FunctionRenderCtx) -> Option<String> {
        None
    }

    /// The scroll extent of the form this extension just drew, as
    /// `(total rows, position)`. Core turns it into the same scrollbar and the
    /// same post-draw scroll clamp every other function-panel frame gets, so a
    /// hosted form scrolls like the rest of the panel instead of being a
    /// special case. `None` (the default) means the form does not scroll.
    fn form_scroll(&self, _ctx: &FunctionRenderCtx) -> Option<(usize, usize)> {
        None
    }

    /// Subtrees grafted under `@/settings/<category>`, or onto this
    /// extension's own screen at `@/settings/Extensions/<id>` (anchor the
    /// node's "category" to `id()`, e.g. lscolors' Themes). The
    /// `SettingsProvider` appends one virtual dir per node under the matching
    /// listing and delegates any deeper path to the owning node. Default: none.
    fn settings_nodes(&self) -> Vec<Arc<dyn crate::extension::settings::SettingsNode>> {
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

    /// Called once per event-loop pass, idle ticks included. For an extension
    /// whose state can change with no intent behind it — a child process
    /// exiting, a watched file changing — this is the only place that change
    /// can become intents.
    ///
    /// `on_cursor_changed` cannot serve: it is deduplicated on the cursor
    /// signature, so it never fires when nothing the *user* did has changed.
    /// Keep this cheap and return nothing when there is nothing to say — it
    /// runs on every pass, for every extension.
    fn on_tick(&self, _state: &AppState) -> Vec<Intent> {
        Vec::new()
    }
}

/// Construct the bundled extensions. The only place they are named. (Settings
/// and theme are built separately in the kernel — they need a shared `Arc`
/// held on `App` itself for the per-frame `resolve_active()` read — and
/// registered there instead.) `config_dir` is handed to any extension that
/// persists its own state (bookmarks, lscolors) — the kernel only knows the
/// directory; the file name inside it is that extension's own business.
pub fn bundled(config_dir: Option<std::path::PathBuf>) -> Vec<Box<dyn Extension>> {
    vec![
        Box::new(preview::PreviewExtension::default()),
        Box::new(archive::ArchiveExtension),
        Box::new(pty::PtyExtension::new()),
        Box::new(bookmarks::BookmarksExtension::new(config_dir.clone())),
        Box::new(lscolors::LsColorsExtension::new()),
        Box::new(repl::ReplExtension::default()),
    ]
}
