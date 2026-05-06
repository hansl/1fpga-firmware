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
use menu_core_host::frame::Frame;
use menu_core_host::mem;
use menu_core_host::protocol::{BlendMode, Rect, Rgba};

use crate::font::{FontError, FontRegistry};
use crate::host::UiState;
use crate::image::ImageRegistry;
use crate::text::TextCache;
use crate::vdom::{NodeId, NodeKind, Tree};

/// Compile-time build identifier — package version. Logged at startup
/// so a glance at the device output confirms which binary is running.
/// (Git hash via build.rs is a future improvement.)
pub const BUILD_ID: &str = env!("CARGO_PKG_VERSION");

/// "Build canary" — paints a small color-cycling square in the
/// top-right corner of every frame so a glance at the screen confirms
/// the loop is alive AND the binary is fresh. Cycles through 6 colors
/// every 6 frames (≈100 ms at 60 fps).
fn paint_canary<'a>(
    frame: Frame<'a>,
    fb: &FramebufferConfig,
    frame_idx: u32,
) -> Result<Frame<'a>, DeviceError> {
    const SIZE: u16 = 24;
    const PALETTE: [Rgba; 6] = [
        Rgba::new(0xFF, 0x40, 0x40, 0xFF), // red
        Rgba::new(0xFF, 0xC0, 0x40, 0xFF), // amber
        Rgba::new(0xFF, 0xFF, 0x40, 0xFF), // yellow
        Rgba::new(0x40, 0xFF, 0x40, 0xFF), // green
        Rgba::new(0x40, 0xC0, 0xFF, 0xFF), // sky
        Rgba::new(0xC0, 0x40, 0xFF, 0xFF), // violet
    ];
    let color = PALETTE[(frame_idx as usize) % PALETTE.len()];
    let x = fb.width.saturating_sub(SIZE + 8);
    let y = 8u16;
    frame.fill_rect_unclipped(Rect::new(x, y, SIZE, SIZE), color, BlendMode::Opaque)
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

mod boa;

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
    let (mut context, _loader) = boa::build_context()?;
    let ui_state = UiState::default();
    context.insert_data(ui_state.clone());

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
    let mut images = ImageRegistry::new();
    while running.load(Ordering::SeqCst) {
        // 1. Resolve text style inheritance once for the frame.
        let text_styles = ui_state.with_tree(|tree| crate::text::resolve(tree, root));

        // 2. Prepare: ensure every (font, size) used by text nodes
        //    has a built+uploaded atlas. Mutates Device, must run
        //    before begin_frame.
        ui_state.with_tree(|tree| crate::text::prepare(tree, &text_styles, &mut fonts, &mut device))?;

        // 3. Prepare images: walk the tree, decode + upload any
        //    `<img>` whose `src` we haven't seen yet. Failures are
        //    cached so we don't retry every frame.
        ui_state.with_tree(|tree| prepare_images(tree, root, &mut images, &mut device));

        // 4. Populate text cache: allocate render-target textures for
        //    any (content, font, size, color) tuples we haven't seen
        //    yet. Returns the list of pendings to render this frame.
        let pendings = ui_state.with_tree(|tree| {
            text_cache.populate(tree, &text_styles, &fonts, &mut device)
        })?;

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

        // 6. Begin frame: render pending text into their RTs first
        //    (target = RT, glyphs, target = framebuffer), then paint
        //    the normal tree using the cached RTs and images.
        let frame = device.begin_frame();
        let frame = crate::paint::render_pending_text(frame, &pendings, &fonts)?;
        let frame = ui_state.with_tree(|tree| {
            crate::paint::paint(
                tree,
                root,
                &fb,
                &layouts,
                &text_styles,
                &text_cache,
                &images,
                frame,
            )
        })?;
        let frame = paint_canary(frame, &fb, frame_idx)?;
        frame.present()?.submit()?.wait_presented(timeout)?;
        ui_state.with_tree_mut(|t| t.clear_dirty());
        frame_idx = frame_idx.wrapping_add(1);
    }

    info!("menu-ui: stopping engine");
    device.stop()?;
    Ok(())
}
