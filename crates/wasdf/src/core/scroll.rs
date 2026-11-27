//! `Scroll`: a one-dimensional scroll offset paired with its render-measured
//! upper bound. The single, reusable unit for every *kernel-owned* scroll in the
//! app — the function panel's vertical and horizontal content scroll, and the
//! CmdOp panel's option-label h-scroll and CLI-builder v-scroll.
//!
//! The offset is reducer-owned: handlers advance it blindly, since the pure
//! reducer cannot see the viewport. The render measures the content/viewport and
//! reports the bound back through the kernel, which calls [`Scroll::bind`] after
//! each draw — folding the bound in *and* clamping the offset, so over-scroll is
//! discarded rather than left to drift. This keeps the whole "blind advance →
//! post-draw clamp" discipline in one place instead of scattered `.min(max)`
//! sites and parallel `*_max` fields.

/// A scroll offset and its last render-measured maximum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Scroll {
    offset: usize,
    max: usize,
}

impl Scroll {
    /// The current offset (top visible row, or leftmost visible column).
    pub fn offset(&self) -> usize {
        self.offset
    }

    /// The last render-measured maximum offset (content − viewport).
    pub fn max(&self) -> usize {
        self.max
    }

    /// Whether the offset sits at or past the last page — no more content beyond.
    /// CmdOp uses this to know when a further step should leave a scroll region.
    pub fn at_end(&self) -> bool {
        self.offset >= self.max
    }

    /// Set the offset directly (e.g. bringing a search match into view).
    pub fn set(&mut self, offset: usize) {
        self.offset = offset;
    }

    /// Advance the offset by `n` (saturating); the post-draw [`bind`](Self::bind)
    /// discards any resulting over-scroll.
    pub fn advance(&mut self, n: usize) {
        self.offset = self.offset.saturating_add(n);
    }

    /// Retreat the offset by `n` (saturating at the top).
    pub fn retreat(&mut self, n: usize) {
        self.offset = self.offset.saturating_sub(n);
    }

    /// Jump to the top.
    pub fn go_top(&mut self) {
        self.offset = 0;
    }

    /// Jump to the bottom: a large offset the next [`bind`](Self::bind) clamps to
    /// the real last page (the reducer cannot see the viewport height).
    pub fn go_bottom(&mut self) {
        self.offset = usize::MAX / 2;
    }

    /// Fold in the render-measured bound and clamp the offset to it. `usize::MAX`
    /// means "no scrollable frame measured this draw — leave the offset untouched".
    pub fn bind(&mut self, max: usize) {
        if max != usize::MAX {
            self.max = max;
            if self.offset > max {
                self.offset = max;
            }
        }
    }
}
