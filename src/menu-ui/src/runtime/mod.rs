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
pub mod engine;
pub mod packet;
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
    /// Enable the content coverage mask (task #15) when compositing: the
    /// host computes a per-frame 64x64-tile coverage bitmap so the
    /// compositor skips reading the transparent majority of the content
    /// layer, cutting scanout DDR contention. No effect without a
    /// wallpaper (compositing off). Default true.
    pub content_mask: bool,
    /// Phase D bring-up: upload a test boxart panel and animate its position
    /// (slide in/out at the right edge) to validate the placed+translatable
    /// overlay layer end-to-end. No effect without compositing.
    pub boxart_demo: bool,
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

/// 256x256 BGRA8888 premultiplied test panel for the Phase D boxart
/// bring-up demo: translucent teal fill (~85% alpha) with an opaque white
/// border, so it's obviously an overlay and its edges/clipping are visible.
fn boxart_demo_panel() -> (Vec<u8>, u16, u16) {
    const W: usize = 256;
    const H: usize = 256;
    let mut px = vec![0u8; W * H * 4];
    for y in 0..H {
        for x in 0..W {
            let i = (y * W + x) * 4;
            let border = x < 4 || x >= W - 4 || y < 4 || y >= H - 4;
            let (b, g, r, a): (u8, u8, u8, u8) = if border {
                (255, 255, 255, 255) // opaque white
            } else {
                let a = 217u16; // ~85%
                let prem = |c: u16| (c * a / 255) as u8;
                (prem(128), prem(128), prem(0), a as u8) // premult teal
            };
            px[i] = b;
            px[i + 1] = g;
            px[i + 2] = r;
            px[i + 3] = a;
        }
    }
    (px, W as u16, H as u16)
}

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
    // before — see `display_list::build`'s background policy.
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

    // Content coverage mask (task #15): only meaningful with compositing
    // (it gates the content layer over the wallpaper). Enabled on the
    // first successful upload below.
    let content_mask_on = compositing && cfg.content_mask;
    if content_mask_on {
        info!("content coverage mask: ON (host computes per-frame tile bitmap)");
    }

    // Phase D boxart-layer bring-up demo: upload a test panel once and
    // animate ONLY its position each frame (slide in/out at the right edge)
    // — proves the placed+translatable overlay blends over the menu and
    // clips off-screen, with zero blit work (content FB untouched).
    let boxart_demo = cfg.boxart_demo;
    if boxart_demo {
        let (px, bw, bh) = boxart_demo_panel();
        match device.upload_boxart(&px, bw, bh) {
            Ok(()) => {
                device.set_boxart_pos(1920, 400); // start fully off-screen right
                device.set_boxart(true);
                info!("boxart demo: ON ({}x{} panel sliding at the right edge)", bw, bh);
            }
            Err(e) => error!("boxart demo upload failed: {e}"),
        }
    }
    // (The boxart demo's per-frame position animation runs on the
    // engine thread — see engine.rs.)

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
    let mut fonts = FontRegistry::new();
    let text_cache = TextCache::new();

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
    let router = IntentRouter::new();
    let mut event_buf: Vec<RawInputEvent> = Vec::new();
    let raf_epoch = std::time::Instant::now();

    let fb_area: u64 = (fb.width as u64) * (fb.height as u64);
    let full_paint_threshold: u64 = fb_area * 95 / 100;

    // Cached layout: the Taffy solve (~4.8 ms) is recomputed only when
    // a layout-affecting change happened (structure / text / a layout
    // style) or the engine grew a cache (new font metrics).
    let mut layouts: std::collections::HashMap<NodeId, crate::layout::ComputedLayout> =
        std::collections::HashMap::new();

    // ================= Dual-core split =================
    //
    //   core 0 — engine thread (engine.rs): evdev input pump +
    //            forwarding, Device ownership (resource ensures,
    //            damage bookkeeping, display-list replay, PRESENT/
    //            fence pacing, boxart animation, coverage mask).
    //   core 1 — THIS thread: Boa (reconcile/effects), Taffy layout,
    //            display-list build.
    //
    // The boundary object is the FramePacket (display list + resource
    // requests + UI timings) through a latest-wins mailbox; raw input
    // flows the other way over an mpsc channel. Registries are shared
    // read-mostly (engine mutates, bumps a generation; we re-layout /
    // rebuild on change). A 10-25 ms JS reconcile no longer delays
    // input forwarding or frame presentation of the previous scene.
    use crate::runtime::packet::{
        FontNeed, FramePacket, FrameRequests, ImageNeed, Mailbox, PlanePacket, SharedCaches,
        TextNeed, UiTimings,
    };
    use std::sync::Mutex;
    use std::sync::atomic::AtomicU64;

    let caches = SharedCaches {
        fonts: Arc::new(Mutex::new(fonts)),
        images: Arc::new(Mutex::new(images)),
        text_cache: Arc::new(Mutex::new(text_cache)),
        generation: Arc::new(AtomicU64::new(0)),
    };
    let mailbox = Mailbox::new();
    let (input_tx, input_rx) = std::sync::mpsc::channel::<RawInputEvent>();
    let engine_handle = engine::spawn(
        engine::DeviceCarrier(device),
        caches.clone(),
        mailbox.clone(),
        input_tx,
        running.clone(),
        engine::EngineConfig {
            boxart_demo,
            content_mask_on,
            fb_area,
            full_paint_threshold,
            timeout,
        },
    );
    // Pin the UI thread to the LAST core (core 1 on the DE10's
    // dual-A9), leaving core 0 — where Linux parks IRQs — to the
    // engine's input/device work.
    if let Some(ids) = core_affinity::get_core_ids()
        && ids.len() > 1
        && let Some(last) = ids.last()
    {
        core_affinity::set_for_current(*last);
    }

    let mut last_sent_hash: Option<u64> = None;
    let mut last_seen_gen: u64 = u64::MAX; // force first layout/build
    let mut layout_gen: u64 = u64::MAX;
    let mut carry_events: Vec<RawInputEvent> = Vec::new();

    use std::time::Instant;
    while running.load(Ordering::SeqCst) {
        let t0 = Instant::now();

        // 0a'. Settle any completed DB queries FIRST so their promise
        //      continuations run inside this tick's job pump.
        if let Some(bridge) = context.get_data::<crate::db::bridge::DbBridge>().cloned() {
            bridge.drain(&mut context);
        }

        // 0a. Drive the JS job queue forward by one tick (see
        //     boa::tick_jobs for why not run_jobs()).
        if let Err(e) = boa::tick_jobs(&executor, &mut context) {
            tracing::warn!("tick_jobs error: {e}");
        }

        let t1 = Instant::now();

        // 0b. Input now arrives from the engine thread's evdev pump
        //     over the channel (plus anything the idle park below
        //     carried over). Dispatch to JS as before.
        event_buf.clear();
        event_buf.append(&mut carry_events);
        while let Ok(ev) = input_rx.try_recv() {
            event_buf.push(ev);
        }
        if !event_buf.is_empty() {
            match input_state.batch_dispatcher() {
                Some(dispatcher) => {
                    dispatch_input_batch(&router, &event_buf, &dispatcher, &mut context)?;
                    event_buf.clear();
                }
                None => {
                    for ev in event_buf.drain(..) {
                        dispatch_input(&input_state, &router, &ev, &mut context)?;
                    }
                }
            }
        }

        // 0c. Tick jobs again — handlers may have queued work.
        if let Err(e) = boa::tick_jobs(&executor, &mut context) {
            tracing::warn!("tick_jobs error: {e}");
        }

        // 0d. requestAnimationFrame queue.
        let raf_callbacks = raf_state.drain();
        if !raf_callbacks.is_empty() {
            let now_ms = raf_epoch.elapsed().as_secs_f64() * 1000.0;
            let arg = JsValue::from(now_ms);
            for cb in raf_callbacks {
                if let Err(e) = cb.call(&JsValue::undefined(), &[arg.clone()], &mut context) {
                    tracing::warn!("requestAnimationFrame callback threw: {e}");
                }
            }
            if let Err(e) = boa::tick_jobs(&executor, &mut context) {
                tracing::warn!("tick_jobs error: {e}");
            }
        }

        let t2 = Instant::now();

        // 0e. Advance tweens (mutates styles; damage picks it up).
        let _active_tweens = anim_mgr.tick(&ui_state);

        // 1. Resolve text style inheritance once for the frame.
        let text_styles = ui_state.with_tree(|tree| crate::text::resolve(tree, root));

        // 2. Collect FONT needs (walk only — the engine rasterises +
        //    uploads). Runtime warmup requests ride along.
        let mut font_needs: Vec<FontNeed> = ui_state.with_tree(|tree| {
            let mut needed: std::collections::HashMap<(String, u16), std::collections::HashSet<char>> =
                std::collections::HashMap::new();
            for (id, rs) in &text_styles {
                let Some(node) = tree.get(*id) else { continue };
                let NodeKind::Text { content } = &node.kind else { continue };
                let key = (rs.font_name.clone(), rs.px_size.round() as u16);
                let chars = needed.entry(key).or_default();
                for ch in content.chars() {
                    chars.insert(ch);
                }
            }
            needed
                .into_iter()
                .map(|((family, px_size), chars)| FontNeed { family, px_size, chars })
                .collect()
        });
        for req in warmup_queue.drain() {
            font_needs.push(FontNeed {
                family: req.family,
                px_size: req.px_size,
                chars: req.chars,
            });
        }

        let t3 = Instant::now();

        // 3. Collect IMAGE needs (walk only).
        let image_needs: Vec<ImageNeed> = ui_state.with_tree(|tree| {
            let mut out = Vec::new();
            fn walk(tree: &Tree, id: NodeId, out: &mut Vec<ImageNeed>) {
                let Some(node) = tree.get(id) else { return };
                if let NodeKind::Img { src } = &node.kind
                    && !src.is_empty()
                {
                    let sized = match (node.style.width, node.style.height) {
                        (Some(w), Some(h)) => Some((
                            (w.round() as i32).clamp(1, u16::MAX as i32) as u16,
                            (h.round() as i32).clamp(1, u16::MAX as i32) as u16,
                        )),
                        _ => None,
                    };
                    out.push(ImageNeed { src: src.clone(), sized });
                }
                for &child in &node.children {
                    walk(tree, child, out);
                }
            }
            walk(tree, root, &mut out);
            out
        });

        let t4 = Instant::now();

        // 4. Collect TEXT-RT needs: cache misses only (the engine
        //    measures + allocates + renders; entries appear next
        //    generation).
        let text_needs: Vec<TextNeed> = {
            let cache = caches.text_cache.lock().unwrap();
            ui_state.with_tree(|tree| {
                let mut out = Vec::new();
                for (id, rs) in &text_styles {
                    let Some(node) = tree.get(*id) else { continue };
                    let NodeKind::Text { content } = &node.kind else { continue };
                    if content.is_empty() {
                        continue;
                    }
                    let key = crate::text::CacheKey {
                        content: content.clone(),
                        font_name: rs.font_name.clone(),
                        px_size: rs.px_size.round() as u16,
                        color: rs.color.to_u32(),
                    };
                    if cache.lookup(&key).is_none() {
                        out.push(TextNeed { key, color: rs.color });
                    }
                }
                out
            })
        };

        let t5 = Instant::now();

        // 5. Layout — when tree-dirty OR the engine grew a cache (new
        //    font metrics can change text measures).
        let cache_gen = caches.generation();
        if ui_state.with_tree(|tree| tree.is_layout_dirty()) || cache_gen != layout_gen {
            let fonts_l = caches.fonts.lock().unwrap();
            let images_l = caches.images.lock().unwrap();
            layouts = ui_state.with_tree(|tree| {
                crate::layout::compute(
                    tree,
                    root,
                    fb.width as f32,
                    fb.height as f32,
                    &text_styles,
                    &fonts_l,
                    &images_l,
                )
            });
            drop(images_l);
            drop(fonts_l);
            ui_state.with_tree_mut(|tree| tree.clear_layout_dirty());
            layout_gen = cache_gen;
        }

        let t6 = Instant::now();

        // 6. Build the display list (the Send packet payload).
        let opacities = ui_state.with_tree(|tree| crate::style::resolve_opacity(tree, root));
        let transforms = ui_state.with_tree(|tree| crate::style::resolve_transforms(tree, root));
        let built = {
            // Lock order: images before text_cache (global order is
            // fonts -> images -> text_cache; see SharedCaches).
            let images_l = caches.images.lock().unwrap();
            let cache = caches.text_cache.lock().unwrap();
            ui_state.with_tree(|tree| {
                crate::display_list::build(
                    tree,
                    root,
                    &fb,
                    &layouts,
                    &text_styles,
                    &cache,
                    &images_l,
                    &opacities,
                    &transforms,
                    compositing,
                )
            })
        };
        let dl = built.content;
        let planes: Vec<PlanePacket> = built
            .planes
            .into_iter()
            .map(|p| PlanePacket {
                z: p.z,
                scene_hash: p.dl.scene_hash(),
                dl: p.dl,
                x: p.x,
                y: p.y,
                w: p.w,
                h: p.h,
            })
            .collect();
        // Content-layer hash: the engine's per-FB-slot skip compares
        // THIS (planes must not invalidate content slots — a plane
        // move repaints nothing).
        let content_hash = dl.scene_hash();
        // Idle-skip hash covers EVERYTHING the engine acts on: the
        // content list plus every plane's content AND geometry (a
        // pure plane move must still produce a packet — it becomes a
        // register write engine-side).
        let current_hash = {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            content_hash.hash(&mut h);
            for p in &planes {
                p.z.hash(&mut h);
                p.scene_hash.hash(&mut h);
                p.x.hash(&mut h);
                p.y.hash(&mut h);
                p.w.hash(&mut h);
                p.h.hash(&mut h);
            }
            h.finish()
        };
        let scene_dt = t6.elapsed();

        let has_requests =
            !font_needs.is_empty() || !image_needs.is_empty() || !text_needs.is_empty();

        // Idle skip: nothing changed since the last packet and there
        // is no resource work to request — park on the input channel
        // so a keypress wakes us instantly (better than the old fixed
        // 16 ms sleep).
        if Some(current_hash) == last_sent_hash && cache_gen == last_seen_gen && !has_requests {
            fps_counter.record_frame();
            if let Ok(ev) = input_rx.recv_timeout(Duration::from_millis(8)) {
                carry_events.push(ev);
            }
            continue;
        }
        last_sent_hash = Some(current_hash);
        last_seen_gen = cache_gen;

        mailbox.send(FramePacket {
            dl,
            scene_hash: content_hash,
            planes,
            requests: FrameRequests {
                fonts: font_needs,
                images: image_needs,
                texts: text_needs,
            },
            ui: UiTimings {
                jobs: t1 - t0,
                input: t2 - t1,
                text_prep: t3 - t2,
                images: t4 - t3,
                text_pop: t5 - t4,
                layout: t6 - t5,
                scene: scene_dt,
            },
        });
        fps_counter.record_frame();
    }

    info!("menu-ui: stopping (ui thread)");
    running.store(false, Ordering::SeqCst);
    match engine_handle.join() {
        Ok(r) => r?,
        Err(_) => {
            return Err(RuntimeError::Io(std::io::Error::other(
                "engine thread panicked",
            )));
        }
    }
    Ok(())
}
