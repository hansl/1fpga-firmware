//! Top-level runtime: open the FPGA device, build the Boa context,
//! evaluate the bundle's `main()` to populate the host tree, then
//! drive the frame loop until exit.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use boa_engine::{Module, Source, js_string};
use thiserror::Error;
use tracing::{debug, error, info};

use menu_core_host::device::{Device, DeviceConfig, FramebufferConfig};
use menu_core_host::error::DeviceError;
use menu_core_host::mem;

use crate::font::{FontError, FontRegistry};
use crate::host::UiState;
use crate::image::ImageRegistry;
use crate::input::events::{InputSource, RawInputEvent};
use crate::input::pump::Pump;
use crate::input::router::{IntentKind, IntentRouter};
use crate::input::state::InputState;
use crate::text::TextCache;
use crate::vdom::{NodeId, NodeKind, Tree};

use boa_engine::{JsObject, JsValue};
use boa_engine::object::builtins::JsFunction;

/// Compile-time build identifier — package version. Logged at startup
/// so a glance at the device output confirms which binary is running.
/// (Git hash via build.rs is a future improvement.)
pub const BUILD_ID: &str = env!("CARGO_PKG_VERSION");


/// Translate one raw event to intents and dispatch JS listeners. Raw
/// listeners for the event's source also fire (so input boxes /
/// global hotkeys work). Snapshot listener lists before calling so
/// handlers that subscribe / unsubscribe during dispatch don't
/// invalidate iteration.
/// Build one batch of `{raw, intents?}` entries (one per evdev event)
/// and invoke the JS dispatcher exactly once. Replaces the per-event
/// crossings with a single Rust→Boa call — when the keyboard buffer
/// has 6-18 events queued under spam, this drops the Boa-overhead
/// cost by the same factor.
///
/// The intent router still runs in Rust (cheap) — only the *delivery*
/// to listeners moves to JS.
fn dispatch_input_batch(
    router: &IntentRouter,
    events: &[RawInputEvent],
    dispatcher: &JsFunction,
    context: &mut boa_engine::Context,
) -> Result<(), RuntimeError> {
    use boa_engine::object::builtins::JsArray;
    let arr = JsArray::new(context).map_err(boa_err)?;
    for ev in events {
        let raw = raw_event_to_js(ev, context)?;
        let intents = router.translate(ev);
        let intents_js = JsArray::new(context).map_err(boa_err)?;
        for intent in &intents {
            let i = intent_to_js(intent, context)?;
            intents_js.push(i, context).map_err(boa_err)?;
        }
        let entry = JsObject::with_null_proto();
        entry.set(js_string!("raw"), raw, false, context).map_err(boa_err)?;
        entry
            .set(js_string!("intents"), JsValue::from(intents_js), false, context)
            .map_err(boa_err)?;
        arr.push(JsValue::from(entry), context).map_err(boa_err)?;
    }
    if let Err(e) = dispatcher.call(&JsValue::undefined(), &[JsValue::from(arr)], context) {
        tracing::warn!("input batch dispatcher threw: {e}");
    }
    Ok(())
}

fn dispatch_input(
    input_state: &InputState,
    router: &IntentRouter,
    ev: &RawInputEvent,
    context: &mut boa_engine::Context,
) -> Result<(), RuntimeError> {
    // Raw listeners (one per source).
    let raw_handlers = input_state.snapshot_raw(ev.source());
    if !raw_handlers.is_empty() {
        let arg = raw_event_to_js(ev, context)?;
        for h in raw_handlers {
            if let Err(e) = h.call(&JsValue::undefined(), &[arg.clone()], context) {
                tracing::warn!("raw input handler threw: {e}");
            }
        }
    }
    // Intent dispatches.
    let intents = router.translate(ev);
    for intent in intents {
        let handlers = input_state.snapshot_intent(&intent.name);
        info!(
            "input: {:?} -> intent '{}' kind={:?} ({} listeners)",
            ev.source(),
            intent.name,
            intent.kind,
            handlers.len()
        );
        if handlers.is_empty() {
            continue;
        }
        let arg = intent_to_js(&intent, context)?;
        for h in handlers {
            if let Err(e) = h.call(&JsValue::undefined(), &[arg.clone()], context) {
                tracing::warn!("intent handler '{}' threw: {e}", intent.name);
            }
        }
    }
    Ok(())
}

/// Build a JS object describing an intent dispatch:
/// `{ name: 'confirm', kind: 'pressed' | 'released' | 'repeat' }`.
fn intent_to_js(
    intent: &crate::input::router::IntentDispatch,
    context: &mut boa_engine::Context,
) -> Result<JsValue, RuntimeError> {
    let obj = JsObject::with_null_proto();
    obj.set(
        js_string!("name"),
        js_string!(intent.name.clone()),
        false,
        context,
    )
    .map_err(boa_err)?;
    obj.set(
        js_string!("kind"),
        js_string!(match intent.kind {
            IntentKind::Pressed => "pressed",
            IntentKind::Released => "released",
            IntentKind::Repeat => "repeat",
        }),
        false,
        context,
    )
    .map_err(boa_err)?;
    Ok(JsValue::from(obj))
}

/// Build a JS object describing a raw input event. Shape varies by
/// source — keyboard gets {code, pressed, kind, mods}, gamepad gets
/// {kind: 'button'|'axis', ...}, mouse similar.
fn raw_event_to_js(
    ev: &RawInputEvent,
    context: &mut boa_engine::Context,
) -> Result<JsValue, RuntimeError> {
    use crate::input::events::{GamepadKind, MouseKind};
    let obj = JsObject::with_null_proto();
    let source = match ev.source() {
        InputSource::Keyboard => "keyboard",
        InputSource::Gamepad => "gamepad",
        InputSource::Mouse => "mouse",
    };
    obj.set(js_string!("source"), js_string!(source), false, context)
        .map_err(boa_err)?;
    match ev {
        RawInputEvent::Keyboard(k) => {
            obj.set(js_string!("code"), JsValue::from(k.code), false, context)
                .map_err(boa_err)?;
            obj.set(
                js_string!("pressed"),
                JsValue::from(k.pressed),
                false,
                context,
            )
            .map_err(boa_err)?;
            obj.set(
                js_string!("repeat"),
                JsValue::from(matches!(k.kind, crate::input::events::KeyKind::Repeat)),
                false,
                context,
            )
            .map_err(boa_err)?;
        }
        RawInputEvent::Gamepad(g) => match g.kind {
            GamepadKind::Button { code, pressed } => {
                obj.set(js_string!("kind"), js_string!("button"), false, context)
                    .map_err(boa_err)?;
                obj.set(js_string!("code"), JsValue::from(code), false, context)
                    .map_err(boa_err)?;
                obj.set(js_string!("pressed"), JsValue::from(pressed), false, context)
                    .map_err(boa_err)?;
            }
            GamepadKind::Axis { axis, value } => {
                obj.set(js_string!("kind"), js_string!("axis"), false, context)
                    .map_err(boa_err)?;
                obj.set(js_string!("axis"), JsValue::from(axis), false, context)
                    .map_err(boa_err)?;
                obj.set(js_string!("value"), JsValue::from(value), false, context)
                    .map_err(boa_err)?;
            }
        },
        RawInputEvent::Mouse(m) => match m.kind {
            MouseKind::Move { dx, dy } => {
                obj.set(js_string!("kind"), js_string!("move"), false, context)
                    .map_err(boa_err)?;
                obj.set(js_string!("dx"), JsValue::from(dx), false, context)
                    .map_err(boa_err)?;
                obj.set(js_string!("dy"), JsValue::from(dy), false, context)
                    .map_err(boa_err)?;
            }
            MouseKind::Button { code, pressed } => {
                obj.set(js_string!("kind"), js_string!("button"), false, context)
                    .map_err(boa_err)?;
                obj.set(js_string!("code"), JsValue::from(code), false, context)
                    .map_err(boa_err)?;
                obj.set(js_string!("pressed"), JsValue::from(pressed), false, context)
                    .map_err(boa_err)?;
            }
            MouseKind::Wheel { delta } => {
                obj.set(js_string!("kind"), js_string!("wheel"), false, context)
                    .map_err(boa_err)?;
                obj.set(js_string!("delta"), JsValue::from(delta), false, context)
                    .map_err(boa_err)?;
            }
        },
    }
    Ok(JsValue::from(obj))
}

fn boa_err(e: boa_engine::JsError) -> RuntimeError {
    RuntimeError::Js(format!("{e}"))
}

/// Walk the tree under `root` and load every `<img src>` we haven't
/// seen yet. Failed loads are recorded as `Failed` entries so we
/// don't retry on every frame.
fn prepare_images(
    tree: &Tree,
    root: NodeId,
    images: &mut ImageRegistry,
    device: &mut Device,
) {
    fn walk(tree: &Tree, id: NodeId, images: &mut ImageRegistry, device: &mut Device) {
        let Some(node) = tree.get(id) else {
            return;
        };
        if let NodeKind::Img { src } = &node.kind
            && !src.is_empty()
        {
            // Intrinsic texture: feeds the layout measure function and
            // the paint fallback. Idempotent.
            let _ = images.get_or_load(device, src);
            // If the node has an explicit pixel size, pre-build a
            // texture resized to it so the blit is 1:1 (crisp Lanczos)
            // instead of nearest-neighbour FPGA scaling. Auto/flex-sized
            // images keep using the intrinsic texture.
            if let (Some(w), Some(h)) = (node.style.width, node.style.height) {
                let tw = (w.round() as i32).clamp(1, u16::MAX as i32) as u16;
                let th = (h.round() as i32).clamp(1, u16::MAX as i32) as u16;
                images.ensure_sized(device, src, tw, th);
            }
        }
        for &child in &node.children {
            walk(tree, child, images, device);
        }
    }
    walk(tree, root, images, device);
}

/// Verbose tree dump used for diagnostics. Only emits at DEBUG level
/// so production logs stay quiet.
fn dump_tree(tree: &Tree, id: NodeId, depth: usize) {
    let Some(node) = tree.get(id) else {
        debug!("{:indent$}[{}] <missing>", "", id.0, indent = depth * 2);
        return;
    };
    debug!(
        "{:indent$}[{}] {:?} style={:?}",
        "",
        id.0,
        node.kind,
        node.style,
        indent = depth * 2,
    );
    for &child in &node.children {
        dump_tree(tree, child, depth + 1);
    }
}

pub mod anim;
mod boa;
pub mod damage;
pub mod dump;
pub mod fps;
pub mod raf;
pub mod viewport;
pub mod warmup;

/// Configuration for [`run`].
#[derive(Debug, Default, Clone)]
pub struct RunConfig {
    /// Override the embedded JS bundle with one read from disk. When
    /// `None`, the embedded bundle is used.
    pub bundle_override: Option<PathBuf>,
    /// Override the reserved DDR3 base address (must be 32 MB
    /// aligned). When `None`, [`mem::DEFAULT_BASE`] is used.
    pub base_phys_addr: Option<u32>,
    /// Override the render resolution (FB dimensions). When `None`,
    /// uses the HDMI mode's native resolution from VIDEO_INFO. The
    /// framework's ASCAL block upscales smaller FBs to the active
    /// HDMI mode — so smaller render resolutions trade visual
    /// crispness for proportional reduction in per-frame DDR3 work.
    pub render_res: Option<(u16, u16)>,
    /// Wallpaper PNG to upload to the scanout compositor's wallpaper layer
    /// (Phase C). When it loads, compositing is enabled and the content
    /// framebuffer is cleared transparent (the wallpaper is no longer
    /// painted into content every frame). When `None` or the file is
    /// missing, content clears opaque as before.
    pub wallpaper: Option<PathBuf>,
}

#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("device error: {0}")]
    Device(#[from] DeviceError),

    #[error("font error: {0}")]
    Font(#[from] FontError),

    #[error("io error reading bundle: {0}")]
    Io(#[from] std::io::Error),

    #[error("boa error: {0}")]
    Js(String),

    #[error("the JS bundle did not export a `main` function")]
    NoMainExport,

    #[error("the JS app did not call `1fpga:gui.run(rootId)`")]
    NoRoot,
}

impl From<boa_engine::JsError> for RuntimeError {
    fn from(e: boa_engine::JsError) -> Self {
        RuntimeError::Js(format!("{e}"))
    }
}

/// Embedded fallback bundle. Used when no `--bundle` override is
/// passed. For N1 we keep this empty — the binary refuses to run
/// without `--bundle` until the JS side is wired up. Subsequent
/// milestones will replace this with `include_bytes!(...)` once the
/// Rollup output is stable.
const EMBEDDED_BUNDLE: &[u8] = b"";

pub fn run(cfg: RunConfig) -> Result<(), RuntimeError> {
    // 1. Open the FPGA device + configure framebuffer + start engine.
    let base = cfg.base_phys_addr.unwrap_or(mem::DEFAULT_BASE);
    let mut device = Device::open_with(DeviceConfig {
        base_phys_addr: base,
        ..DeviceConfig::default()
    })?;
    let info = device.video_info();
    // FB resolution: default to native HDMI. A full 1920×1080×4B FB once
    // jittered — its ~498 MB/s of vbuf scanout bandwidth starved ASCAL's
    // reads under HPS DDR3 contention, corrupting the right edge of each
    // scanline. That's now fixed in the core by deepening ASCAL's
    // read-ahead (N_BURST 256→2048, ~1024 px buffered), so native runs
    // clean — and native renders text/icons at full resolution with no
    // upscale. `--render-res` still forces a smaller FB if ever needed.
    let (render_w, render_h) = cfg.render_res.unwrap_or((info.width, info.height));
    let fb = FramebufferConfig {
        width: render_w,
        height: render_h,
        stride: (render_w as u32) * 4,
        fb0_phys: base + mem::FB0_OFFSET as u32,
        fb1_phys: base + mem::FB1_OFFSET as u32,
        fb2_phys: base + mem::FB2_OFFSET as u32,
    };
    device.configure_framebuffer(fb)?;
    device.start()?;
    info!(
        "menu-ui {}: device open, HDMI {}×{}, FB {}×{}",
        BUILD_ID, info.width, info.height, fb.width, fb.height,
    );

    // Phase C: upload the wallpaper to the scanout compositor's layer and
    // enable the hardware blend, so the content FB never carries it (the
    // per-frame full-FB wallpaper copy was the menu-fps bottleneck). On
    // any failure compositing stays off and content clears opaque, as
    // before — see `paint::paint`.
    let compositing = match cfg.wallpaper.as_ref() {
        Some(p) if crate::image::upload_wallpaper_layer(
            &mut device,
            &p.to_string_lossy(),
            fb.width,
            fb.height,
            fb.stride,
        ) => {
            device.set_composite(true);
            info!("compositing ON: wallpaper is a hardware layer; content clears transparent");
            true
        }
        _ => false,
    };

    // 2. Load the JS bundle.
    let bundle: Vec<u8> = match cfg.bundle_override.as_ref() {
        Some(p) => {
            info!("loading bundle from {}", p.display());
            std::fs::read(p)?
        }
        None => {
            if EMBEDDED_BUNDLE.is_empty() {
                error!(
                    "no embedded bundle and no --bundle path provided; \
                     pass --bundle <path> to a built dist/menu_ui.js"
                );
                return Err(RuntimeError::NoMainExport);
            }
            EMBEDDED_BUNDLE.to_vec()
        }
    };

    // 3. Build the Boa context and evaluate the bundle.
    let (mut context, executor, _loader) = boa::build_context()?;
    let ui_state = UiState::default();
    let input_state = InputState::new();
    let fps_counter = fps::FpsCounter::new();
    let raf_state = raf::RafState::new();
    let warmup_queue = warmup::WarmupQueue::new();
    let anim_mgr = anim::AnimationManager::new();
    let viewport = viewport::Viewport::new(fb.width, fb.height);
    context.insert_data(ui_state.clone());
    context.insert_data(input_state.clone());
    context.insert_data(fps_counter.clone());
    context.insert_data(raf_state.clone());
    context.insert_data(warmup_queue.clone());
    context.insert_data(anim_mgr.clone());
    context.insert_data(viewport.clone());

    let module = {
        let source = Source::from_bytes(&bundle);
        Module::parse(source, None, &mut context)?
    };
    debug!("bundle parsed, evaluating");
    if let Err(e) = module.load_link_evaluate(&mut context).await_blocking(&mut context) {
        return Err(e.into());
    }

    let namespace = module.namespace(&mut context);
    let main_fn = namespace.get(js_string!("main"), &mut context)?;
    debug!(
        "main export resolved: callable={}, type={}",
        main_fn.as_callable().is_some(),
        main_fn.type_of()
    );
    let main_fn = main_fn.as_callable().ok_or(RuntimeError::NoMainExport)?;
    let mut result = main_fn.call(
        &boa_engine::JsValue::undefined(),
        &[],
        &mut context,
    )?;
    while let Some(p) = result.as_promise() {
        match p.await_blocking(&mut context) {
            Ok(v) => result = v,
            Err(e) => return Err(e.into()),
        }
    }
    debug!("main() resolved; tree root = {}", ui_state.root().0);
    ui_state.with_tree(|t| dump_tree(t, ui_state.root(), 0));

    // 4. Frame loop.
    let root = ui_state.root();
    if root == NodeId::NONE {
        return Err(RuntimeError::NoRoot);
    }

    let running = Arc::new(AtomicBool::new(true));
    {
        let r = running.clone();
        ctrlc::set_handler(move || r.store(false, Ordering::SeqCst))
            .map_err(|e| RuntimeError::Io(std::io::Error::other(e.to_string())))?;
    }

    // Fence-wait ceiling. Normal frames retire in ~15-25 ms; this only
    // trips on a genuine stall. Kept at 2 s (not the old 500 ms) because
    // this is a deliberately bandwidth-constrained config where an
    // occasional slow frame under heavy DDR contention is legitimate.
    let timeout = Duration::from_secs(2);
    let mut frame_idx: u32 = 0;
    let mut fonts = FontRegistry::new();
    let mut text_cache = TextCache::new();

    // Drain any `gui.warmupGlyphs` requests the bundle's main()
    // queued. Done here, before the frame loop, so the first
    // user-visible frame already has populated atlases — no atlas
    // rebuilds during navigation. Failures are logged but
    // non-fatal; the missing chars will rebuild lazily later.
    {
        let requests = warmup_queue.drain();
        if !requests.is_empty() {
            info!("warming up {} font atlas request(s)", requests.len());
        }
        for req in requests {
            match fonts.ensure(&mut device, &req.family, req.px_size, &req.chars) {
                Ok(_) => {}
                Err(e) => tracing::warn!(
                    "atlas warmup failed for {}@{}: {}",
                    req.family,
                    req.px_size,
                    e
                ),
            }
        }
    }
    let mut images = ImageRegistry::new();
    // Aspect-fit any oversized PNGs to the FB on load — e.g., the
    // 1920×1080 wallpaper at a 720p render target gets resized to
    // 1280×720 once, then every paint hits the 1:1 burst path.
    images.set_max_dims(fb.width, fb.height);
    let mut pump = Pump::open_all();
    let router = IntentRouter::new();
    let mut event_buf: Vec<RawInputEvent> = Vec::new();
    // Reference epoch for `requestAnimationFrame` timestamps. The
    // first callback sees a small positive number (ms since just
    // before the loop started) — matches the browser DOMHighResTimeStamp
    // shape close enough for our use cases.
    let raf_epoch = std::time::Instant::now();

    // Per-FB content hash + scene snapshot. The three FB slots have
    // *distinct* physical addresses (`mem::FB0_OFFSET`/`FB1_OFFSET`/
    // `FB2_OFFSET` — 8 MB apart), so each render_idx slot's memory
    // is independent. The damage diff that drives each frame's paint
    // is against THIS slot's prior snapshot — otherwise we'd skip
    // painting regions that are stale in *this* slot just because
    // they're up-to-date in a different slot we painted recently.
    // (Trying to collapse this to a single snapshot was a brief
    // mistake born from a stale comment claiming the FB pointers
    // were aliased; with non-aliased pointers it produced visible
    // ghosts during slide animations.)
    let mut scene_hash_per_fb: [Option<u64>; 3] = [None, None, None];
    let mut scene_per_fb: [Option<damage::PaintedScene>; 3] =
        [None, None, None];
    // In-flight fence values from submitted-but-not-yet-retired
    // frames. The triple-buffer fb_swapper has 3 FB slots: at most
    // one displayed + one ready + one rendering. To match that,
    // bound the host's lead to 2 in-flight frames — if we have 2
    // pending, wait for the oldest fence to retire before submitting
    // the next frame. This is "1-2 frames ahead" pipelining: the
    // FPGA is processing frame N while the host prepares N+1 and
    // N+2 is queued in the ring.
    // Each entry is (fence_value, submit_instant) so that on retire we
    // can measure the frame's true FPGA render time (submit -> retire).
    let mut pending_fences: std::collections::VecDeque<(u32, std::time::Instant)> =
        std::collections::VecDeque::with_capacity(3);
    // Bounded to 1 so the host always waits for the previous frame's
    // PRESENT to retire before reading FB_STATE.render. Without this,
    // every other iteration would read a stale render_idx (FPGA still
    // processing the previous PRESENT), causing damage-tracking
    // misalignment and ghosting. The dual blit engines still help
    // because: (a) PRESENT alternates them so consecutive frames don't
    // hit the same engine back-to-back; (b) the wait happens AFTER
    // scene compute, so the FPGA's blit work overlaps with the host's
    // CPU work for the next frame.
    const MAX_INFLIGHT_FRAMES: usize = 1;

    // Damage-paint threshold: above this fraction of the FB we fall
    // back to a single clip-less paint instead of per-rect clipped
    // paints. The guard exists because each damage rect costs a full
    // tree-walk; but `coalesce_nearby` keeps the post-coalesce rect
    // count tiny (~2-3), so that per-rect overhead is small and a
    // partial paint stays cheaper than a full one well past 70% — a
    // full paint redraws the *entire* opaque wallpaper (~110 ms at
    // 1080p, the worst single-frame cost in the profiles). A category
    // switch legitimately dirties ~45-70% (full column content change +
    // sliding strip), and at 70% this threshold was escalating those to
    // a whole-screen wallpaper repaint that's more expensive than the
    // partial it replaced. Set high so only near-total damage goes full.
    // (Deeper win, separate change: the per-FB-slot diff is 3 frames
    // stale under triple-buffering, inflating nav damage; and
    // `total_area` sums rects without de-overlapping — both bias this
    // measurement upward.)
    let fb_area: u64 = (fb.width as u64) * (fb.height as u64);
    let full_paint_threshold: u64 = fb_area * 95 / 100;
    // Per-stage timing accumulators. Every TIMING_LOG_PERIOD frames
    // we log the average per-stage cost. Tells us where the frame
    // budget actually goes (so we can tell tick_jobs from layout
    // from present, etc.).
    use std::time::Instant;
    const TIMING_LOG_PERIOD: u32 = 60;
    let mut t_jobs = Duration::ZERO;
    let mut t_input = Duration::ZERO;
    let mut t_text_prep = Duration::ZERO;
    let mut t_images = Duration::ZERO;
    let mut t_text_pop = Duration::ZERO;
    let mut t_layout = Duration::ZERO;
    let mut t_paint = Duration::ZERO;
    let mut t_fence = Duration::ZERO;
    let mut t_scanout = Duration::ZERO;
    // Scene-build (opacity/transform/compute_scene) split out of the
    // paint bucket — it's the "tree logic" that would move to the JS
    // core in a 2-thread split, so we want it priced separately.
    let mut t_scene = Duration::ZERO;
    // True FPGA render time per frame: submit -> that frame's fence
    // retire, independent of host/FPGA overlap. Compared against `cpu`
    // (below) this is the CPU-bound vs FPGA-bound verdict — i.e. whether
    // moving the JS reconcile to a second core can lift fps at all, and
    // where the core-0/core-1 boundary should sit. `t_fpga_max` catches
    // the full-repaint spikes that the average hides.
    let mut t_fpga = Duration::ZERO;
    let mut t_fpga_max = Duration::ZERO;
    // Damage-stage accounting: rect count + total area per frame,
    // and how many frames in the window fell back to full paint.
    let mut sum_paint_rect_count: u64 = 0;
    let mut sum_paint_area_px: u64 = 0;
    let mut count_full_paints: u32 = 0;

    while running.load(Ordering::SeqCst) {
        let t0 = Instant::now();

        // 0a. Drive the JS job queue forward by one tick. We can't
        //     use `context.run_jobs()` here — it blocks until every
        //     queued job (including future-scheduled timeouts) is
        //     drained, so a recurring `setInterval` would deadlock
        //     the loop. `boa::tick_jobs` polls the executor's
        //     `run_jobs_async` future a bounded number of times
        //     instead; see its doc comment for the rationale.
        if let Err(e) = boa::tick_jobs(&executor, &mut context) {
            tracing::warn!("tick_jobs error: {e}");
        }

        let t1 = Instant::now();

        // 0b. Pump input. Drain pending evdev events, translate to
        //     intents, dispatch any subscribed JS listeners. When a
        //     batch dispatcher is registered (via
        //     `gui.setInputDispatcher`), we cross the Rust→Boa
        //     boundary once per drain regardless of event count —
        //     JS owns the per-listener routing. The per-event path
        //     stays as the fallback for any bundle that hasn't
        //     registered a dispatcher.
        event_buf.clear();
        pump.drain(&mut event_buf);
        if !event_buf.is_empty() {
            match input_state.batch_dispatcher() {
                Some(dispatcher) => {
                    dispatch_input_batch(
                        &router,
                        &event_buf,
                        &dispatcher,
                        &mut context,
                    )?;
                    event_buf.clear();
                }
                None => {
                    for ev in event_buf.drain(..) {
                        dispatch_input(&input_state, &router, &ev, &mut context)?;
                    }
                }
            }
        }

        // 0c. Tick jobs again — handlers above may have called
        //     setTimeout (directly or via React's setState scheduler).
        if let Err(e) = boa::tick_jobs(&executor, &mut context) {
            tracing::warn!("tick_jobs error: {e}");
        }

        // 0d. Drain the requestAnimationFrame queue. Each callback
        //     receives ms-since-runtime-start, matching the browser's
        //     DOMHighResTimeStamp contract. Re-registrations from
        //     inside callbacks land in the next frame's queue (drain
        //     snapshots before iterating).
        let raf_callbacks = raf_state.drain();
        if !raf_callbacks.is_empty() {
            let now_ms = raf_epoch.elapsed().as_secs_f64() * 1000.0;
            let arg = JsValue::from(now_ms);
            for cb in raf_callbacks {
                if let Err(e) = cb.call(&JsValue::undefined(), &[arg.clone()], &mut context) {
                    tracing::warn!("requestAnimationFrame callback threw: {e}");
                }
            }
            // RAF callbacks routinely call setState / updateStyle and
            // may have queued more work; pump it before paint.
            if let Err(e) = boa::tick_jobs(&executor, &mut context) {
                tracing::warn!("tick_jobs error: {e}");
            }
        }

        let t2 = Instant::now();

        // 0e. Advance any in-flight tweens. Runs before layout so
        //     tweens of layout-affecting properties (width, height,
        //     etc.) take effect this frame. Each tween mutates the
        //     target node's style via `Style::merge_from`, which is
        //     the same path React's `gui.updateStyle` uses; the
        //     damage system then naturally diffs the changed value
        //     into a per-rect repaint.
        let _active_tweens = anim_mgr.tick(&ui_state);

        // 1. Resolve text style inheritance once for the frame.
        let text_styles = ui_state.with_tree(|tree| crate::text::resolve(tree, root));

        // 2. Prepare: ensure every (font, size) used by text nodes
        //    has a built+uploaded atlas. Mutates Device, must run
        //    before begin_frame.
        ui_state.with_tree(|tree| crate::text::prepare(tree, &text_styles, &mut fonts, &mut device))?;

        let t3 = Instant::now();

        // 3. Prepare images: walk the tree, decode + upload any
        //    `<img>` whose `src` we haven't seen yet. Failures are
        //    cached so we don't retry every frame.
        ui_state.with_tree(|tree| prepare_images(tree, root, &mut images, &mut device));

        let t4 = Instant::now();

        // 4. Populate text cache: allocate render-target textures for
        //    any (content, font, size, color) tuples we haven't seen
        //    yet. Returns the list of pendings to render this frame.
        let pendings = ui_state.with_tree(|tree| {
            text_cache.populate(tree, &text_styles, &fonts, &mut device)
        })?;

        let t5 = Instant::now();

        // 5. Compute layout. Taffy's measure function consults the
        //    font atlas / image registry for text and img leaves.
        let layouts = ui_state.with_tree(|tree| {
            crate::layout::compute(
                tree,
                root,
                fb.width as f32,
                fb.height as f32,
                &text_styles,
                &fonts,
                &images,
            )
        });

        let t6 = Instant::now();

        // 6. Build the current PaintedScene snapshot, and skip
        //    everything when this FB slot already has it. Computing
        //    the scene is cheap (just a tree walk + small Vec) and
        //    gives us both the fast skip-hash and the structure
        //    damage needs to diff against the prior state.
        //
        // Compute scene FIRST — this work doesn't depend on render_idx
        // and overlaps with the FPGA's previous-frame blit work. The
        // wait_fence below blocks only if we're outrunning the FPGA.
        let opacities = ui_state.with_tree(|tree| crate::style::resolve_opacity(tree, root));
        let transforms = ui_state.with_tree(|tree| crate::style::resolve_transforms(tree, root));
        let current_scene = ui_state.with_tree(|tree| {
            damage::compute_scene(tree, root, &layouts, &text_styles, &opacities, &transforms)
        });
        let current_hash = current_scene.hash();

        // NOW wait for the previous frame's fence (= sync barrier for
        // reading FB_STATE.render). Wait happens AFTER scene compute
        // so the FPGA's blit work overlaps with the host's CPU work.
        // The MAX_INFLIGHT_FRAMES=2 bound is here so the host can be
        // up to 1 frame ahead of the FPGA's PRESENT processing.
        let fence_wait_start = Instant::now();
        // submit -> retire of the frame we block on = the FPGA's true
        // render time for that frame (it started rendering at submit and
        // is done when its fence retires), regardless of how much the
        // host overlapped it. This is the number that decides CPU- vs
        // FPGA-bound. 0 only on the first frame (nothing in flight yet).
        let mut frame_fpga_dt = Duration::ZERO;
        if pending_fences.len() >= MAX_INFLIGHT_FRAMES {
            let (oldest_fence, oldest_submit) = pending_fences.pop_front().unwrap();
            device.wait_fence(oldest_fence, timeout)?;
            frame_fpga_dt = oldest_submit.elapsed();
        }
        let fence_dt = fence_wait_start.elapsed();

        // render_idx is fresh now: the wait above ensures the FPGA
        // has processed our most-recently-committed PRESENT and
        // fb_swapper has rotated. Damage diff is against the scene
        // we last painted INTO THIS SAME PHYSICAL SLOT, which is the
        // current content.
        let render_idx = (device.fb_state().render as usize).min(2);
        if scene_hash_per_fb[render_idx] == Some(current_hash) {
            // This FB slot already has the desired content. Sleep
            // ~one vsync to bound the loop and continue.
            fps_counter.record_frame();
            std::thread::sleep(Duration::from_millis(16));
            continue;
        }

        // 7. Paint directly into FB[render_idx] with damage-region
        //    tracking. Each FB slot has its own physical memory (see
        //    `scene_per_fb`'s declaration), so the diff that drives
        //    this paint is against THIS slot's prior snapshot.
        //
        //    The slot we're targeting isn't the one HDMI is currently
        //    scanning (fb_swapper rotates display ↔ render on each
        //    PRESENT), so we can paint without tearing the displayed
        //    image. A previous version of the runtime painted into a
        //    single staging RT and copied it to each FB on every
        //    frame; the copy alone cost ~14 ms of DDR3 bandwidth and
        //    added nothing once the FBs were correctly per-slot
        //    tracked, so it was removed.
        let frame = device.begin_frame();
        let frame = crate::paint::render_pending_text(frame, &pendings, &fonts)?;
        let frame = frame.set_target_framebuffer()?;

        // Track what we actually painted this frame for the timing
        // log's damage stats. Initialized in each arm of the match
        // below.
        let paint_rect_count: u32;
        let paint_area_px: u64;
        let mut paint_full = false;

        let damage_paint_plan: Option<Vec<damage::PixelRect>> =
            match &scene_per_fb[render_idx] {
                Some(prev) => {
                    let d = damage::compute_damage(prev, &current_scene);
                    let area = damage::total_area(&d);
                    if d.is_empty() || area > full_paint_threshold {
                        None
                    } else {
                        Some(d)
                    }
                }
                None => None,
            };
        let frame = match damage_paint_plan {
            Some(rects) => {
                paint_rect_count = rects.len() as u32;
                paint_area_px = damage::total_area(&rects);
                // Per-rect clipped paint. clear_clip after each rect
                // so the next rect's set_clip replaces it cleanly.
                // The host-side bbox cull inside paint() also uses
                // the rect so nodes outside it never get a blit op
                // issued.
                let mut frame = frame;
                for r in &rects {
                    let clip_rect: menu_core_host::protocol::Rect = (*r).into();
                    let f = frame.set_clip(clip_rect)?;
                    let f = ui_state.with_tree(|tree| {
                        crate::paint::paint(
                            tree,
                            root,
                            &fb,
                            &layouts,
                            &text_styles,
                            &text_cache,
                            &images,
                            &opacities,
                            &transforms,
                            Some(clip_rect),
                            compositing,
                            f,
                        )
                    })?;
                    frame = f.clear_clip()?;
                }
                frame
            }
            None => {
                paint_full = true;
                paint_rect_count = 1;
                paint_area_px = fb_area;
                ui_state.with_tree(|tree| {
                    crate::paint::paint(
                        tree,
                        root,
                        &fb,
                        &layouts,
                        &text_styles,
                        &text_cache,
                        &images,
                        &opacities,
                        &transforms,
                        None,
                        compositing,
                        frame,
                    )
                })?
            }
        };

        let t7 = Instant::now();

        let new_token = frame.present()?.submit()?;
        pending_fences.push_back((new_token.fence_value(), Instant::now()));
        let scanout_dt = Duration::ZERO;

        scene_hash_per_fb[render_idx] = Some(current_hash);
        scene_per_fb[render_idx] = Some(current_scene);
        ui_state.with_tree_mut(|t| t.clear_dirty());
        fps_counter.record_frame();

        sum_paint_rect_count += paint_rect_count as u64;
        sum_paint_area_px += paint_area_px;
        if paint_full {
            count_full_paints += 1;
        }

        t_jobs += t1 - t0;
        t_input += t2 - t1;
        t_text_prep += t3 - t2;
        t_images += t4 - t3;
        t_text_pop += t5 - t4;
        t_layout += t6 - t5;
        // Scene build = opacity/transform/compute_scene, i.e. t6 -> the
        // start of the fence wait. The rest of t7 - t6 (after removing
        // the scene build and the fence wait) is the damage diff + the
        // actual paint loop.
        let scene_dt = fence_wait_start.saturating_duration_since(t6);
        t_scene += scene_dt;
        t_paint += (t7 - t6).saturating_sub(fence_dt).saturating_sub(scene_dt);
        t_fence += fence_dt;
        t_fpga += frame_fpga_dt;
        if frame_fpga_dt > t_fpga_max {
            t_fpga_max = frame_fpga_dt;
        }
        t_scanout += scanout_dt;

        frame_idx = frame_idx.wrapping_add(1);

        if frame_idx % TIMING_LOG_PERIOD == 0 {
            let n = TIMING_LOG_PERIOD as u32;
            let avg = |t: Duration| t.as_micros() as u32 / n;
            let total = t_jobs + t_input + t_text_prep + t_images + t_text_pop
                + t_layout + t_scene + t_paint + t_fence + t_scanout;
            // cpu = host-thread busy time (everything except the fence
            // wait). The cpu-vs-fpga comparison is the verdict: cpu >
            // fpga ⇒ CPU-bound, a JS render-thread split can lift fps;
            // fpga > cpu ⇒ render is the ceiling, split won't help fps
            // (cut render cost instead). `fpga_max` flags full-repaint
            // spikes the average hides.
            let cpu = total.saturating_sub(t_fence);
            tracing::info!(
                "frame timings (us avg over {n}): jobs={} input={} text_prep={} images={} text_pop={} layout={} scene={} paint={} fence={} fpga={} fpga_max={} cpu={} total={}",
                avg(t_jobs),
                avg(t_input),
                avg(t_text_prep),
                avg(t_images),
                avg(t_text_pop),
                avg(t_layout),
                avg(t_scene),
                avg(t_paint),
                avg(t_fence),
                avg(t_fpga),
                t_fpga_max.as_micros() as u32,
                avg(cpu),
                avg(total),
            );
            // Damage / pixel accounting. "rects" is average rect
            // count per frame (after coalescing); "area" is average
            // damaged pixels per frame as a percentage of the FB;
            // "full" is the number of frames in this window that
            // fell back to a full repaint (above the 70 % threshold
            // or first-frame for the slot).
            let pct = |area: u64| {
                if fb_area == 0 { 0u32 } else {
                    ((area * 100) / (fb_area * n as u64)) as u32
                }
            };
            tracing::info!(
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
            t_paint = Duration::ZERO;
            t_fence = Duration::ZERO;
            t_scanout = Duration::ZERO;
            t_scene = Duration::ZERO;
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
