//! The UI-thread → engine-thread frame boundary.
//!
//! Dual-core split (see `engine.rs`): the UI thread (Boa + layout +
//! display-list build, pinned to core 1) produces one [`FramePacket`]
//! per UI tick; the engine thread (Device + input pump + paint,
//! pinned to core 0) consumes the LATEST one. The mailbox is
//! latest-wins by design — if the engine is mid-frame when two
//! packets arrive, the intermediate one is dropped, decoupling UI
//! cadence from paint cadence with natural frame skipping.
//!
//! Everything in a packet is plain owned data ([`DisplayList`] is
//! `Send` by construction — see `display_list.rs`).

use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use crate::display_list::DisplayList;
use crate::font::FontRegistry;
use crate::image::ImageRegistry;
use crate::text::{CacheKey, TextCache};
use menu_core_host::protocol::Rgba;

/// Font atlas coverage the frame needs: `(family, px_size, chars)`.
/// The engine runs `FontRegistry::ensure` for each (rasterize + atlas
/// upload happen device-side).
#[derive(Clone, Debug)]
pub struct FontNeed {
    pub family: String,
    pub px_size: u16,
    pub chars: HashSet<char>,
}

/// Image texture the frame needs. `sized` carries the explicit
/// layout box for the pre-resized variant (1:1 blit path).
#[derive(Clone, Debug)]
pub struct ImageNeed {
    pub src: String,
    pub sized: Option<(u16, u16)>,
}

/// Device-side work the engine must ensure before (or right after)
/// painting this frame. Misses are tolerated everywhere downstream:
/// a text/image op simply isn't in this packet's display list yet,
/// and the cache-generation bump makes the UI rebuild once the
/// resource lands.
/// Text line the frame needs cached as an RT. Carries the decoded
/// color (CacheKey stores it packed) so the engine can build the
/// `PendingRender` without unpacking.
#[derive(Clone, Debug)]
pub struct TextNeed {
    pub key: CacheKey,
    pub color: Rgba,
}

#[derive(Clone, Debug, Default)]
pub struct FrameRequests {
    pub fonts: Vec<FontNeed>,
    pub images: Vec<ImageNeed>,
    pub texts: Vec<TextNeed>,
}

/// UI-thread stage timings, carried along so the engine can log the
/// combined per-frame line in the familiar format.
#[derive(Clone, Copy, Debug, Default)]
pub struct UiTimings {
    pub jobs: Duration,
    pub input: Duration,
    pub text_prep: Duration,
    pub images: Duration,
    pub text_pop: Duration,
    pub layout: Duration,
    pub scene: Duration,
}

/// A hardware overlay plane's frame contribution: content (in
/// plane-local coordinates) and screen geometry, SEPARATELY hashed —
/// the engine re-renders the plane surface only when `scene_hash`
/// changes and turns a pure geometry change into a position-register
/// write (no frame submission at all).
#[derive(Clone, Debug)]
pub struct PlanePacket {
    pub z: u8,
    pub dl: DisplayList,
    /// Hash of `dl` content only (geometry excluded).
    pub scene_hash: u64,
    pub x: i32,
    pub y: i32,
    pub w: u16,
    pub h: u16,
    /// Hardware plane alpha (see PlaneDL::alpha) — geometry-class:
    /// changes are register writes, never re-renders.
    pub alpha: u8,
}

/// One UI tick's output.
#[derive(Clone, Debug)]
pub struct FramePacket {
    pub dl: DisplayList,
    /// Content-layer hash only (planes hash separately — see
    /// [`PlanePacket::scene_hash`]).
    pub scene_hash: u64,
    /// Overlay planes, highest-z first (v1: at most one — the
    /// hardware has a single plane).
    pub planes: Vec<PlanePacket>,
    pub requests: FrameRequests,
    pub ui: UiTimings,
}

/// Latest-wins single-slot mailbox with blocking receive.
#[derive(Default)]
pub struct Mailbox {
    slot: Mutex<Option<FramePacket>>,
    cv: Condvar,
}

impl Mailbox {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Replace whatever is queued (the engine only ever wants the
    /// newest frame).
    pub fn send(&self, pkt: FramePacket) {
        let mut slot = self.slot.lock().unwrap();
        *slot = Some(pkt);
        self.cv.notify_one();
    }

    /// Take the newest packet, waiting up to `timeout`. `None` on
    /// timeout — the engine uses that to keep its input pump and
    /// housekeeping (boxart animation, shutdown checks) ticking.
    pub fn recv_timeout(&self, timeout: Duration) -> Option<FramePacket> {
        let slot = self.slot.lock().unwrap();
        let (mut slot, _) = self
            .cv
            .wait_timeout_while(slot, timeout, |s| s.is_none())
            .unwrap();
        slot.take()
    }
}

/// Registries shared across the thread boundary. The ENGINE is the
/// only mutator (uploads, atlas builds, RT allocation); the UI locks
/// them briefly for reads (layout measure, display-list build).
/// `generation` bumps on every engine-side mutation — the UI treats a
/// change as "rebuild the display list, and re-run layout" (text
/// metrics may have appeared).
///
/// LOCK ORDER (deadlock freedom): `fonts` → `images` → `text_cache`.
/// Every site acquires in this order (skipping is fine, acquiring
/// backwards is not): engine ensure = fonts → images → text_cache;
/// UI layout = fonts → images; UI build = images → text_cache;
/// engine glyph render = fonts only. Keep it that way.
#[derive(Clone)]
pub struct SharedCaches {
    pub fonts: Arc<Mutex<FontRegistry>>,
    pub images: Arc<Mutex<ImageRegistry>>,
    pub text_cache: Arc<Mutex<TextCache>>,
    pub generation: Arc<AtomicU64>,
}

impl SharedCaches {
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    pub fn bump_generation(&self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
    }
}
