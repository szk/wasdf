//! UiManager: render-only caches (notifications, computed size class) and the
//! render entry point. No field here is ever read by the reducer or a handler
//! to make a behavioral decision.

use std::time::{Duration, Instant};

use ratatui::Frame;

use crate::core::{AppState, Notice};
use crate::extension::ExtensionRegistry;
use crate::ui::middle::ScrollMemory;

const NOTICE_TTL: Duration = Duration::from_secs(4);

pub struct UiManager {
    notice: Option<(Notice, Instant)>,
    /// Persistent list scroll offsets, so the lists scroll only at the window
    /// edges (doc/UI.md). Render-only; never read by the reducer.
    scroll: ScrollMemory,
}

impl Default for UiManager {
    fn default() -> Self {
        UiManager::new()
    }
}

impl UiManager {
    pub fn new() -> Self {
        UiManager { notice: None, scroll: ScrollMemory::default() }
    }

    /// Queue a notification (only Failed/errors and op completions reach here).
    pub fn notify(&mut self, notice: Notice) {
        self.notice = Some((notice, Instant::now()));
    }

    /// The function panel's maximum scroll offset as measured at the last render
    /// (content rows − viewport rows). The kernel clamps `function.scroll` to this
    /// after each draw so over-scroll input is discarded rather than left to
    /// drift. `usize::MAX` (no scrollable frame drawn) leaves the scroll untouched.
    pub fn function_max_scroll(&self) -> usize {
        self.scroll.function_max()
    }

    /// The function panel's maximum *horizontal* scroll offset as measured at the
    /// last render (content columns − viewport columns). Same post-draw clamp
    /// contract as [`Self::function_max_scroll`].
    pub fn function_hmax_scroll(&self) -> usize {
        self.scroll.function_hmax()
    }

    /// The file-list geometry (columns × visual rows) measured at the last render.
    /// The kernel copies this into `AppState.list_geom` after each draw so the
    /// reducer can move the cursor in 2-D matching what is on screen.
    pub fn list_geom(&self) -> crate::core::ListGeom {
        self.scroll.list_geom()
    }

    /// The current live notice, if any (expired notices are hidden).
    pub fn live_notice(&self) -> Option<&Notice> {
        self.notice.as_ref().and_then(|(n, at)| {
            if at.elapsed() < NOTICE_TTL { Some(n) } else { None }
        })
    }

    /// Render the whole UI from the read-only state.
    pub fn render(&mut self, frame: &mut Frame, state: &AppState, extensions: &ExtensionRegistry) {
        // `live_notice` borrows `&self`; resolve it before the `&mut self.scroll`
        // borrow below.
        let notice = self.notice.as_ref().and_then(|(n, at)| {
            (at.elapsed() < NOTICE_TTL).then_some(n)
        });
        crate::ui::render::render(frame, state, extensions, &mut self.scroll, notice);
    }
}
