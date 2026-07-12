//! Engine thread: everything that touches the [`Device`] or must not
//! wait on a JS reconcile.
//!
//! Pinned to core 0 (where Linux routes IRQs by default), it owns:
//!  - the evdev input pump — events are forwarded to the UI thread
//!    over a channel, and this is where a future "forward directly to
//!    the active MiSTer core" fast path taps in, so input never
//!    queues behind a 10-25 ms Boa reconcile;
//!  - the `Device` (deliberately `!Send`): resource ensures (font
//!    atlas uploads, image uploads, text-RT allocation + glyph
//!    rendering), damage bookkeeping per FB slot, display-list
//!    replay, PRESENT/fence pacing, boxart-demo register animation,
//!    and the content coverage mask.
//!
//! It consumes [`FramePacket`]s from a latest-wins mailbox produced
//! by the UI thread (Boa + Taffy + display-list build, core 1). The
//! two cadences are decoupled: a slow reconcile never stops input
//! pumping; a slow paint drops intermediate UI frames naturally.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use tracing::info;

use menu_core_host::device::Device;
use menu_core_host::error::DeviceError;
use menu_core_host::protocol::Rect;

use crate::input::events::RawInputEvent;
use crate::input::pump::Pump;
use crate::runtime::damage::{self, PaintedScene};
use crate::runtime::packet::{FramePacket, Mailbox, SharedCaches};

/// Static engine-side configuration captured at spawn.
pub struct EngineConfig {
    pub boxart_demo: bool,
    pub content_mask_on: bool,
    pub fb_area: u64,
    pub full_paint_threshold: u64,
    /// Fence-wait ceiling (see run()'s rationale).
    pub timeout: Duration,
}

/// How long the engine sleeps in the mailbox when no packet is
/// pending. Bounds input-forwarding latency and keeps the boxart
/// register animation smooth; 2 ms costs ~nothing on an idle core.
const IDLE_TICK: Duration = Duration::from_millis(2);

const TIMING_LOG_PERIOD: u32 = 60;
const MAX_INFLIGHT_FRAMES: usize = 1;

/// Move-only carrier that lets the fully-constructed [`Device`] cross
/// into the engine thread. `Device` is auto-`!Send` because it holds
/// raw pointers into `/dev/mem` mmaps — but those mappings are
/// process-wide, not thread-bound. What actually requires care is
/// CONCURRENT use (the single-producer ring discipline), and this
/// module preserves that by construction: the device is created and
/// configured on the main thread, moved here by value BEFORE the
/// main thread's loop starts, and never touched by any other thread
/// again.
pub struct DeviceCarrier(pub Device);
// SAFETY: see above — ownership transfers wholesale before any
// concurrent access; the engine thread is the sole user thereafter.
unsafe impl Send for DeviceCarrier {}

/// Spawn the engine thread.
#[allow(clippy::too_many_arguments)]
pub fn spawn(
    device: DeviceCarrier,
    caches: SharedCaches,
    mailbox: Arc<Mailbox>,
    input_tx: Sender<RawInputEvent>,
    running: Arc<AtomicBool>,
    cfg: EngineConfig,
) -> std::thread::JoinHandle<Result<(), DeviceError>> {
    std::thread::Builder::new()
        .name("engine".into())
        .spawn(move || {
            // Core 0: input IRQ locality + everything device-paced.
            if let Some(ids) = core_affinity::get_core_ids()
                && let Some(core0) = ids.first()
            {
                core_affinity::set_for_current(*core0);
            }
            // The carrier crosses the closure boundary WHOLE (passing
            // it to a function is an opaque full-value use): RFC 2229
            // precise captures would otherwise narrow the capture to
            // the inner !Send Device field, defeating the carrier.
            let result = engine_main(device, &caches, &mailbox, &input_tx, &running, &cfg);
            // Make sure the UI thread unblocks and exits too.
            running.store(false, Ordering::SeqCst);
            if let Err(ref e) = result {
                tracing::error!("engine thread error: {e}");
            }
            result
        })
        .expect("spawn engine thread")
}

fn engine_main(
    device: DeviceCarrier,
    caches: &SharedCaches,
    mailbox: &Mailbox,
    input_tx: &Sender<RawInputEvent>,
    running: &AtomicBool,
    cfg: &EngineConfig,
) -> Result<(), DeviceError> {
    let DeviceCarrier(mut device) = device;
    let mut pump = Pump::open_all();
    let mut event_buf: Vec<RawInputEvent> = Vec::new();

    let mut boxart_anim: u64 = 0;
    // Stuck-flip warn throttle (see the loop-top check).
    let mut last_flip_warn = Instant::now();

    /// Hardware overlay plane state (v1: the single scanout plane).
    ///
    /// DOUBLE-BUFFERED: renders target the back surface while the
    /// scanout reads the front; the base-register flip is deferred
    /// until the render's fence lands (`flip_after`), so the beam
    /// never sees a mid-render surface. The first HW test showed why
    /// this is required, not a nicety: the tween re-renders two cards
    /// every frame and a window recenter redraws the whole strip
    /// (~40 ms) — single-buffered, the beam caught the punched-
    /// transparent-but-not-yet-redrawn state as card-body flicker.
    ///
    /// `scene`/`hash` are PER SURFACE (same pattern as the per-FB-slot
    /// records): each buffer diffs against what IT last held, so
    /// damage accumulated while it was front replays correctly when
    /// it becomes back. The first flip doubles as the enable (the
    /// scanout never reads uninitialised DDR3).
    /// A flip waiting on its render fence. Carries the position that
    /// MATCHES the pending content: on a carousel window recenter,
    /// content and position change together in one packet — applying
    /// the position immediately while the front still shows the old
    /// content would jump the cards sideways for a few frames and
    /// snap back at the flip. Position writes while a flip is pending
    /// therefore ride the flip.
    struct PendingFlip {
        fence: u32,
        idx: usize,
        x: i32,
        y: i32,
        /// Whether position must apply ATOMICALLY with this flip.
        /// True when the render moved the plane's local coordinate
        /// system (a recenter: damage covered most of the surface) —
        /// applying the new position against the old front content
        /// would jump the cards sideways. False for coordinate-
        /// preserving content changes (selection tween: two cards'
        /// damage), where position applies IMMEDIATELY per packet so
        /// the slide never quantises to fence cadence — that
        /// quantisation was the residual "flicker": the strip moved
        /// in 30-70 ms jolts while content-change flips paced it.
        coupled: bool,
        /// When the flip was armed — a healthy flip completes in
        /// milliseconds (blit-only fence); staying pending for a
        /// second means the fence is stuck, and that must be SAID in
        /// the log rather than reconstructed from a video.
        set_at: Instant,
    }
    struct PlaneHw {
        tex: [menu_core_host::texture::TextureHandle; 2],
        scene: [Option<PaintedScene>; 2],
        hash: [Option<u64>; 2],
        /// Surface the scanout reads (meaningful once `enabled`).
        front: usize,
        /// Last position actually written to the registers.
        x: i32,
        y: i32,
        enabled: bool,
        /// Last alpha written to the register (reset value 0xFF).
        alpha_reg: u8,
        flip_after: Option<PendingFlip>,
        /// A flip's base-register write only takes effect at the
        /// compositor's NEXT per-frame config latch — for up to a
        /// scanout frame the beam keeps reading the OLD front. The
        /// old front (the new back) must therefore not be rendered
        /// into until the compositor has provably latched: gated on
        /// (config_latch_target, vsync_target) — the latch counter is
        /// the EXACT signal (increments precisely at the latch); the
        /// vsync target is the fallback for bitstreams predating the
        /// counter (LAYER_DEBUG[31:17] stuck at zero there). Without
        /// this gate the beam photographs a mid-replay surface —
        /// HW test 11's one-frame white-card flashes.
        back_unsafe_until: Option<(u16, u32)>,
    }
    let mut plane_hw: Option<PlaneHw> = None;

    fn plane_pos_regs(device: &mut menu_core_host::device::Device, x: i32, y: i32) {
        device.set_plane_pos(
            x.clamp(i16::MIN as i32, i16::MAX as i32) as i16,
            y.clamp(i16::MIN as i32, i16::MAX as i32) as i16,
        );
    }

    // Per-FB-slot damage bookkeeping (see run()'s original comment:
    // the three slots are distinct physical buffers; each diffs
    // against ITS OWN prior snapshot).
    let mut scene_hash_per_fb: [Option<u64>; 3] = [None, None, None];
    let mut scene_per_fb: [Option<PaintedScene>; 3] = [None, None, None];
    let mut pending_fences: std::collections::VecDeque<(u32, Instant)> =
        std::collections::VecDeque::with_capacity(3);

    let mut coverage_ring = [menu_core_host::mask::CoverageMask::empty(); 3];
    let mut coverage_frame: usize = 0;
    let mut content_mask_enabled = false;

    // Timing accumulators — the combined log line keeps the familiar
    // format; UI-side stages come from the packet.
    let mut frame_idx: u32 = 0;
    let mut t_jobs = Duration::ZERO;
    let mut t_input = Duration::ZERO;
    let mut t_text_prep = Duration::ZERO;
    let mut t_images = Duration::ZERO;
    let mut t_text_pop = Duration::ZERO;
    let mut t_layout = Duration::ZERO;
    let mut t_scene = Duration::ZERO;
    let mut t_ensure = Duration::ZERO;
    let mut t_paint = Duration::ZERO;
    let mut t_fence = Duration::ZERO;
    let mut t_fpga = Duration::ZERO;
    let mut t_fpga_max = Duration::ZERO;
    let mut sum_paint_rect_count: u64 = 0;
    let mut sum_paint_area_px: u64 = 0;
    let mut count_full_paints: u32 = 0;

    while running.load(Ordering::SeqCst) {
        // ---- Input first, always -----------------------------------
        // This is the latency path. Draining evdev and forwarding
        // costs microseconds and never waits on paint or JS. (The
        // future MiSTer-core direct-forward hook belongs right here.)
        event_buf.clear();
        pump.drain(&mut event_buf);
        for ev in event_buf.drain(..) {
            // UI thread gone = shutting down; loop exits via `running`.
            let _ = input_tx.send(ev);
        }

        // ---- Boxart demo: pure register animation ------------------
        if cfg.boxart_demo {
            const PERIOD: u64 = 240;
            let phase = boxart_anim % PERIOD;
            let tri = if phase < PERIOD / 2 { phase } else { PERIOD - phase };
            let frac = tri as f32 / (PERIOD / 2) as f32;
            let x_off = 1920.0_f32;
            let x_on = (1920 - 256 - 40) as f32;
            let x = (x_off - frac * (x_off - x_on)).round() as i16;
            device.set_boxart_pos(x, 400);
            boxart_anim = boxart_anim.wrapping_add(1);
        }

        // ---- Deferred plane flip ------------------------------------
        // Back-surface render fence reached → flip the scanout to it
        // (BASE/SIZE/STRIDE are frame-latched together by the
        // compositor, so the write is tear-free) and enable on the
        // first flip. Runs every loop tick so it fires even when no
        // further packets arrive.
        if let Some(hw) = plane_hw.as_mut()
            && let Some(fl) = &hw.flip_after
            && !device.fence_reached(fl.fence)
            && fl.set_at.elapsed() > Duration::from_secs(1)
            && last_flip_warn.elapsed() > Duration::from_secs(2)
        {
            last_flip_warn = Instant::now();
            tracing::warn!(
                "plane flip stuck: fence {} pending for {:.1?} (surface {})",
                fl.fence,
                fl.set_at.elapsed(),
                fl.idx
            );
        }
        if let Some(hw) = plane_hw.as_mut()
            && let Some(fl) = &hw.flip_after
            && device.fence_reached(fl.fence)
        {
            let (idx, fx, fy, coupled) = (fl.idx, fl.x, fl.y, fl.coupled);
            // Coupled flips write position + surface in the same
            // compositor frame-latch generation (the render moved the
            // plane's coordinate system); uncoupled flips already
            // applied position per packet.
            if coupled {
                plane_pos_regs(&mut device, fx, fy);
                hw.x = fx;
                hw.y = fy;
            }
            if let Err(e) = device.set_plane_surface(&hw.tex[idx]) {
                tracing::error!("plane flip failed: {e}");
            }
            hw.front = idx;
            hw.flip_after = None;
            // The freed buffer stays on-beam until the next config
            // latch — see back_unsafe_until.
            hw.back_unsafe_until = Some((
                device.config_latch_count().wrapping_add(1) & 0x7FFF,
                device.vsync_count().wrapping_add(1),
            ));
            if !hw.enabled {
                device.set_plane_enabled(true);
                hw.enabled = true;
            }
        }

        // ---- Latest UI frame (or housekeeping tick) -----------------
        let Some(pkt) = mailbox.recv_timeout(IDLE_TICK) else {
            continue;
        };

        // ---- Overlay plane: registers first, render below ----------
        // A pure move (slide tween on a LayerPortal) is JUST the
        // register writes here — no frame submission, no blits, no
        // fence. Content changes fold into the frame built further
        // down (plane surface re-render under plane-local damage).
        let plane_pkt = pkt.planes.first();
        let mut plane_render = false;
        // Surface the render below targets: the pending flip's target
        // (keep accumulating into it; its fence just moves forward) or
        // the non-front buffer.
        let mut plane_back = 0usize;
        match plane_pkt {
            Some(p) => {
                let need_alloc = match &plane_hw {
                    Some(hw) => hw.tex[0].width != p.w || hw.tex[0].height != p.h,
                    None => true,
                };
                if need_alloc {
                    // The pool is a bump allocator — a size change
                    // leaks the old surfaces. Fine for the intended
                    // use (static-size portals); log so churn is
                    // visible.
                    if plane_hw.is_some() {
                        tracing::warn!(
                            "overlay plane resized to {}x{} (old surfaces leaked)",
                            p.w,
                            p.h
                        );
                        device.set_plane_enabled(false);
                    }
                    match device
                        .create_render_texture(p.w, p.h)
                        .and_then(|a| device.create_render_texture(p.w, p.h).map(|b| [a, b]))
                    {
                        Ok(tex) => {
                            plane_hw = Some(PlaneHw {
                                tex,
                                scene: [None, None],
                                hash: [None, None],
                                front: 0,
                                x: i32::MIN,
                                y: i32::MIN,
                                enabled: false,
                                alpha_reg: 0xFF,
                                flip_after: None,
                                back_unsafe_until: None,
                            });
                        }
                        Err(e) => {
                            // Degradation gap: the subtree was
                            // partitioned OUT of the content list, so
                            // it simply won't show. Only reachable on
                            // texture-pool exhaustion.
                            tracing::error!("plane surface alloc failed: {e}");
                        }
                    }
                }
                if let Some(hw) = plane_hw.as_mut() {
                    // Alpha is geometry-class: frame-latched with the
                    // rest of the config, coordinate-independent —
                    // always an immediate register write. This is the
                    // whole point of the PLANE_ALPHA primitive: a
                    // portal fade is zero blits.
                    if hw.alpha_reg != p.alpha {
                        device.set_plane_alpha(p.alpha);
                        hw.alpha_reg = p.alpha;
                    }
                    plane_back = match &hw.flip_after {
                        Some(fl) => fl.idx,
                        None if hw.enabled => 1 - hw.front,
                        // Not yet on screen: keep filling the first
                        // surface until its flip enables us.
                        None => hw.front,
                    };
                    plane_render = hw.hash[plane_back] != Some(p.scene_hash);
                    // New content while a flip is pending: COMPLETE the
                    // pending flip first (bounded wait — its frame was
                    // already submitted) and render into the freed
                    // surface. Stacking renders onto the pending back
                    // instead replaces the fence every packet, so under
                    // a sustained tween the loop-top check never sees a
                    // landed fence — flips starve, the front freezes on
                    // stale content (HW test 3: ring on the wrong card,
                    // animation 'not running' until input stopped).
                    if plane_render && hw.flip_after.is_some() {
                        let fl = hw.flip_after.take().expect("just checked");
                        device.wait_fence(fl.fence, cfg.timeout)?;
                        if fl.coupled {
                            plane_pos_regs(&mut device, fl.x, fl.y);
                            hw.x = fl.x;
                            hw.y = fl.y;
                        }
                        if let Err(e) = device.set_plane_surface(&hw.tex[fl.idx]) {
                            tracing::error!("plane flip failed: {e}");
                        }
                        hw.front = fl.idx;
                        hw.back_unsafe_until = Some((
                            device.config_latch_count().wrapping_add(1) & 0x7FFF,
                            device.vsync_count().wrapping_add(1),
                        ));
                        if !hw.enabled {
                            device.set_plane_enabled(true);
                            hw.enabled = true;
                        }
                        plane_back = 1 - hw.front;
                        plane_render = hw.hash[plane_back] != Some(p.scene_hash);
                    }
                    if let Some(fl) = hw.flip_after.as_mut() {
                        fl.x = p.x;
                        fl.y = p.y;
                        if !fl.coupled && (hw.x != p.x || hw.y != p.y) {
                            // In-flight content is coordinate-
                            // preserving: the slide applies NOW.
                            plane_pos_regs(&mut device, p.x, p.y);
                            hw.x = p.x;
                            hw.y = p.y;
                        }
                    } else if !plane_render && (hw.x != p.x || hw.y != p.y) {
                        // Pure move, nothing in flight: registers now.
                        plane_pos_regs(&mut device, p.x, p.y);
                        hw.x = p.x;
                        hw.y = p.y;
                    }
                    // (plane_render with no pending flip: the render
                    // block below decides — immediate pos for small
                    // damage, ride-the-flip for a recenter.)
                    // Re-enable after a disable, when the front still
                    // holds this exact content: registers only.
                    if !hw.enabled
                        && hw.flip_after.is_none()
                        && hw.hash[hw.front] == Some(p.scene_hash)
                    {
                        plane_pos_regs(&mut device, p.x, p.y);
                        hw.x = p.x;
                        hw.y = p.y;
                        if device.set_plane_surface(&hw.tex[hw.front]).is_ok() {
                            device.set_plane_enabled(true);
                            hw.enabled = true;
                        }
                    }
                }
            }
            None => {
                if let Some(hw) = plane_hw.as_mut()
                    && hw.enabled
                {
                    device.set_plane_enabled(false);
                    hw.enabled = false;
                    // Content/hash records stay — re-enabling with
                    // unchanged content is registers-only.
                }
            }
        }

        // ---- Ensure device-side resources the frame asked for ------
        // Any actual mutation bumps the cache generation so the UI
        // rebuilds (and re-runs layout — text metrics may have just
        // appeared).
        let t_ensure_start = Instant::now();
        let mut mutated = false;
        let pendings = {
            let mut fonts = caches.fonts.lock().unwrap();
            for need in &pkt.requests.fonts {
                let existed = fonts.get(&need.family, need.px_size).is_some();
                match fonts.ensure(&mut device, &need.family, need.px_size, &need.chars) {
                    Ok(_) => {
                        if !existed {
                            mutated = true;
                        }
                    }
                    Err(e) => tracing::warn!(
                        "font ensure failed for {}@{}: {e}",
                        need.family,
                        need.px_size
                    ),
                }
            }
            {
                let mut images = caches.images.lock().unwrap();
                for need in &pkt.requests.images {
                    let had = images.get(&need.src).is_some();
                    let _ = images.get_or_load(&mut device, &need.src);
                    if !had && images.get(&need.src).is_some() {
                        mutated = true;
                    }
                    if let Some((w, h)) = need.sized {
                        let had_sized = images.get_sized(&need.src, w, h).is_some();
                        images.ensure_sized(&mut device, &need.src, w, h);
                        if !had_sized && images.get_sized(&need.src, w, h).is_some() {
                            mutated = true;
                        }
                    }
                }
            }
            let needs: Vec<_> = pkt
                .requests
                .texts
                .iter()
                .map(|t| (t.key.clone(), t.color))
                .collect();
            let mut text_cache = caches.text_cache.lock().unwrap();
            match text_cache.ensure_keys(&needs, &fonts, &mut device) {
                Ok(p) => p,
                Err(e) => {
                    tracing::warn!("text RT ensure failed: {e}");
                    Vec::new()
                }
            }
        };
        if !pendings.is_empty() {
            mutated = true;
        }
        if mutated {
            caches.bump_generation();
        }
        let ensure_dt = t_ensure_start.elapsed();

        // ---- Fence pacing + render_idx trust (moved verbatim) -------
        let fence_wait_start = Instant::now();
        let mut frame_fpga_dt = Duration::ZERO;
        if pending_fences.len() >= MAX_INFLIGHT_FRAMES {
            let (oldest_fence, oldest_submit) = pending_fences.pop_front().unwrap();
            device.wait_fence(oldest_fence, cfg.timeout)?;
            frame_fpga_dt = oldest_submit.elapsed();
        }
        while let Some(&(f, _)) = pending_fences.front() {
            if device.fence_reached(f) {
                pending_fences.pop_front();
            } else {
                break;
            }
        }
        let fence_dt = fence_wait_start.elapsed();

        // render_idx trust: see run()'s original comment. All fences
        // drained ⇒ the FPGA consumed our last PRESENT ⇒ fresh.
        let render_trusted = pending_fences.is_empty();
        let render_idx = (device.fb_state().render as usize).min(2);

        let t_paint_start = Instant::now();

        // ---- Plane surface re-render: OWN submission + fence --------
        // Same damage machinery as the FB path, but the target is the
        // plane's BACK surface and the submission carries NO PRESENT:
        // its fence fires when the blits complete (a few ms), not
        // after present/vsync — so the flip (and therefore the tween
        // cadence during animations) is decoupled from the display
        // frame rate. Ring execution is FIFO, so the pending-text
        // renders here are also visible to the content replay below.
        let mut texts_rendered = false;
        if plane_render
            && let (Some(p), Some(hw)) = (plane_pkt, plane_hw.as_mut())
        {
            let new_scene = p.dl.to_scene();
            let plan: Option<Vec<damage::PixelRect>> = match &hw.scene[plane_back] {
                Some(prev) => Some(damage::compute_damage(prev, &new_scene)),
                None => None, // fresh surface → full render
            };
            // Coupled = the render rewrote most of the surface (a
            // recenter shifted local coordinates, or first render):
            // position must wait for the flip. Small damage preserves
            // coordinates: position applies immediately below.
            let plane_area = (p.w as u64) * (p.h as u64);
            let coupled = match &plan {
                Some(rects) => damage::total_area(rects) * 2 > plane_area,
                None => true,
            };
            if !coupled && (hw.x != p.x || hw.y != p.y) {
                plane_pos_regs(&mut device, p.x, p.y);
                hw.x = p.x;
                hw.y = p.y;
            }
            // Vsync gate: the surface we are about to render into may
            // still be ON-BEAM (a flip's base write only latches at
            // the next scanout frame). Wait for the counter to pass
            // the flip's mark — at most one display frame, and it
            // correctly caps plane updates at the display rate.
            if let Some((latch_target, vsync_target)) = hw.back_unsafe_until.take() {
                let start = Instant::now();
                loop {
                    // Exact: the config-latch counter reached the
                    // flip's mark (15-bit wrap-safe forward compare).
                    let latch_ok = (device.config_latch_count().wrapping_sub(latch_target)
                        & 0x7FFF)
                        < 0x4000;
                    // Fallback for pre-counter bitstreams.
                    let vsync_ok =
                        (device.vsync_count().wrapping_sub(vsync_target) as i32) >= 0;
                    if latch_ok || vsync_ok {
                        break;
                    }
                    if start.elapsed() > Duration::from_millis(40) {
                        tracing::warn!(
                            "flip-latch gate timed out (latch {latch_target}, vsync {vsync_target}); rendering anyway"
                        );
                        break;
                    }
                    std::thread::sleep(Duration::from_micros(300));
                }
            }
            let pf = device.begin_frame();
            let pf = {
                let fonts = caches.fonts.lock().unwrap();
                crate::paint::render_pending_text(pf, &pendings, &fonts)?
            };
            texts_rendered = true;
            let mut f = pf.set_target(&hw.tex[plane_back])?;
            {
                let text_cache_l = caches.text_cache.lock().unwrap();
                match plan {
                    Some(rects) => {
                        for r in &rects {
                            let clip_rect: Rect = (*r).into();
                            let ff = f.set_clip(clip_rect)?;
                            let ff = crate::display_list::replay(
                                &p.dl, Some(clip_rect), &text_cache_l, ff,
                            )?;
                            f = ff.clear_clip()?;
                        }
                    }
                    None => {
                        f = crate::display_list::replay(&p.dl, None, &text_cache_l, f)?;
                    }
                }
            }
            let ptok = f.submit()?;
            hw.scene[plane_back] = Some(new_scene);
            hw.hash[plane_back] = Some(p.scene_hash);
            hw.flip_after = Some(PendingFlip {
                fence: ptok.fence_value(),
                idx: plane_back,
                x: p.x,
                y: p.y,
                coupled,
                set_at: Instant::now(),
            });
        }

        // Per-slot skip: this FB already holds exactly this scene and
        // there is no glyph work to flush. Plane-only packets (moves
        // AND content re-renders — submitted above) take this path
        // without touching the content framebuffer.
        if render_trusted
            && pendings.is_empty()
            && scene_hash_per_fb[render_idx] == Some(pkt.scene_hash)
        {
            accumulate_ui(&pkt, &mut t_jobs, &mut t_input, &mut t_text_prep,
                          &mut t_images, &mut t_text_pop, &mut t_layout, &mut t_scene);
            t_ensure += ensure_dt;
            t_fence += fence_dt;
            continue;
        }

        let current_scene = pkt.dl.to_scene();

        // ---- Paint ---------------------------------------------------
        let frame = device.begin_frame();
        let frame = if texts_rendered {
            frame
        } else {
            let fonts = caches.fonts.lock().unwrap();
            crate::paint::render_pending_text(frame, &pendings, &fonts)?
        };

        let frame = frame.set_target_framebuffer()?;

        let paint_rect_count: u32;
        let paint_area_px: u64;
        let mut paint_full = false;

        let damage_paint_plan: Option<Vec<damage::PixelRect>> =
            match &scene_per_fb[render_idx] {
                Some(prev) if render_trusted => {
                    let d = damage::compute_damage(prev, &current_scene);
                    let area = damage::total_area(&d);
                    if d.is_empty() || area > cfg.full_paint_threshold {
                        None
                    } else {
                        Some(d)
                    }
                }
                // Untrusted render_idx → full paint (slot-agnostic).
                _ => None,
            };
        // Text ops resolve against the cache AT REPLAY (the ensure
        // above already landed this packet's RTs, so brand-new text
        // paints in this same frame — no one-frame blank). Lock order:
        // text_cache is last, and fonts/images are not held here.
        let text_cache_l = caches.text_cache.lock().unwrap();
        let frame = match damage_paint_plan {
            Some(rects) => {
                paint_rect_count = rects.len() as u32;
                paint_area_px = damage::total_area(&rects);
                let mut frame = frame;
                for r in &rects {
                    let clip_rect: Rect = (*r).into();
                    let f = frame.set_clip(clip_rect)?;
                    let f = crate::display_list::replay(
                        &pkt.dl, Some(clip_rect), &text_cache_l, f,
                    )?;
                    frame = f.clear_clip()?;
                }
                frame
            }
            None => {
                paint_full = true;
                paint_rect_count = 1;
                paint_area_px = cfg.fb_area;
                crate::display_list::replay(&pkt.dl, None, &text_cache_l, frame)?
            }
        };
        drop(text_cache_l);

        let paint_dt = t_paint_start.elapsed();

        let new_token = frame.present()?.submit()?;
        pending_fences.push_back((new_token.fence_value(), Instant::now()));

        // ---- Content coverage mask (unchanged policy) ----------------
        if cfg.content_mask_on {
            let mut cov = menu_core_host::mask::CoverageMask::empty();
            for e in &pkt.dl.entries {
                cov.mark_rect(
                    e.item.bbox.x as i32,
                    e.item.bbox.y as i32,
                    e.item.bbox.w as u32,
                    e.item.bbox.h as u32,
                );
            }
            coverage_ring[coverage_frame % 3] = cov;
            coverage_frame += 1;
            let mut union = coverage_ring[0];
            union.union(&coverage_ring[1]);
            union.union(&coverage_ring[2]);
            device.upload_content_mask(union.words())?;
            if !content_mask_enabled {
                device.set_content_mask(true);
                content_mask_enabled = true;
                info!(
                    "content coverage mask enabled ({} tiles)",
                    union.covered_tiles()
                );
            }
        }

        if render_trusted {
            scene_hash_per_fb[render_idx] = Some(pkt.scene_hash);
            scene_per_fb[render_idx] = Some(current_scene);
        }
        // else: target slot unknown — records stay conservative.

        // ---- Timing bookkeeping --------------------------------------
        accumulate_ui(&pkt, &mut t_jobs, &mut t_input, &mut t_text_prep,
                      &mut t_images, &mut t_text_pop, &mut t_layout, &mut t_scene);
        t_ensure += ensure_dt;
        t_paint += paint_dt;
        t_fence += fence_dt;
        t_fpga += frame_fpga_dt;
        if frame_fpga_dt > t_fpga_max {
            t_fpga_max = frame_fpga_dt;
        }
        sum_paint_rect_count += paint_rect_count as u64;
        sum_paint_area_px += paint_area_px;
        if paint_full {
            count_full_paints += 1;
        }

        frame_idx = frame_idx.wrapping_add(1);
        if frame_idx % TIMING_LOG_PERIOD == 0 {
            let n = TIMING_LOG_PERIOD;
            let avg = |t: Duration| t.as_micros() as u32 / n;
            let total = t_jobs + t_input + t_text_prep + t_images + t_text_pop
                + t_layout + t_scene + t_ensure + t_paint + t_fence;
            // NB: with the dual-core split, `cpu` is the sum of BOTH
            // threads' busy time — the wall-clock frame cost is now
            // max(ui, engine) instead of the sum. `ensure` replaces
            // the old text_prep/images/text_pop device time (those
            // now measure only the UI-side walks).
            let cpu = total.saturating_sub(t_fence);
            info!(
                "frame timings (us avg over {n}): jobs={} input={} text_prep={} images={} text_pop={} layout={} scene={} ensure={} paint={} fence={} fpga={} fpga_max={} cpu={} total={}",
                avg(t_jobs), avg(t_input), avg(t_text_prep), avg(t_images),
                avg(t_text_pop), avg(t_layout), avg(t_scene), avg(t_ensure),
                avg(t_paint), avg(t_fence), avg(t_fpga),
                t_fpga_max.as_micros() as u32, avg(cpu), avg(total),
            );
            let pct = |area: u64| {
                if cfg.fb_area == 0 {
                    0u32
                } else {
                    ((area * 100) / (cfg.fb_area * n as u64)) as u32
                }
            };
            info!(
                "damage avg over {n}: rects={:.1} area={}% full={}",
                (sum_paint_rect_count as f32) / (n as f32),
                pct(sum_paint_area_px),
                count_full_paints,
            );
            t_jobs = Duration::ZERO;
            t_input = Duration::ZERO;
            t_text_prep = Duration::ZERO;
            t_images = Duration::ZERO;
            t_text_pop = Duration::ZERO;
            t_layout = Duration::ZERO;
            t_scene = Duration::ZERO;
            t_ensure = Duration::ZERO;
            t_paint = Duration::ZERO;
            t_fence = Duration::ZERO;
            t_fpga = Duration::ZERO;
            t_fpga_max = Duration::ZERO;
            sum_paint_rect_count = 0;
            sum_paint_area_px = 0;
            count_full_paints = 0;
        }
    }

    info!("menu-ui: stopping engine");
    device.stop()?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn accumulate_ui(
    pkt: &FramePacket,
    t_jobs: &mut Duration,
    t_input: &mut Duration,
    t_text_prep: &mut Duration,
    t_images: &mut Duration,
    t_text_pop: &mut Duration,
    t_layout: &mut Duration,
    t_scene: &mut Duration,
) {
    *t_jobs += pkt.ui.jobs;
    *t_input += pkt.ui.input;
    *t_text_prep += pkt.ui.text_prep;
    *t_images += pkt.ui.images;
    *t_text_pop += pkt.ui.text_pop;
    *t_layout += pkt.ui.layout;
    *t_scene += pkt.ui.scene;
}
