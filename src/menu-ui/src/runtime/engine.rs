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

        // ---- Latest UI frame (or housekeeping tick) -----------------
        let Some(pkt) = mailbox.recv_timeout(IDLE_TICK) else {
            continue;
        };

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

        // Per-slot skip: this FB already holds exactly this scene and
        // there is no glyph work to flush.
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
        let frame = {
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
        let frame = match damage_paint_plan {
            Some(rects) => {
                paint_rect_count = rects.len() as u32;
                paint_area_px = damage::total_area(&rects);
                let mut frame = frame;
                for r in &rects {
                    let clip_rect: Rect = (*r).into();
                    let f = frame.set_clip(clip_rect)?;
                    let f = crate::display_list::replay(&pkt.dl, Some(clip_rect), f)?;
                    frame = f.clear_clip()?;
                }
                frame
            }
            None => {
                paint_full = true;
                paint_rect_count = 1;
                paint_area_px = cfg.fb_area;
                crate::display_list::replay(&pkt.dl, None, frame)?
            }
        };

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
