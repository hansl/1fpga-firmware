//! Font-atlas warmup queue.
//!
//! JS code calls `gui.warmupGlyphs(family, sizes, chars)` from app
//! init to enqueue a list of (font, size, charset) requests. The
//! runtime drains the queue once between bundle evaluation and the
//! first frame and calls [`crate::font::FontRegistry::ensure`] for
//! each, so the atlas is fully populated before the user can trigger
//! any text node — eliminating the per-nav atlas-rebuild cost spike.

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use boa_macros::{Finalize, JsData, Trace};

#[derive(Default, Clone, Trace, Finalize, JsData)]
pub struct WarmupQueue {
    #[unsafe_ignore_trace]
    inner: Rc<RefCell<Vec<WarmupRequest>>>,
}

#[derive(Debug, Clone)]
pub struct WarmupRequest {
    pub family: String,
    pub px_size: u16,
    pub chars: HashSet<char>,
}

impl WarmupQueue {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&self, family: String, px_size: u16, chars: HashSet<char>) {
        self.inner.borrow_mut().push(WarmupRequest {
            family,
            px_size,
            chars,
        });
    }

    /// Take the current set of pending requests. Subsequent pushes
    /// land in a fresh list.
    pub fn drain(&self) -> Vec<WarmupRequest> {
        std::mem::take(&mut *self.inner.borrow_mut())
    }
}
