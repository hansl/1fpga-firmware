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

/// Compile-time build identifier — package version. Logged at startup
/// so a glance at the device output confirms which binary is running.
/// (Git hash via build.rs is a future improvement.)
pub const BUILD_ID: &str = env!("CARGO_PKG_VERSION");


/// Translate one raw event to intents and dispatch JS listeners. Raw
/// listeners for the event's source also fire (so input boxes /
/// global hotkeys work). Snapshot listener lists before calling so
/// handlers that subscribe / unsubscribe during dispatch don't
/// invalidate iteration.
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
            && images.get(src).is_none()
            && !src.is_empty()
        {
            let _ = images.get_or_load(device, src);
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
    let fb = FramebufferConfig::for_video(info, base);
    device.configure_framebuffer(fb)?;
    device.start()?;
    info!(
        "menu-ui {}: device open, framebuffer {}×{}",
        BUILD_ID, info.width, info.height
    );

    // 1b. Allocate a staging RT for proper triple-buffering. The
    //     React tree is painted ONCE into this RT, then each frame
    //     copy_rect's the RT onto the active FB. This guarantees all
    //     three rotating FBs receive bit-identical pixels — without
    //     the staging step, painting directly into FB[render_idx]
    //     produces subtle timing-induced differences across the
    //     three buffers that fb_swapper's rotation surfaces as text
    //     flicker. Single allocation; sized to the framebuffer.
    let staging_rt = device.create_render_target(fb.width, fb.height)?;
    info!(
        "staging rt allocated: tex_id={}, phys={:#010X}",
        staging_rt.id, staging_rt.phys_addr
    );

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
    context.insert_data(ui_state.clone());
    context.insert_data(input_state.clone());
    context.insert_data(fps_counter.clone());
    context.insert_data(raf_state.clone());
    context.insert_data(warmup_queue.clone());
    context.insert_data(anim_mgr.clone());

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

    let timeout = Duration::from_millis(500);
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
    let mut pump = Pump::open_all();
    let router = IntentRouter::new();
    let mut event_buf: Vec<RawInputEvent> = Vec::new();
    // Reference epoch for `requestAnimationFrame` timestamps. The
    // first callback sees a small positive number (ms since just
    // before the loop started) — matches the browser DOMHighResTimeStamp
    // shape close enough for our use cases.
    let raf_epoch = std::time::Instant::now();

    // Single FB tracker — *not* per-render_idx. The FB slot pointers
    // are aliased to the same physical address (see
    // `damage.rs`'s top-level comment), so what's in memory at any
    // moment is whatever the last *frame* painted regardless of
    // which render_idx that frame used. Tracking per-slot would
    // make the diff a 3-frame-old stale snapshot for an FB we
    // rotated through, which both over-counts damage (icons that
    // slid past in the meantime) and under-counts it (regions that
    // matched 3 frames ago but were overwritten in between). One
    // global snapshot is both correct and minimal.
    let mut last_fb_hash: Option<u64> = None;
    let mut last_fb_scene: Option<damage::PaintedScene> = None;
    // Hash + scene of what's currently rendered into the staging RT.
    // staging_scene drives the damage diff for the *paint* step;
    // staging_hash is the fast skip check.
    let mut scene_hash_in_staging: Option<u64> = None;
    let mut staging_scene: Option<damage::PaintedScene> = None;

    // Damage-paint threshold: if the union of damage rects covers
    // more than this fraction of the FB, fall back to a single
    // clip-less paint/copy. Per-rect iteration has per-walk host
    // overhead and per-op FPGA dispatch overhead; for big damage the
    // wins from skipping clean pixels evaporate.
    let fb_area: u64 = (fb.width as u64) * (fb.height as u64);
    let full_paint_threshold: u64 = fb_area * 70 / 100;
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
    // Damage-stage accounting: lets us see whether a high fence is
    // caused by big damage areas or per-rect dispatch overhead.
    // `staging_*` reflect what was painted into the staging RT this
    // frame; `fb_*` reflect what was copied to FB.
    let mut sum_staging_rect_count: u64 = 0;
    let mut sum_staging_area_px: u64 = 0;
    let mut sum_fb_rect_count: u64 = 0;
    let mut sum_fb_area_px: u64 = 0;
    let mut count_staging_full_paints: u32 = 0;
    let mut count_fb_full_copies: u32 = 0;

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
        //     intents, dispatch any subscribed JS listeners.
        event_buf.clear();
        pump.drain(&mut event_buf);
        for ev in event_buf.drain(..) {
            dispatch_input(&input_state, &router, &ev, &mut context)?;
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
        //    everything when the FB already has it (memory hasn't
        //    changed since the last frame, and the desired scene
        //    matches what's there). Computing the scene is cheap
        //    (just a tree walk + small Vec) and gives us both the
        //    fast skip-hash and the structure damage needs to diff
        //    against the prior state.
        let opacities = ui_state.with_tree(|tree| crate::style::resolve_opacity(tree, root));
        let transforms = ui_state.with_tree(|tree| crate::style::resolve_transforms(tree, root));
        let current_scene = ui_state.with_tree(|tree| {
            damage::compute_scene(tree, root, &layouts, &text_styles, &opacities, &transforms)
        });
        let current_hash = current_scene.hash();
        if last_fb_hash == Some(current_hash) {
            // FB memory already matches the desired scene. Sleep
            // ~one vsync to bound the loop and continue.
            fps_counter.record_frame();
            std::thread::sleep(Duration::from_millis(16));
            continue;
        }

        // 7. Paint pipeline with damage-region tracking:
        //
        //    a. If staging doesn't already hold the current scene,
        //       paint into it. Either a full repaint (first time, or
        //       damage exceeds the threshold) or a sequence of
        //       per-rect clipped paints. The blit engine evaluates
        //       dst ∩ clip up front, so an op fully outside the clip
        //       produces zero DDR work.
        //    b. Copy staging→FB. Either one full-screen copy or a
        //       copy per damage rect (relative to FB's last-frame
        //       snapshot — see `last_fb_scene`'s declaration for why
        //       there's no per-render_idx tracking).
        let frame = device.begin_frame();
        let frame = crate::paint::render_pending_text(frame, &pendings, &fonts)?;

        // Track what we actually painted/copied this frame so the
        // timing log can report damage areas alongside the fence.
        let mut staging_rect_count: u32 = 0;
        let mut staging_area_px: u64 = 0;
        let mut staging_full_paint = false;
        let mut fb_rect_count: u32 = 0;
        let mut fb_area_px: u64 = 0;
        let mut fb_full_copy = false;

        let frame = if scene_hash_in_staging != Some(current_hash) {
            let frame = frame.set_target(&staging_rt)?;
            // Decide between damage paint and full paint.
            let damage_paint_plan = match &staging_scene {
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
                    staging_rect_count = rects.len() as u32;
                    staging_area_px = damage::total_area(&rects);
                    // Per-rect clipped paint. clear_clip after each
                    // rect so the next rect's set_clip replaces it
                    // cleanly (not unions/intersects with it).
                    let mut frame = frame;
                    for r in &rects {
                        let f = frame.set_clip((*r).into())?;
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
                                f,
                            )
                        })?;
                        frame = f.clear_clip()?;
                    }
                    frame
                }
                None => {
                    staging_full_paint = true;
                    staging_rect_count = 1;
                    staging_area_px = fb_area;
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
                            frame,
                        )
                    })?
                }
            };
            scene_hash_in_staging = Some(current_hash);
            staging_scene = Some(current_scene.clone());
            frame
        } else {
            frame
        };

        // Copy staging→FB. Damage diff is against the single global
        // last_fb_scene snapshot (NOT a per-render_idx slot — see
        // last_fb_scene's declaration).
        let frame = frame.set_target_framebuffer()?;
        let fb_damage_plan: Option<Vec<damage::PixelRect>> =
            match &last_fb_scene {
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
        let frame = match fb_damage_plan {
            Some(rects) => {
                fb_rect_count = rects.len() as u32;
                fb_area_px = damage::total_area(&rects);
                let mut frame = frame;
                for r in &rects {
                    let rect: menu_core_host::protocol::Rect = (*r).into();
                    frame = frame.copy_rect(
                        &staging_rt,
                        rect,
                        rect,
                        menu_core_host::frame::CopyOpts::default(),
                    )?;
                }
                frame
            }
            None => {
                fb_full_copy = true;
                fb_rect_count = 1;
                fb_area_px = fb_area;
                frame.copy_rect(
                    &staging_rt,
                    menu_core_host::protocol::Rect::new(0, 0, fb.width, fb.height),
                    menu_core_host::protocol::Rect::new(0, 0, fb.width, fb.height),
                    menu_core_host::frame::CopyOpts::default(),
                )?
            }
        };

        let t7 = Instant::now();

        let (_count, fence_dt, scanout_dt) =
            frame.present()?.submit()?.wait_presented_timed(timeout)?;

        last_fb_hash = Some(current_hash);
        last_fb_scene = Some(current_scene);
        ui_state.with_tree_mut(|t| t.clear_dirty());
        fps_counter.record_frame();

        sum_staging_rect_count += staging_rect_count as u64;
        sum_staging_area_px += staging_area_px;
        sum_fb_rect_count += fb_rect_count as u64;
        sum_fb_area_px += fb_area_px;
        if staging_full_paint {
            count_staging_full_paints += 1;
        }
        if fb_full_copy {
            count_fb_full_copies += 1;
        }

        t_jobs += t1 - t0;
        t_input += t2 - t1;
        t_text_prep += t3 - t2;
        t_images += t4 - t3;
        t_text_pop += t5 - t4;
        t_layout += t6 - t5;
        t_paint += t7 - t6;
        t_fence += fence_dt;
        t_scanout += scanout_dt;

        frame_idx = frame_idx.wrapping_add(1);

        if frame_idx % TIMING_LOG_PERIOD == 0 {
            let n = TIMING_LOG_PERIOD as u32;
            let avg = |t: Duration| t.as_micros() as u32 / n;
            tracing::info!(
                "frame timings (us avg over {n}): jobs={} input={} text_prep={} images={} text_pop={} layout={} paint={} fence={} scanout={} total={}",
                avg(t_jobs),
                avg(t_input),
                avg(t_text_prep),
                avg(t_images),
                avg(t_text_pop),
                avg(t_layout),
                avg(t_paint),
                avg(t_fence),
                avg(t_scanout),
                avg(t_jobs + t_input + t_text_prep + t_images + t_text_pop + t_layout + t_paint + t_fence + t_scanout),
            );
            // Damage / pixel accounting. "rects" is average rect count
            // per frame (after coalescing); "area_pct" is average
            // damaged pixels per frame as a percentage of the FB;
            // "full" is the number of frames in this window that
            // fell back to a full repaint / full copy (above the
            // 70 % threshold or first-frame). Lets us correlate fence
            // cost with the actual painted area, separately for the
            // staging-RT paint step and the staging→FB copy step.
            let pct = |area: u64| {
                if fb_area == 0 { 0u32 } else {
                    ((area * 100) / (fb_area * n as u64)) as u32
                }
            };
            tracing::info!(
                "damage avg over {n}: staging rects={:.1} area={}% full={} | fb rects={:.1} area={}% full={}",
                (sum_staging_rect_count as f32) / (n as f32),
                pct(sum_staging_area_px),
                count_staging_full_paints,
                (sum_fb_rect_count as f32) / (n as f32),
                pct(sum_fb_area_px),
                count_fb_full_copies,
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
            sum_staging_rect_count = 0;
            sum_staging_area_px = 0;
            sum_fb_rect_count = 0;
            sum_fb_area_px = 0;
            count_staging_full_paints = 0;
            count_fb_full_copies = 0;
        }
    }

    info!("menu-ui: stopping engine");

    // Diagnostic: snapshot staging RT + each FB to /tmp/menu-ui-*.png
    // before stopping. Lets us see whether the host actually wrote
    // identical bytes to all three FBs (and whether staging matches
    // what HDMI displayed) without trusting fb_swapper or scanout to
    // do the right thing.
    {
        let out_dir = std::path::Path::new("/tmp");
        dump::try_dump(
            "staging_rt",
            staging_rt.phys_addr,
            fb.width,
            fb.height,
            (fb.width as u32) * 4,
            &out_dir.join("menu-ui-staging.png"),
        );
        dump::try_dump(
            "fb0",
            fb.fb0_phys,
            fb.width,
            fb.height,
            fb.stride,
            &out_dir.join("menu-ui-fb0.png"),
        );
        dump::try_dump(
            "fb1",
            fb.fb1_phys,
            fb.width,
            fb.height,
            fb.stride,
            &out_dir.join("menu-ui-fb1.png"),
        );
        dump::try_dump(
            "fb2",
            fb.fb2_phys,
            fb.width,
            fb.height,
            fb.stride,
            &out_dir.join("menu-ui-fb2.png"),
        );
    }

    device.stop()?;
    Ok(())
}
