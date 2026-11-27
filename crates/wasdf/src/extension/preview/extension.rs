//! PreviewExtension (bundled): file preview — the reference "fat" function-panel
//! content extension. It owns everything content-specific: decoding read bytes
//! ([decode]), syntax highlighting ([highlight]), the less-style search matcher
//! ([search]), and its function-panel keybindings + `:when` predicates declared
//! in Scheme ([`scheme_source`]). The actual line/image painting is the kernel's
//! generic content blitter (`crate::ui::content`), shared by every content
//! extension; this extension just feeds it decoded content.
//!
//! The decoded content lives **inside this extension** (interior mutability):
//! core reads file bytes / dir entries off-thread and hands them to
//! [`accept_content`], which decodes + stashes; [`view`] later renders
//! from that stash. Core keeps no preview content — only the generic
//! function-panel geometry + (for key gating) the search match list it stores on
//! this extension's behalf.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use crate::core::{AppState, Block, ExtensionIntent, Intent, Mode, OptionStore, PanelView, Content, SelectSource, SpanStyle, TextSpan};
use crate::extension::{Extension, FunctionRenderCtx};
use crate::script::condition::Conditions;

// The detail submodules (decode / highlight / markdown / search) are declared by
// the `preview.rs` facade; from here they are siblings under `preview`.
use super::decode::{self, Decoded, Kind};
use super::search;

/// Lines below the viewport to keep loaded: once the scroll is within this many
/// lines of the loaded end, the next file chunk is fetched (so scrolling never
/// has to wait for a full-file read).
const LOOKAHEAD: usize = 256;

/// The extension's function-panel keymap, declared as Scheme data and evaluated
/// at registration. Bindings emit core function-panel intents; the `:when`
/// predicates `function-searching` / `function-has-matches` are registered below.
/// Layer priority (Extension > Core) makes these win over the core Enter/Esc/n
/// bindings while a search is active.
const KEYMAP_SCHEME: &str = include_str!("keymap.scm");
const OPTIONS_SCHEME: &str = include_str!("options.scm");

/// The preview extension. Holds its decoded content and the chunk-load
/// bookkeeping internally (main-thread `RefCell`; the worker never touches this
/// object). Content grows chunk by chunk as the cursor follows / the viewport
/// scrolls — the chunking lives entirely here, not in core.
#[derive(Default)]
pub struct PreviewExtension {
    content: RefCell<Option<Decoded>>,
    target: RefCell<Option<PathBuf>>,
    load: RefCell<Load>,
    /// Set once from boot via `bind_store`; read in `accept_content` for tab width.
    store: OnceLock<Arc<OptionStore>>,
    /// The view built from the stash, until the stash changes: its pointer
    /// identity is the content revision core caches its layout by.
    view: RefCell<Option<Arc<PanelView>>>,
}

/// The current window's chunk-load state: which file it covers, how it streams
/// (text vs hex; `None` once a whole-decoded image/dir), the next byte offset to
/// request, whether the file is fully loaded, and the undecoded trailing bytes
/// carried between chunks.
#[derive(Default)]
struct Load {
    path: Option<PathBuf>,
    kind: Option<Kind>,
    next_offset: u64,
    eof: bool,
    tail: Vec<u8>,
}

impl PreviewExtension {
    /// Fold one byte chunk into the window. Offset 0 starts a fresh window (the
    /// first chunk's MIME class fixes the kind; images decode whole); later chunks
    /// for the same path append `tail ++ bytes`, decoding the completed units and
    /// keeping the new tail.
    fn accept_chunk(&self, path: &Path, offset: u64, bytes: &[u8], eof: bool, tab_width: usize) {
        self.view.borrow_mut().take();
        let mut load = self.load.borrow_mut();
        let mut content = self.content.borrow_mut();
        if offset == 0 {
            let (decoded, kind) = decode::classify_first(path, bytes);
            *content = Some(decoded);
            *load = Load { path: Some(path.to_path_buf()), kind, eof: true, ..Default::default() };
            if kind.is_none() {
                return; // image/whole content: nothing to stream
            }
        } else if load.path.as_deref() != Some(path) {
            return; // a stale chunk for a path we no longer window
        }
        let Some(kind) = load.kind else { return };
        let mut buf = std::mem::take(&mut load.tail);
        buf.extend_from_slice(bytes);
        if let Some(dec) = content.as_mut() {
            load.tail = decode::append(path, kind, dec, &buf, eof, tab_width);
        }
        load.next_offset = offset + bytes.len() as u64;
        load.eof = eof;
    }

    /// The path whose content should follow the cursor: the File-mode entry while
    /// the panel is visible, or the moving file-search hit.
    fn cursor_target(&self, state: &AppState) -> Option<PathBuf> {
        // A form extension hosting the panel (form_owner) pauses content-follow, so
        // its own reads (e.g. the CmdOp listing) are not cancelled by the preview.
        if state.function.form_owner.is_some() {
            return None;
        }
        // In File mode, only follow while the panel is actually shown (a hidden
        // panel has nothing to preview into); Select's file-search always follows
        // — the dir-listing hint has no visibility toggle of its own.
        if matches!(state.mode(), Mode::File) && !state.function.visible {
            return None;
        }
        state.cursor_path()
    }

    /// Whether the panel is currently *following the cursor* — i.e. the panel is
    /// meant to preview whatever is under the cursor. Distinct from
    /// [`Self::cursor_target`] returning `Some`: here we are following even when
    /// there is nothing under the cursor (an empty directory), which is exactly
    /// the case where a stale preview must be dropped.
    fn following(&self, state: &AppState) -> bool {
        if state.function.form_owner.is_some() {
            return false; // a form owns the panel — not previewing the cursor
        }
        match state.mode() {
            Mode::File => state.function.visible,
            // Any Select sourced from an entry grid (deep file-search, a
            // shallow filter/refine — SEARCH_RENEW.md), not just file-search
            // by id: exactly the sources `AppState::cursor_path` reads a
            // `.view` from, so "following" and "has a cursor path" agree.
            Mode::Select(spec) => matches!(spec.source, SelectSource::FileWalk | SelectSource::Entries(_)),
            _ => false,
        }
    }

    /// Drop the stashed content so the panel renders nothing. Called when the
    /// cursor moves onto "no target" (empty directory) so the previous file's
    /// preview does not linger.
    fn clear(&self) {
        self.view.borrow_mut().take();
        *self.content.borrow_mut() = None;
        *self.target.borrow_mut() = None;
        *self.load.borrow_mut() = Load::default();
    }
}

impl Extension for PreviewExtension {
    fn id(&self) -> &str {
        "preview"
    }

    fn about(&self) -> crate::extension::About {
        crate::extension::About::new(
            "Preview",
            "File preview (text / hex / image / directory), syntax highlight, and less-style search",
        )
    }

    fn provides_function_content(&self) -> bool {
        true
    }

    fn register_conditions(&self, c: &mut Conditions) {
        c.register("function-searching", Box::new(|s| s.function.search.input_active));
        c.register("function-has-matches", Box::new(|s| !s.function.search.matches.is_empty()));
    }

    fn bind_store(&self, store: &Arc<OptionStore>) {
        let _ = self.store.set(store.clone());
    }

    fn scheme_source(&self) -> Option<String> {
        Some(KEYMAP_SCHEME.to_string())
    }

    fn option_scheme(&self) -> Option<String> {
        Some(OPTIONS_SCHEME.to_string())
    }

    /// Decode a read chunk (core fetched the bytes/entries off-thread) and fold it
    /// into the stash. The first chunk (offset 0) fixes the content kind and
    /// resets the window; later chunks append. Runs on the main thread.
    fn accept_content(&self, path: &Path, result: &Content) {
        let tab_width = self
            .store
            .get()
            .map(|s| s.get_u32("preview.tab-width") as usize)
            .unwrap_or(4)
            .max(1);
        match result {
            Content::Dir { entries } => {
                self.view.borrow_mut().take();
                *self.content.borrow_mut() = Some(Decoded::Dir { entries: entries.clone() });
                *self.load.borrow_mut() =
                    Load { path: Some(path.to_path_buf()), eof: true, ..Default::default() };
            }
            Content::Bytes { offset, bytes, eof } => {
                self.accept_chunk(path, *offset, bytes, *eof, tab_width)
            }
            Content::Value(_) => return, // never asked for one
        }
        *self.target.borrow_mut() = Some(path.to_path_buf());
    }

    /// Produce the stashed content as a logical [`PanelView`]; **core** lays it out
    /// (h-scroll, line numbers, search highlight, theming) and draws chrome. Text
    /// carries per-line syntax spans; hex/dir are plain/role-styled lines; a
    /// "… truncated" marker line is appended while more remains to stream.
    fn view(&self, _ctx: &FunctionRenderCtx) -> Option<Arc<PanelView>> {
        if let Some(v) = self.view.borrow().as_ref() {
            return Some(v.clone());
        }
        let stash = self.content.borrow();
        let decoded = stash.as_ref()?;
        let more = !self.load.borrow().eof;
        let view = match decoded {
            Decoded::Text { lines, spans } => {
                let mut blocks: Vec<Block> = lines
                    .iter()
                    .enumerate()
                    .map(|(i, line)| match spans.get(i) {
                        Some(sp) if !sp.is_empty() => Block::Line(sp.clone()),
                        _ => Block::Line(vec![TextSpan::plain(line.clone())]),
                    })
                    .collect();
                if more {
                    blocks.push(truncated_marker());
                }
                PanelView::Document(blocks)
            }
            Decoded::Binary { lines } => {
                let mut blocks: Vec<Block> =
                    lines.iter().map(|l| Block::Line(vec![TextSpan::plain(l.clone())])).collect();
                if more {
                    blocks.push(truncated_marker());
                }
                PanelView::Document(blocks)
            }
            Decoded::Markdown { blocks, .. } => {
                let mut blocks = blocks.clone();
                if more {
                    blocks.push(truncated_marker());
                }
                PanelView::Document(blocks)
            }
            Decoded::Image { width, height, rgb } => {
                PanelView::Image { width: *width, height: *height, rgb: rgb.clone() }
            }
            Decoded::Dir { entries } => {
                let blocks = entries
                    .iter()
                    .map(|e| {
                        let name = format!("{}{}", e.name, if e.is_dir { "/" } else { "" });
                        let style = if e.is_dir { SpanStyle::Role("dir".into()) } else { SpanStyle::Default };
                        Block::Line(vec![TextSpan::styled(name, style)])
                    })
                    .collect();
                PanelView::Document(blocks)
            }
        };
        let view = Arc::new(view);
        *self.view.borrow_mut() = Some(view.clone());
        Some(view)
    }

    /// `search` runs the matcher over the stashed content and returns the match
    /// set (+ a jump to the first) as generic intents the reducer stores. Match
    /// state lives in core because `:when` gates `n`/`p` on it.
    fn handle_intent(&self, intent: &ExtensionIntent, _state: &AppState) -> Vec<Intent> {
        if intent.intent != "search" {
            return Vec::new();
        }
        let query = intent.data.as_str().unwrap_or("");
        let stash = self.content.borrow();
        // Matches are logical — (block, byte range in its text) — so no panel
        // geometry is needed: text and hex have one block per line, markdown
        // one per block (an image has no text).
        let texts: Vec<String>;
        let lines: &[String] = match stash.as_ref() {
            Some(Decoded::Text { lines, .. }) | Some(Decoded::Binary { lines }) => lines,
            Some(Decoded::Markdown { blocks, .. }) => {
                texts = blocks.iter().map(|b| b.text().unwrap_or_default()).collect();
                &texts
            }
            _ => return Vec::new(),
        };
        vec![Intent::SetSearchMatches(search::find_matches(lines, query))]
    }

    /// Follow the cursor: when the function panel is visible in File mode, or the
    /// file-search candidate moves, load the target's content. This is the whole
    /// "cursor → preview" wiring, owned by the extension — the kernel only
    /// broadcasts the event and runs the returned `LoadContent`.
    fn on_cursor_changed(&self, state: &AppState) -> Vec<Intent> {
        let Some(path) = self.cursor_target(state) else {
            // Following the cursor but nothing is under it (e.g. an empty
            // directory): drop any stale preview so the panel stops showing the
            // previous file's content and title.
            if self.following(state) {
                self.clear();
            }
            return Vec::new();
        };
        let load = self.load.borrow();
        let offset = if load.path.as_deref() != Some(path.as_path()) {
            0 // a new target: load its first chunk
        } else if load.eof {
            return Vec::new(); // fully loaded already — no re-read
        } else {
            // Same file with more to load: fetch the next chunk only once the
            // scroll comes within LOOKAHEAD lines of what's decoded so far.
            let loaded = self.content.borrow().as_ref().map(content_lines).unwrap_or(0);
            if state.function.scroll.offset() + LOOKAHEAD < loaded {
                return Vec::new();
            }
            load.next_offset
        };
        vec![Intent::LoadContent { owner: self.id().to_string(), path, offset }]
    }
}

/// The trailing "… truncated" marker line appended while more content remains to
/// stream (core renders it muted, like any other muted span).
fn truncated_marker() -> Block {
    Block::Line(vec![TextSpan::styled("… truncated", SpanStyle::Role("muted".into()))])
}

/// The number of streamed lines decoded so far (text or hex); 0 for whole-decoded
/// content, which never pages. Markdown reports 0 as well — it renders from the
/// whole document, so its chunks must keep arriving regardless of the scroll
/// rather than waiting for the viewport to approach the loaded end.
fn content_lines(d: &Decoded) -> usize {
    match d {
        Decoded::Text { lines, .. } | Decoded::Binary { lines } => lines.len(),
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::ExtensionValue;

    fn state() -> AppState {
        AppState::new(PathBuf::from("/tmp"))
    }

    fn search(ext: &PreviewExtension, state: &AppState, query: &str) -> Vec<Intent> {
        ext.handle_intent(
            &ExtensionIntent {
                extension: "preview".into(),
                intent: "search".into(),
                data: ExtensionValue::String(query.into()),
            },
            state,
        )
    }

    fn matches(out: &[Intent]) -> Vec<(usize, usize, usize)> {
        out.iter()
            .find_map(|i| match i {
                Intent::SetSearchMatches(m) => Some(m.clone()),
                _ => None,
            })
            .unwrap_or_default()
    }

    #[test]
    fn plain_text_search_reports_matches_and_jumps_to_the_first() {
        let ext = PreviewExtension::default();
        let path = Path::new("/tmp/a.txt");
        ext.accept_content(
            path,
            &Content::Bytes { offset: 0, bytes: b"alpha\ncol a here\nbeta\n".to_vec(), eof: true },
        );
        let out = search(&ext, &state(), "col a");
        assert_eq!(matches(&out), vec![(1, 0, 5)]);
    }

    #[test]
    fn markdown_search_matches_the_rendered_text_not_the_source() {
        let ext = PreviewExtension::default();
        let path = Path::new("/tmp/a.md");
        // `**bold**` renders as `bold`: the asterisks are markup, not content.
        ext.accept_content(
            path,
            &Content::Bytes { offset: 0, bytes: b"intro\n\n**bold** tail\n".to_vec(), eof: true },
        );
        let state = state();
        assert!(matches(&search(&ext, &state, "**bold**")).is_empty());
        assert_eq!(matches(&search(&ext, &state, "bold")), vec![(2, 0, 4)]);
    }

    #[test]
    fn markdown_matches_name_their_block_whatever_an_image_occupies() {
        let ext = PreviewExtension::default();
        ext.accept_content(
            Path::new("/tmp/a.md"),
            &Content::Bytes { offset: 0, bytes: b"top\n\n![i](i.png)\n\nneedle\n".to_vec(), eof: true },
        );
        // blocks: 0 "top", 1 "", 2 the image, 3 "", 4 "needle" — the row it
        // lands on is core's to work out from the panel width.
        assert_eq!(matches(&search(&ext, &state(), "needle")), vec![(4, 0, 6)]);
    }

    #[test]
    fn the_view_is_shared_until_the_content_changes() {
        let p = PreviewExtension::default();
        let func = crate::core::state::FunctionPanelState::default();
        let theme = crate::core::Theme::default();
        let ctx = FunctionRenderCtx { width: 40, height: 10, focused: false, func: &func, sources: &[], theme: &theme };
        let path = std::path::Path::new("/tmp/a.txt");
        p.accept_content(path, &Content::Bytes { offset: 0, bytes: b"hello\n".to_vec(), eof: true });
        let a = p.view(&ctx).unwrap();
        assert!(Arc::ptr_eq(&a, &p.view(&ctx).unwrap()), "the same revision");
        p.accept_content(path, &Content::Bytes { offset: 0, bytes: b"world\n".to_vec(), eof: true });
        assert!(!Arc::ptr_eq(&a, &p.view(&ctx).unwrap()), "new content, new revision");
    }
}
