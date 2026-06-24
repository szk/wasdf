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
//! [`accept_content`], which decodes + stashes; [`render_function`] later renders
//! from that stash. Core keeps no preview content — only the generic
//! function-panel geometry + (for key gating) the search match list it stores on
//! this extension's behalf.

pub mod decode;
pub mod highlight;
pub mod search;

use crate::ui::content;

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use crate::core::{AppState, ExtensionIntent, Intent, Mode, ReadResult};
use crate::extension::{Extension, FunctionDraw, FunctionRenderCtx};
use crate::script::condition::Conditions;

use decode::{Decoded, Kind};

/// Lines below the viewport to keep loaded: once the scroll is within this many
/// lines of the loaded end, the next file chunk is fetched (so scrolling never
/// has to wait for a full-file read).
const LOOKAHEAD: usize = 256;

/// The extension's function-panel keymap, declared as Scheme data and evaluated
/// at registration. Bindings emit core function-panel intents; the `:when`
/// predicates `function-searching` / `function-has-matches` are registered below.
/// Layer priority (Extension > Core) makes these win over the core Enter/Esc/n
/// bindings while a search is active.
const KEYMAP_SCHEME: &str = include_str!("preview_keymap.scm");

/// The preview extension. Holds its decoded content and the chunk-load
/// bookkeeping internally (main-thread `RefCell`; the worker never touches this
/// object). Content grows chunk by chunk as the cursor follows / the viewport
/// scrolls — the chunking lives entirely here, not in core.
#[derive(Default)]
pub struct PreviewExtension {
    content: RefCell<Option<Decoded>>,
    target: RefCell<Option<PathBuf>>,
    load: RefCell<Load>,
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
    fn accept_chunk(&self, path: &Path, offset: u64, bytes: &[u8], eof: bool) {
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
            load.tail = decode::append(path, kind, dec, &buf, eof);
        }
        load.next_offset = offset + bytes.len() as u64;
        load.eof = eof;
    }

    /// The path whose content should follow the cursor: the File-mode entry while
    /// the panel is visible, or the moving file-search hit.
    fn cursor_target(&self, state: &AppState) -> Option<PathBuf> {
        match state.mode() {
            Mode::File if state.function.visible => state.current_entry().map(|e| e.path.clone()),
            Mode::Select(spec) if spec.id == "file-search" => state
                .select
                .as_ref()
                .and_then(|s| s.view.as_ref())
                .and_then(|v| v.current())
                .map(|e| e.path.clone()),
            _ => None,
        }
    }
}

impl Extension for PreviewExtension {
    fn id(&self) -> &str {
        "preview"
    }

    fn provides_function_content(&self) -> bool {
        true
    }

    fn register_conditions(&self, c: &mut Conditions) {
        c.register("function-searching", Box::new(|s| s.function.search.input_active));
        c.register("function-has-matches", Box::new(|s| !s.function.search.matches.is_empty()));
    }

    fn scheme_source(&self) -> Option<String> {
        Some(KEYMAP_SCHEME.to_string())
    }

    /// Decode a read chunk (core fetched the bytes/entries off-thread) and fold it
    /// into the stash. The first chunk (offset 0) fixes the content kind and
    /// resets the window; later chunks append. Runs on the main thread.
    fn accept_content(&self, path: &Path, result: &ReadResult) {
        match result {
            ReadResult::Dir { entries } => {
                *self.content.borrow_mut() = Some(Decoded::Dir { entries: entries.clone() });
                *self.load.borrow_mut() =
                    Load { path: Some(path.to_path_buf()), eof: true, ..Default::default() };
            }
            ReadResult::Bytes { offset, bytes, eof } => self.accept_chunk(path, *offset, bytes, *eof),
        }
        *self.target.borrow_mut() = Some(path.to_path_buf());
    }

    /// Render the stashed content for the viewport. The extension owns the
    /// drawing (syntax color, search highlight, h-scroll, line numbers, prompt);
    /// core only blits the returned [`FunctionDraw`]. Geometry + the match list
    /// come from `ctx.func`; the content from this extension's stash.
    fn render_function(&self, ctx: &FunctionRenderCtx) -> Option<FunctionDraw> {
        let stash = self.content.borrow();
        let decoded = stash.as_ref()?;
        let f = ctx.func;
        let target = self.target.borrow();
        let title = content::title(target.as_deref(), &f.search);
        let prompt = content::prompt_line(&f.search);
        // Clamp h-scroll to the content width so the text never scrolls off into
        // blank columns (the kernel applies the same bound to the stored hscroll
        // after the draw, so over-scroll input is discarded). Core owns the
        // viewport (`ctx.width`); this extension owns the content width.
        let hclamp = |w: usize| f.hscroll.min(w.saturating_sub(ctx.width as usize));
        // Text and hex stream in chunks: while not at EOF, mark "more below".
        let more = !self.load.borrow().eof;
        let (lines, scroll, width) = match decoded {
            Decoded::Text { lines, styles } => {
                let w = content::content_width(lines);
                (content::text_lines(lines, styles, &f.search, hclamp(w), f.show_line_numbers, more), f.scroll, w)
            }
            Decoded::Binary { lines } => {
                let w = content::content_width(lines);
                (content::text_lines(lines, &[], &f.search, hclamp(w), f.show_line_numbers, more), f.scroll, w)
            }
            // Images do not scroll (vertically or horizontally); they fill the viewport.
            Decoded::Image { width, height, rgb } => {
                (content::image_cells(*width, *height, rgb, ctx.width, ctx.height), 0, 0)
            }
            Decoded::Dir { entries } => (content::dir_lines(entries), f.scroll, 0),
        };
        let total = lines.len();
        Some(FunctionDraw { lines, title, scroll, total, width, prompt })
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
        let lines: &[String] = match stash.as_ref() {
            Some(Decoded::Text { lines, .. }) | Some(Decoded::Binary { lines }) => lines,
            _ => return Vec::new(),
        };
        let matches = search::find_matches(lines, query);
        let first = matches.first().map(|&(l, _, _)| l);
        let mut out = vec![Intent::SetSearchMatches(matches)];
        if let Some(line) = first {
            out.push(Intent::SetScroll(line));
        }
        out
    }

    /// Follow the cursor: when the function panel is visible in File mode, or the
    /// file-search candidate moves, load the target's content. This is the whole
    /// "cursor → preview" wiring, owned by the extension — the kernel only
    /// broadcasts the event and runs the returned `LoadContent`.
    fn on_cursor_changed(&self, state: &AppState) -> Vec<Intent> {
        let Some(path) = self.cursor_target(state) else { return Vec::new() };
        let load = self.load.borrow();
        let offset = if load.path.as_deref() != Some(path.as_path()) {
            0 // a new target: load its first chunk
        } else if load.eof {
            return Vec::new(); // fully loaded already — no re-read
        } else {
            // Same file with more to load: fetch the next chunk only once the
            // scroll comes within LOOKAHEAD lines of what's decoded so far.
            let loaded = self.content.borrow().as_ref().map(content_lines).unwrap_or(0);
            if state.function.scroll + LOOKAHEAD < loaded {
                return Vec::new();
            }
            load.next_offset
        };
        vec![Intent::LoadContent { owner: self.id().to_string(), path, offset }]
    }
}

/// The number of streamed lines decoded so far (text or hex); 0 for whole-decoded
/// content, which never pages.
fn content_lines(d: &Decoded) -> usize {
    match d {
        Decoded::Text { lines, .. } | Decoded::Binary { lines } => lines.len(),
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accept_then_render_draws_text_content() {
        let ext = PreviewExtension::default();
        ext.accept_content(
            Path::new("/x/notes.txt"),
            &ReadResult::Bytes { offset: 0, bytes: b"hello\nworld\n".to_vec(), eof: true },
        );
        let s = AppState::new(std::env::temp_dir());
        let ctx = FunctionRenderCtx { width: 40, height: 10, focused: true, func: &s.function };
        let draw = ext.render_function(&ctx).expect("draws the stashed content");
        assert_eq!(draw.title, "notes.txt");
        assert_eq!(draw.total, 2);
    }

    #[test]
    fn render_is_none_without_accepted_content() {
        let ext = PreviewExtension::default();
        let s = AppState::new(std::env::temp_dir());
        let ctx = FunctionRenderCtx { width: 40, height: 10, focused: true, func: &s.function };
        assert!(ext.render_function(&ctx).is_none());
    }

    #[test]
    fn search_intent_matches_over_the_stash() {
        let ext = PreviewExtension::default();
        ext.accept_content(
            Path::new("/x/a.txt"),
            &ReadResult::Bytes { offset: 0, bytes: b"foo\nbar foo\n".to_vec(), eof: true },
        );
        let s = AppState::new(std::env::temp_dir());
        let ei = ExtensionIntent {
            extension: "preview".into(),
            intent: "search".into(),
            data: crate::core::ExtensionValue::String("foo".into()),
        };
        match ext.handle_intent(&ei, &s).as_slice() {
            [Intent::SetSearchMatches(m), Intent::SetScroll(line)] => {
                assert_eq!(m.len(), 2);
                assert_eq!(*line, 0);
            }
            other => panic!("expected matches + jump, got {other:?}"),
        }
    }

    fn state_with_file(visible: bool) -> AppState {
        let mut s = AppState::new(std::path::PathBuf::from("/d"));
        s.entries = vec![crate::core::Entry {
            path: std::path::PathBuf::from("/d/a.txt"),
            name: "a.txt".into(),
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
        }];
        s.function.visible = visible;
        s
    }

    #[test]
    fn on_cursor_changed_loads_when_visible() {
        let ext = PreviewExtension::default();
        // Visible: the provider asks the kernel to load the cursor path's first chunk.
        match ext.on_cursor_changed(&state_with_file(true)).as_slice() {
            [Intent::LoadContent { owner, path, offset }] => {
                assert_eq!(owner, "preview");
                assert_eq!(path, std::path::Path::new("/d/a.txt"));
                assert_eq!(*offset, 0, "a new target loads from the start");
            }
            other => panic!("expected a LoadContent, got {other:?}"),
        }
        // Hidden: no follow.
        assert!(ext.on_cursor_changed(&state_with_file(false)).is_empty());
    }

    #[test]
    fn paging_requests_the_next_chunk_only_when_scrolled_near_the_end() {
        let ext = PreviewExtension::default();
        let path = Path::new("/d/a.txt");
        // A first chunk that is not EOF: 300 lines ("x\n" × 300 = 600 bytes).
        let first = b"x\n".repeat(300);
        let first_len = first.len() as u64;
        ext.accept_content(path, &ReadResult::Bytes { offset: 0, bytes: first, eof: false });
        let mut s = state_with_file(true);
        s.entries[0].path = path.to_path_buf();

        // Scroll well above the loaded end (300 lines) → no further read yet.
        s.function.scroll = 0;
        assert!(
            ext.on_cursor_changed(&s).is_empty(),
            "no paging while the viewport is far from the loaded end",
        );

        // Scroll within LOOKAHEAD of the loaded end → request the next chunk at
        // the byte offset where the first chunk ended.
        s.function.scroll = 300 - LOOKAHEAD + 1;
        match ext.on_cursor_changed(&s).as_slice() {
            [Intent::LoadContent { offset, .. }] => {
                assert_eq!(*offset, first_len, "next chunk at the byte where the first ended")
            }
            other => panic!("expected the next chunk, got {other:?}"),
        }

        // Once EOF arrives, paging stops.
        ext.accept_content(
            path,
            &ReadResult::Bytes { offset: first_len, bytes: b"end\n".to_vec(), eof: true },
        );
        s.function.scroll = 999;
        assert!(ext.on_cursor_changed(&s).is_empty(), "fully loaded → no re-read");
    }
}
