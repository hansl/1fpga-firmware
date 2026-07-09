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
use menu_core_host::protocol::{self, LayerBlend, registers};
use menu_core_host::texture::TextureHandle;

/// Dump every diagnostic register relevant to a v2 hang. Called both
/// right after the v2 setup completes (sanity baseline) and on the
/// first fence timeout so we can see how the chip's state changed.
fn fmt_build_id(hash: u32) -> String {
    // BUILD_ID is now a 32-bit content hash of the menu-core source
    // tree (see `_gen-build-id` in the justfile). Stable across
    // rebuilds when source hasn't changed, which is what we want
    // for verifying "deployed RBF == source tree" — but the value
    // has no time encoding any more, so display as raw hex. Match
    // against the build's own hash via:
    //     cat cores/menu-core-fpga/build_id.svh
    if hash == 0 || hash == u32::MAX {
        format!("{:#010X} (unset)", hash)
    } else {
        format!("{:#010X}", hash)
    }
}

fn dump_compositor_regs(device: &Device, label: &str) {
    let regs = device.register_block();
    let status = regs.read32(registers::STATUS);
    let error_info = regs.read32(registers::ERROR_INFO);
    let ring_head = regs.read32(registers::RING_HEAD);
    let ring_tail = regs.read32(registers::RING_TAIL);
    let fence_val = regs.read32(registers::FENCE_VALUE);
    let frame_cnt = regs.read32(registers::FRAME_COUNT);
    let vsync = regs.read32(registers::VSYNC_COUNT);
    let fb_state = regs.read32(registers::FB_STATE);
    let comp_status = regs.read32(registers::COMPOSITOR_STATUS);
    let comp_fence = regs.read32(registers::COMPOSITE_FENCE);
    let layer_dbg = regs.read32(registers::LAYER_DEBUG);
    let build_id = regs.read32(registers::BUILD_ID);

    // LAYER_DEBUG carries the ring fetcher's state + pending opcode +
    // bitmask/diff busy flags so we can tell exactly which pipeline
    // stage is wedged when the fence times out. See the bit layout
    // in menu_core.sv near layer_descriptors_i.
    let f_state = layer_dbg & 0xF;
    let f_opcode = (layer_dbg >> 4) & 0xFF;
    let f_busy = (layer_dbg >> 12) & 0x1;
    let f_err = (layer_dbg >> 13) & 0x1;
    let bm_busy = (layer_dbg >> 16) & 0x1;
    let diff_busy = (layer_dbg >> 17) & 0x1;
    let owner = (layer_dbg >> 18) & 0x3;
    let outstanding = (layer_dbg >> 20) & 0xFF;
    let ddram_busy = (layer_dbg >> 28) & 0x1;
    let pipe_idle = (layer_dbg >> 29) & 0x1;
    let owner_name = match owner {
        0 => "FETCH",
        1 => "BLIT",
        2 => "TEX",
        3 => "SCAN",
        _ => "???",
    };
    let state_name = match f_state {
        0 => "IDLE",
        1 => "FETCH_HEADER",
        2 => "WAIT_HEADER",
        3 => "DECODE",
        4 => "FETCH_ARG",
        5 => "WAIT_ARG",
        6 => "FETCH_DESC",
        7 => "WAIT_DESC",
        8 => "BLIT_DISPATCH",
        9 => "BLIT_WAIT",
        10 => "RETIRE",
        11 => "HALT",
        _ => "???",
    };
    tracing::info!(
        "[diag {label}] BUILD_ID={} STATUS={status:#010X} ERROR_INFO={error_info:#010X} \
         RING_HEAD={ring_head:#010X} RING_TAIL={ring_tail:#010X} \
         FENCE={fence_val} FRAME_COUNT={frame_cnt} VSYNC={vsync} \
         FB_STATE={fb_state:#010X} COMP_STATUS={comp_status:#010X} \
         COMP_FENCE={comp_fence} \
         fetcher.state={state_name}({f_state}) opcode={f_opcode:#04X} \
         fb={f_busy} fe={f_err} bm_busy={bm_busy} diff_busy={diff_busy} \
         owner={owner_name}({owner}) outst={outstanding} ddram_busy={ddram_busy} pipe_idle={pipe_idle}",
        fmt_build_id(build_id),
    );
}

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
/// Walks the tree, decode+upload-ing any `<img>` whose `src` isn't
/// already cached. Returns the number of decode+upload operations
/// performed this frame — should be 0 in steady state. A non-zero
/// value indicates either first-time content or an animated source
/// changing every frame.
fn prepare_images(
    tree: &Tree,
    root: NodeId,
    images: &mut ImageRegistry,
    device: &mut Device,
) -> u32 {
    fn walk(
        tree: &Tree,
        id: NodeId,
        images: &mut ImageRegistry,
        device: &mut Device,
        loaded: &mut u32,
    ) {
        let Some(node) = tree.get(id) else {
            return;
        };
        if let NodeKind::Img { src } = &node.kind
            && images.get(src).is_none()
            && !src.is_empty()
        {
            let _ = images.get_or_load(device, src);
            *loaded += 1;
        }
        for &child in &node.children {
            walk(tree, child, images, device, loaded);
        }
    }
    let mut loaded = 0;
    walk(tree, root, images, device, &mut loaded);
    loaded
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
    // FB resolution: explicit override > HDMI native. The framework
    // reads pixels from the FB and feeds ASCAL, which upscales to the
    // active HDMI mode — so render < HDMI is "free" beyond the loss
    // of visual crispness on text/icons.
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

    let timeout = Duration::from_millis(500);
    let mut frame_idx: u32 = 0;

    // Phase 2 smoke test: when MENU_UI_COMPOSITOR_V2_SMOKE_TEST is set,
    // append INVALIDATE_ALL + MASK_COMMIT to every frame so the new
    // ring opcodes flow through ring_fetcher and the dirty_bitmask
    // module. Default behavior is unchanged — the FPGA boots with
    // active bank = all-1, so the smoke test's repeating
    // {all-1 → swap, building cleared, swap-back} cycle keeps every
    // scanline marked dirty and the displayed image matches v1. If
    // ring_fetcher fails to decode either opcode it'll halt with
    // ERR_UNKNOWN_OPCODE and the fence wait below will time out;
    // that's the failure signal we're looking for.
    let smoke_test_phase2 = std::env::var("MENU_UI_COMPOSITOR_V2_SMOKE_TEST")
        .is_ok_and(|v| !v.is_empty() && v != "0");
    if smoke_test_phase2 {
        info!("compositor-v2 Phase 2 smoke test enabled: appending INVALIDATE_ALL + MASK_COMMIT to every frame");
    }

    // Phase 4 (COMPOSITOR_V2.md §11.4): full v2 paint path. Host paints
    // into a per-layer source RT instead of the FB; the FPGA's
    // compositor reads that RT and writes the scanout FB. Gated on the
    // MENU_UI_COMPOSITOR_V2 env var while the path stabilises — once
    // shipped, the v1 paint path goes away (Phase 5).
    //
    // When enabled:
    //   - allocate a single full-size L0 layer source RT at startup
    //     and commit a `tex_id = rt.id` layer descriptor pointing at it,
    //   - paint into the RT each frame (set_target instead of
    //     set_target_framebuffer),
    //   - emit INVALIDATE_RECT / INVALIDATE_ALL + MASK_COMMIT so the
    //     compositor knows which scanlines to repaint,
    //   - drop PRESENT — fb_swapper is now driven by the compositor's
    //     frame-done pulse, not by the host,
    //   - flip SCANOUT_FB_SELECT=1 + COMPOSITOR_CONTROL.enable=1.
    let composite_v2 = std::env::var("MENU_UI_COMPOSITOR_V2")
        .is_ok_and(|v| !v.is_empty() && v != "0");
    let layer_rt: Option<[TextureHandle; 2]> = if composite_v2 {
        info!("compositor-v2 Phase 4 paint path enabled");
        let rt_a = device.create_layer_rt(fb.width, fb.height)?;
        let rt_b = device.create_layer_rt(fb.width, fb.height)?;
        info!(
            "allocated L0 layer source RTs: A tex_id={} phys={:#010X}, B tex_id={} phys={:#010X}, {}x{}",
            rt_a.id, rt_a.phys_addr, rt_b.id, rt_b.phys_addr, fb.width, fb.height,
        );
        // Start with the compositor reading RT-A. The host will
        // paint into RT-B first, then swap on each frame closeout.
        let desc = protocol::LayerDescriptor::textured(rt_a.id, 0, 0, fb.width, fb.height)
            .with_blend(LayerBlend::Opaque);
        device.set_layer(0, &desc)?;
        device.commit_layers();
        // Compositor's scanout triple-buffer base. We alias the v1
        // FB0/1/2 region because the v1 paint path is now disabled —
        // no collision. The compositor writes
        //   FB_BASE + render_idx × 0x800000.
        let fb_base = base + mem::FB0_OFFSET as u32;
        device.set_compositor_fb_base(fb_base);
        device.set_scanout_select(true);
        device.set_compositor_enable(true);
        info!(
            "compositor-v2 path active: FB_BASE={:#010X}, SCANOUT_FB_SELECT=1, COMPOSITOR_CONTROL.enable=1",
            fb_base,
        );
        // Let the compositor run for a few raster periods so we can
        // observe whether its scanout pipeline is actually advancing
        // before the first host frame submits commands. If COMP_FENCE
        // is non-zero / advancing here, the compositor path is fine;
        // if it's stuck at 0, the v2 setup itself is broken.
        std::thread::sleep(Duration::from_millis(100));
        dump_compositor_regs(&device, "post-setup");
        Some([rt_a, rt_b])
    } else {
        None
    };
    // Double-buffer index: which RT the host paints into (the
    // compositor reads the OTHER one). Flips after each frame.
    let mut rt_write_idx: usize = 1;

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
    // The t_input bucket got large enough in production (~20 ms/frame
    // steady state) that conflating evdev syscalls with React/JS work
    // hid the cause. Split into four sub-buckets so the timing log
    // can localize the cost:
    //   drain    = pump.drain (evdev fd reads + translate)
    //   dispatch = JS dispatcher call(s) for the drained batch
    //   jobs2    = boa::tick_jobs that drains React's microtask queue
    //              triggered by input handlers
    //   raf      = RAF callback list + the tick_jobs that follows
    // Their sum should approximate the legacy t_input value.
    let mut t_drain = Duration::ZERO;
    let mut t_dispatch = Duration::ZERO;
    let mut t_jobs2 = Duration::ZERO;
    let mut t_raf = Duration::ZERO;
    let mut t_text_prep = Duration::ZERO;
    let mut t_images = Duration::ZERO;
    // Diagnostic: how many image decode+upload operations actually
    // happen per frame. Hits the cache for already-loaded sources, so
    // steady-state should be 0. If it's not, t_images is being eaten
    // by re-uploads (an image src changing every frame, etc.).
    let mut images_loaded_total: u64 = 0;
    let mut t_text_pop = Duration::ZERO;
    let mut t_layout = Duration::ZERO;
    let mut t_paint = Duration::ZERO;
    let mut t_fence = Duration::ZERO;
    let mut t_scanout = Duration::ZERO;
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
        let t1a = Instant::now();
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
        let t1b = Instant::now();

        // 0c. Tick jobs again — handlers above may have called
        //     setTimeout (directly or via React's setState scheduler).
        if let Err(e) = boa::tick_jobs(&executor, &mut context) {
            tracing::warn!("tick_jobs error: {e}");
        }

        let t1c = Instant::now();

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

        t_drain    += t1a - t1;
        t_dispatch += t1b - t1a;
        t_jobs2    += t1c - t1b;
        t_raf      += t2  - t1c;

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
        //    cached so we don't retry every frame. The returned count
        //    feeds the diagnostic so we can tell cache hits from
        //    repeated re-decodes.
        let loaded_this_frame =
            ui_state.with_tree(|tree| prepare_images(tree, root, &mut images, &mut device));
        images_loaded_total += loaded_this_frame as u64;

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
        let render_idx = (device.fb_state().render as usize).min(2);
        let opacities = ui_state.with_tree(|tree| crate::style::resolve_opacity(tree, root));
        let transforms = ui_state.with_tree(|tree| crate::style::resolve_transforms(tree, root));
        let current_scene = ui_state.with_tree(|tree| {
            damage::compute_scene(tree, root, &layouts, &text_styles, &opacities, &transforms)
        });
        let current_hash = current_scene.hash();
        let damage_idx = if composite_v2 { rt_write_idx } else { render_idx };
        if scene_hash_per_fb[damage_idx] == Some(current_hash) {
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
        // v2: redirect host paints into the L0 layer source RT. v1:
        // paint into FB[render_idx] as before. The damage / clip
        // coordinate system is identical (the RT is exactly FB-sized),
        // so the rect logic below is target-agnostic.
        let frame = match &layer_rt {
            Some(rts) => frame.set_target(&rts[rt_write_idx])?,
            None => frame.set_target_framebuffer()?,
        };

        // Track what we actually painted this frame for the timing
        // log's damage stats. Initialized in each arm of the match
        // below.
        let paint_rect_count: u32;
        let paint_area_px: u64;
        let mut paint_full = false;

        let damage_paint_plan: Option<Vec<damage::PixelRect>> =
            match &scene_per_fb[damage_idx] {
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
                // issued. In v2 mode, an invalidate_rect alongside
                // the clip tells the compositor's dirty bitmask that
                // these scanlines need recomposition this frame.
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
                            f,
                        )
                    })?;
                    let f = f.clear_clip()?;
                    frame = if composite_v2 {
                        f.invalidate_rect(clip_rect)?
                    } else {
                        f
                    };
                }
                frame
            }
            None => {
                paint_full = true;
                paint_rect_count = 1;
                paint_area_px = fb_area;
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
                        None,
                        frame,
                    )
                })?;
                if composite_v2 { f.invalidate_all()? } else { f }
            }
        };

        let t7 = Instant::now();

        // Frame closeout. v1: PRESENT + wait_presented (the fb_swapper
        // PRESENT op rotates display↔render, FRAME_COUNT advances on
        // the next vsync). v2: MASK_COMMIT (promote queued invalidates
        // into the compositor's active dirty bitmask) + fence-only
        // wait — the compositor drives fb_swapper itself when its
        // scanout_writer finishes a frame, asynchronously from us.
        //
        // The legacy Phase 2 smoke test is folded into the v1 branch:
        // it ran a no-op {INVALIDATE_ALL → MASK_COMMIT} to exercise the
        // ring opcodes without changing visible output. v2 always emits
        // those ops with real semantics, so the smoke test is redundant
        // when composite_v2 is on.
        let (fence_dt, scanout_dt) = if composite_v2 {
            // Submit the frame's blits + INVALIDATE_RECTs and wait for
            // the ring to drain. MASK_COMMIT is deliberately NOT part
            // of this submission: publishing the invalidates while
            // layer 0 still points at the PREVIOUS RT would let the
            // compositor drain the damaged scanlines with stale
            // content — and clear their per-slot pending bits, so the
            // real content would never be composited.
            let token = frame.submit()?;
            let t_submit = Instant::now();
            match token.wait(timeout) {
                Ok(()) => {}
                Err(e) => {
                    dump_compositor_regs(&device, "fence-timeout");
                    let ring_head = device.register_block()
                        .read32(menu_core_host::protocol::registers::RING_HEAD);
                    let bytes = device.read_ring_bytes(ring_head, 32);
                    let hex: Vec<String> = bytes.iter()
                        .map(|b| format!("{:02X}", b))
                        .collect();
                    tracing::info!(
                        "ring[{:#06X}..+32] = {}",
                        ring_head,
                        hex.join(" "),
                    );
                    return Err(e.into());
                }
            }
            // Blits are done — swap the compositor to read from the
            // RT we just finished painting, and flip the write index
            // so the next frame paints into the other RT.
            if let Some(rts) = &layer_rt {
                let read_idx = rt_write_idx;
                rt_write_idx = 1 - rt_write_idx;
                let desc = protocol::LayerDescriptor::textured(
                    rts[read_idx].id, 0, 0, fb.width, fb.height,
                ).with_blend(LayerBlend::Opaque);
                device.set_layer(0, &desc)?;
                device.commit_layers();
            }
            // NOW publish the damage. The layer table points at the
            // new RT, so every scanline the compositor drains from
            // here on composites current content.
            let token = device.begin_frame().mask_commit()?.submit()?;
            if let Err(e) = token.wait(timeout) {
                dump_compositor_regs(&device, "mask-commit-fence-timeout");
                return Err(e.into());
            }
            let t_done = Instant::now();
            (t_done - t_submit, Duration::ZERO)
        } else {
            let frame_for_submit = if smoke_test_phase2 {
                frame.invalidate_all()?.mask_commit()?
            } else {
                frame
            };
            let (_count, fdt, sdt) = frame_for_submit
                .present()?
                .submit()?
                .wait_presented_timed(timeout)?;
            (fdt, sdt)
        };

        scene_hash_per_fb[damage_idx] = Some(current_hash);
        scene_per_fb[damage_idx] = Some(current_scene);
        ui_state.with_tree_mut(|t| t.clear_dirty());
        fps_counter.record_frame();

        sum_paint_rect_count += paint_rect_count as u64;
        sum_paint_area_px += paint_area_px;
        if paint_full {
            count_full_paints += 1;
        }

        t_jobs += t1 - t0;
        // t_drain/dispatch/jobs2/raf were already accumulated above
        // (split out of the legacy t_input bucket).
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
            // The four input-related buckets (drain/dispatch/jobs2/raf)
            // replace the legacy `input=` total. Their sum should
            // approximate what the old `input=` printed; if one is the
            // outlier we now see which one.
            let t_input_total = t_drain + t_dispatch + t_jobs2 + t_raf;
            tracing::info!(
                "frame timings (us avg over {n}): jobs={} drain={} dispatch={} jobs2={} raf={} (input={}) text_prep={} images={} (loaded={}) text_pop={} layout={} paint={} fence={} scanout={} total={}",
                avg(t_jobs),
                avg(t_drain),
                avg(t_dispatch),
                avg(t_jobs2),
                avg(t_raf),
                avg(t_input_total),
                avg(t_text_prep),
                avg(t_images),
                images_loaded_total,
                avg(t_text_pop),
                avg(t_layout),
                avg(t_paint),
                avg(t_fence),
                avg(t_scanout),
                avg(t_jobs + t_input_total + t_text_prep + t_images + t_text_pop + t_layout + t_paint + t_fence + t_scanout),
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
            // v2-path swap observability: confirm fb_swapper is
            // actually rotating display/render in v2 mode (HDMI
            // stays black if it doesn't, because MISTER_FB reads
            // display_idx but the compositor writes render_idx).
            if composite_v2 {
                let fb_state = device.register_block().read32(registers::FB_STATE);
                let comp_fence = device.register_block().read32(registers::COMPOSITE_FENCE);
                let comp_status = device.register_block().read32(registers::COMPOSITOR_STATUS);
                // Probe L0 RT (host's paint target) and the FB
                // currently being displayed (compositor's scanout
                // output). Compare both sides:
                //   - L0 nonzero & FB nonzero & match: chain works,
                //     HDMI black is a downstream issue (ASCAL, HDMI
                //     mux, etc.).
                //   - L0 nonzero, FB all-zero: compositor isn't
                //     writing pixels — painter pipeline bug.
                //   - L0 all-zero: host isn't painting into the RT.
                let display_idx = (fb_state & 0x3) as u32;
                let fb_base = base + mem::FB0_OFFSET as u32
                            + display_idx * 0x0080_0000;
                // Sample at 4 positions across scanline 0 of L0 RT and
                // the currently-displayed FB. Same offsets in both —
                // if the host paints the full width, L0 has data at
                // every offset; if scanout writes the full width, FB
                // does too. Comparing the two pinpoints which side is
                // dropping data.
                //   col 0   = top-left
                //   col 480 = 25% across (1920*0.25)
                //   col 960 = middle (1920*0.5)
                //   col 1440 = 75% across
                let stride_bytes = (fb.width as u32) * 4;
                let mut l0_samples = Vec::with_capacity(4);
                let mut fb_samples = Vec::with_capacity(4);
                for col in [0u32, 480, 960, 1440] {
                    let off = col * 4; // BGRA = 4 bytes per pixel
                    let l0_b = device.read_tex_pool_bytes(off, 8).unwrap_or_default();
                    let fb_b = device.read_fb_bytes(fb_base, off, 8).unwrap_or_default();
                    l0_samples.push((col, l0_b));
                    fb_samples.push((col, fb_b));
                }
                let _ = stride_bytes;
                let fmt = |samples: &Vec<(u32, Vec<u8>)>| {
                    samples.iter()
                        .map(|(col, b)| {
                            let hex = b.iter().map(|x| format!("{:02X}", x))
                                .collect::<Vec<_>>().join("");
                            format!("@{col}={hex}")
                        })
                        .collect::<Vec<_>>().join(" ")
                };
                tracing::info!(
                    "v2: FB_STATE={:#010X} (display={} render={} ready={}) \
                     COMP_FENCE={} COMP_STATUS={:#010X} \
                     L0: {} FB{}: {}",
                    fb_state,
                    fb_state & 0x3,
                    (fb_state >> 2) & 0x3,
                    (fb_state >> 4) & 0x3,
                    comp_fence,
                    comp_status,
                    fmt(&l0_samples),
                    display_idx,
                    fmt(&fb_samples),
                );
            }
            t_jobs = Duration::ZERO;
            t_drain = Duration::ZERO;
            t_dispatch = Duration::ZERO;
            t_jobs2 = Duration::ZERO;
            t_raf = Duration::ZERO;
            t_text_prep = Duration::ZERO;
            t_images = Duration::ZERO;
            images_loaded_total = 0;
            t_text_pop = Duration::ZERO;
            t_layout = Duration::ZERO;
            t_paint = Duration::ZERO;
            t_fence = Duration::ZERO;
            t_scanout = Duration::ZERO;
            sum_paint_rect_count = 0;
            sum_paint_area_px = 0;
            count_full_paints = 0;
        }
    }

    info!("menu-ui: stopping engine");
    // Drop the v2 compositor + scanout select back to v1 so a
    // subsequent boot (or a v1-only tool like layer-draw) comes up
    // in the expected state — symmetric with `compositor-v2-test`.
    if composite_v2 {
        device.set_compositor_enable(false);
        device.set_scanout_select(false);
        device.clear_layers();
    }
    device.stop()?;
    Ok(())
}
