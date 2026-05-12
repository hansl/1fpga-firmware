//! `one_fpga_menu_core` — host-side test driver for the menu-core
//! FPGA bitstream.
//!
//! Each subcommand exercises a slice of the protocol on real hardware.
//! All of them open the device through [`menu_core_host::Device`],
//! configure the framebuffer to match the active HDMI mode, and then
//! drive the ring via [`menu_core_host::Frame`]. See the docs on each
//! subcommand for the visual outcome to verify.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use clap::{Parser, Subcommand};
use clap_verbosity_flag::Level as VerbosityLevel;
use clap_verbosity_flag::{LogLevel, Verbosity};
use tracing::info;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::fmt::Subscriber;

use menu_core_host::device::{Device, DeviceConfig, FramebufferConfig};
use menu_core_host::error::DeviceError;
use menu_core_host::frame::CopyOpts;
use menu_core_host::protocol::{self, BlendMode, Rect, Rgba, TextureFormat, registers};
use menu_core_host::texture::TextureSpec;
use menu_core_host::{device, mem};

// Crates the binary itself doesn't reference, but the package depends
// on for the lib (`text`) or for the sibling `menu_demo` binary. The
// workspace lint is enabled per-target, so we declare them here.
use cyclone_v as _;
use fontdue as _;
use thiserror as _;
#[cfg(test)]
use pretty_assertions as _;

#[derive(Copy, Clone, Debug, Default)]
pub struct NoneLevel;

impl LogLevel for NoneLevel {
    fn default() -> Option<clap_verbosity_flag::Level> {
        None
    }
}

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
pub struct Flags {
    #[command(flatten)]
    pub verbose: Verbosity<clap_verbosity_flag::InfoLevel>,

    /// Print the resolved protocol layout and exit (no hardware access).
    #[clap(long)]
    pub print_layout: bool,

    /// Override the reserved DDR3 base address (default: 0x30000000).
    /// Must be 32-MB-aligned and within the kernel's reserved region.
    #[clap(long, value_parser = parse_u32_hex_or_dec)]
    pub base_addr: Option<u32>,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Bring up the LW_H2F bridge, validate the menu-core ID register,
    /// and pattern-test the entire register window. Requires root and
    /// a flashed menu-core RBF on the FPGA.
    Probe,

    /// Push NOP/FENCE/PRESENT commands through the DDR3 ring and
    /// verify FENCE_VALUE / FRAME_COUNT update as the FPGA retires them.
    RingTest,

    /// FILL_RECT a half-screen red rectangle on a black background.
    /// Visual: red rectangle on HDMI.
    DrawTest,

    /// Upload a 64×64 RGBA8888 checkerboard texture and COPY_RECT it
    /// onto the framebuffer.
    TextureTest,

    /// Upload a 64×64 A8 alpha gradient and COPY_RECT it with a red
    /// tint onto a black background.
    A8Test,

    /// Upload a 64×64 A8 alpha gradient and COPY_RECT it with a red
    /// tint and SrcAlpha blend over a blue background.
    BlendTest,

    /// Render "Hello, 1FPGA!" via per-glyph COPY_RECT against a Noto
    /// Sans atlas with SrcAlpha blend.
    TextTest,

    /// Animate "Hello, 1FPGA!" left-right with ease-in-out cubic
    /// timing for ~5 s, reporting FPS at the end.
    TextAnim,

    /// Visual test for SET_CLIP / CLEAR_CLIP using a centred clip rect.
    ClipTest,

    /// Visual test for SET_RENDER_TARGET. Allocates a 200×200
    /// RGBA8888 render target, paints four colored quadrants into it,
    /// then COPY_RECTs the cached texture to the centre of the FB.
    /// Validates the FPGA's RTT path end-to-end.
    RttTest,

    /// Phase 2a step 2 round-trip: write a single solid layer, commit,
    /// sample LAYER_DEBUG over 1s to confirm the FPGA-side DMA ticks
    /// at frame rate, then re-commit with count=0 and confirm the DMA
    /// halts. No visual change — the colour-bar test pattern stays
    /// (the renderer lands in step 3).
    LayerProbe,

    /// Phase 2a step 3 visual: holds a 5-layer scene (navy background,
    /// header bar, three overlapping coloured panels) on HDMI and
    /// prints the compositor's frame rate every second. Useful for
    /// eyeballing z-order behaviour. Runs until Ctrl-C or 60 s.
    LayerDraw,

    /// Phase 2c step 1 visual: uploads a 256×256 BGRA texture (red
    /// disc with smooth alpha falloff) and commits a 4-layer scene.
    /// PASS = a soft red disc visibly fading into the navy background
    /// near its edges (SrcAlpha blend). Sharp solid panels above /
    /// below the disc, FPS readout to confirm timing closure.
    LayerTexProbe,

    /// Phase 2c step 2 visual: uploads a 256×256 A8 texture (single-
    /// channel alpha disc) and commits one tinted-A8 textured layer
    /// over a navy background + header. The layer's `color` field is
    /// the tint colour — texture_unit pre-bakes (tint.rgb, alpha) per
    /// pixel into the line buffer, and the painter SrcAlpha-blends
    /// against the topmost solid. PASS = a soft white disc fading
    /// into the navy background, no fringing or color shifts.
    LayerA8Probe,

    /// Phase 2c step 3 visual: four textured layers in a row, with
    /// the rightmost three overlapping. Exercises the full
    /// 4-buffer back-to-front blend pipeline (MAX_TEXTURED = 4).
    /// PASS = leftmost red disc isolated, then a chain of three
    /// overlapping discs (red → white-blended-on-red → next-disc-
    /// blended-on-white) demonstrating each pipeline stage's
    /// SrcAlpha composition.
    LayerMultiTexProbe,
}

fn parse_u32_hex_or_dec(s: &str) -> Result<u32, std::num::ParseIntError> {
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u32::from_str_radix(hex, 16)
    } else {
        s.parse::<u32>()
    }
}

fn main() {
    if let Some(cores) = core_affinity::get_core_ids() {
        let core = cores.get(1).or_else(|| cores.first()).copied();
        if let Some(c) = core {
            core_affinity::set_for_current(c);
        }
    }

    let opts = Flags::parse();

    let level_filter = match opts.verbose.log_level() {
        Some(VerbosityLevel::Error) => LevelFilter::ERROR,
        Some(VerbosityLevel::Warn) => LevelFilter::WARN,
        Some(VerbosityLevel::Info) => LevelFilter::INFO,
        Some(VerbosityLevel::Debug) => LevelFilter::DEBUG,
        None | Some(VerbosityLevel::Trace) => LevelFilter::TRACE,
    };

    Subscriber::builder()
        .with_env_filter(
            EnvFilter::builder()
                .with_default_directive(level_filter.into())
                .from_env_lossy(),
        )
        .with_ansi(true)
        .with_writer(std::io::stderr)
        .init();

    tracing::debug!(?opts);

    let base = opts.base_addr.unwrap_or(mem::DEFAULT_BASE);

    info!(
        "menu-core host driver — protocol v{} (ID = {:#010X})",
        protocol::PROTOCOL_VERSION,
        protocol::ID_VALUE
    );

    if opts.print_layout {
        print_layout(base);
        return;
    }

    let result: Result<(), Box<dyn std::error::Error>> = match opts.command {
        Some(Command::Probe) => probe(base).map_err(Into::into),
        Some(Command::RingTest) => ring_test(base).map_err(Into::into),
        Some(Command::DrawTest) => draw_test(base).map_err(Into::into),
        Some(Command::TextureTest) => texture_test(base).map_err(Into::into),
        Some(Command::A8Test) => a8_test(base).map_err(Into::into),
        Some(Command::BlendTest) => blend_test(base).map_err(Into::into),
        Some(Command::TextTest) => text_test(base).map_err(Into::into),
        Some(Command::TextAnim) => text_anim(base).map_err(Into::into),
        Some(Command::ClipTest) => clip_test(base).map_err(Into::into),
        Some(Command::RttTest) => rtt_test(base).map_err(Into::into),
        Some(Command::LayerProbe) => layer_probe(base).map_err(Into::into),
        Some(Command::LayerDraw) => layer_draw(base).map_err(Into::into),
        Some(Command::LayerTexProbe) => layer_tex_probe(base).map_err(Into::into),
        Some(Command::LayerA8Probe) => layer_a8_probe(base).map_err(Into::into),
        Some(Command::LayerMultiTexProbe) => layer_multitex_probe(base).map_err(Into::into),
        None => {
            info!(
                "no subcommand given — re-run with `probe`, `ring-test`, `draw-test`, …, or `--print-layout`"
            );
            Ok(())
        }
    };

    if let Err(e) = result {
        tracing::error!("{e}");
        std::process::exit(1);
    }
}

/// Open the device pre-configured for `base`, with the framebuffer
/// programmed to match the live video mode and the engine started.
fn open_ready(base: u32) -> Result<Device, DeviceError> {
    let mut device = Device::open_with(DeviceConfig {
        base_phys_addr: base,
        ..DeviceConfig::default()
    })?;
    let info = device.video_info();
    let fb = FramebufferConfig::for_video(info, base);
    device.configure_framebuffer(fb)?;
    device.start()?;
    Ok(device)
}

fn probe(base: u32) -> Result<(), DeviceError> {
    let device = Device::open_with(DeviceConfig {
        base_phys_addr: base,
        ..DeviceConfig::default()
    })?;
    let regs = device.register_block();

    let id = regs.read32(registers::ID);
    let magic = id >> 16;
    let version = id & 0xFFFF;
    println!("ID:     {id:#010X}  (MAGIC={magic:#06X} VERSION={version})");

    let status = regs.read32(registers::STATUS);
    println!("STATUS: {status:#010X}");

    let info = device.video_info();
    println!("VIDEO_INFO: {}×{}", info.width, info.height);

    // CONTROL — only bit 0 latches; tested via Device::start/stop.
    regs.write32(registers::CONTROL, registers::CONTROL_ENABLE);
    let after_set = regs.read32(registers::CONTROL);
    if after_set != registers::CONTROL_ENABLE {
        return Err(DeviceError::Hardware {
            kind: menu_core_host::error::HardwareErrorKind::Other(0),
            info: after_set,
        });
    }
    regs.write32(registers::CONTROL, 0);
    println!("CONTROL scratch round-trip: OK");

    // Pattern-test every R/W slot. Skip read-only / pulsed slots.
    let read_only = [
        registers::ID,
        registers::STATUS,
        registers::CONTROL,
        registers::ERROR_INFO,
        registers::VSYNC_COUNT,
        registers::FRAME_COUNT,
        registers::VIDEO_INFO,
        registers::FB_STATE,
        registers::RING_HEAD,
        registers::RING_KICK,
        registers::FENCE_VALUE,
    ];
    let mut tested = 0usize;
    let mut failed = 0usize;
    for off in (0..registers::REGISTER_WINDOW_SIZE).step_by(4) {
        if read_only.contains(&off) {
            continue;
        }
        let pat = 0xCAFE_0000u32 | (off as u32);
        regs.write32(off, pat);
        let got = regs.read32(off);
        if got != pat {
            tracing::warn!("offset {off:#04X}: wrote {pat:#010X}, read {got:#010X}");
            failed += 1;
        }
        tested += 1;
        regs.write32(off, 0);
    }
    println!("Window R/W pattern: {}/{} slots OK", tested - failed, tested);
    if failed != 0 {
        return Err(DeviceError::Hardware {
            kind: menu_core_host::error::HardwareErrorKind::Other(0xFF),
            info: failed as u32,
        });
    }
    Ok(())
}

fn ring_test(base: u32) -> Result<(), DeviceError> {
    let mut device = open_ready(base)?;
    let info = device.video_info();
    println!(
        "Ring init: RING_BASE={:#010X} RING_SIZE={}",
        base + mem::RING_OFFSET as u32,
        mem::RING_SIZE
    );
    println!("Video: {}×{}", info.width, info.height);

    let frame_count_at_start = device.frame_count();
    let token = device
        .begin_frame()
        .present()?
        .submit()?;
    let fence_value = token.fence_value();
    let started = Instant::now();
    let final_count = token.wait_presented(Duration::from_millis(500))?;
    println!(
        "FENCE_VALUE reached {fence_value:#010X} in {} µs",
        started.elapsed().as_micros()
    );
    println!(
        "FRAME_COUNT: {final_count} (expected ≥ {})",
        frame_count_at_start + 1
    );
    println!("M2b: OK");
    Ok(())
}

fn draw_test(base: u32) -> Result<(), DeviceError> {
    let mut device = open_ready(base)?;
    let info = device.video_info();
    let mode_w = info.width;
    let mode_h = info.height;
    println!("Video: {mode_w}×{mode_h}");

    let rect_w = mode_w / 2;
    let rect_h = mode_h / 2;
    let rect_x = mode_w / 4;
    let rect_y = mode_h / 4;

    device
        .begin_frame()
        .fill_rect_unclipped(
            Rect::new(0, 0, mode_w, mode_h),
            Rgba::BLACK,
            BlendMode::Opaque,
        )?
        .fill_rect_unclipped(
            Rect::new(rect_x, rect_y, rect_w, rect_h),
            Rgba::new(0xFF, 0x00, 0x00, 0xFF),
            BlendMode::Opaque,
        )?
        .present()?
        .submit()?
        .wait_presented(Duration::from_millis(10_000))?;

    println!(
        "M2c1: check HDMI — {rect_w}×{rect_h} red square at ({rect_x}, {rect_y})"
    );
    Ok(())
}

fn texture_test(base: u32) -> Result<(), DeviceError> {
    let mut device = open_ready(base)?;
    let info = device.video_info();
    println!("Video: {}×{}", info.width, info.height);

    const TEX_W: u16 = 64;
    const TEX_H: u16 = 64;
    const CELL: u16 = 8;
    let red = Rgba::new(0xFF, 0x00, 0x00, 0xFF);
    let yellow = Rgba::new(0xFF, 0xFF, 0x00, 0xFF);
    let mut tex_bytes = vec![0u8; (TEX_W as usize) * (TEX_H as usize) * 4];
    for y in 0..TEX_H {
        for x in 0..TEX_W {
            let cell_x = x / CELL;
            let cell_y = y / CELL;
            let color = if (cell_x + cell_y) % 2 == 0 { red } else { yellow };
            let off = ((y as usize) * (TEX_W as usize) + (x as usize)) * 4;
            tex_bytes[off..off + 4].copy_from_slice(&color.to_u32().to_le_bytes());
        }
    }

    let tex = device.upload_texture(&TextureSpec {
        format: TextureFormat::Rgba8888,
        width: TEX_W,
        height: TEX_H,
        stride: (TEX_W as u32) * 4,
        data: &tex_bytes,
    })?;

    let dst_x = (info.width / 2).saturating_sub(TEX_W / 2);
    let dst_y = (info.height / 2).saturating_sub(TEX_H / 2);

    device
        .begin_frame()
        .fill_rect_unclipped(
            Rect::new(0, 0, info.width, info.height),
            Rgba::BLACK,
            BlendMode::Opaque,
        )?
        .copy_rect(
            &tex,
            Rect::new(0, 0, TEX_W, TEX_H),
            Rect::new(dst_x, dst_y, TEX_W, TEX_H),
            CopyOpts::default(),
        )?
        .present()?
        .submit()?
        .wait_presented(Duration::from_millis(10_000))?;

    println!(
        "M2c3.1: check HDMI — {TEX_W}×{TEX_H} red/yellow checkerboard centred"
    );
    Ok(())
}

fn a8_test(base: u32) -> Result<(), DeviceError> {
    let mut device = open_ready(base)?;
    let info = device.video_info();
    println!("Video: {}×{}", info.width, info.height);

    const TEX_W: u16 = 64;
    const TEX_H: u16 = 64;
    let mut tex_bytes = vec![0u8; (TEX_W as usize) * (TEX_H as usize)];
    for y in 0..TEX_H {
        for x in 0..TEX_W {
            tex_bytes[(y as usize) * (TEX_W as usize) + (x as usize)] = (x as u8).saturating_mul(4);
        }
    }
    let tex = device.upload_texture(&TextureSpec {
        format: TextureFormat::A8,
        width: TEX_W,
        height: TEX_H,
        stride: TEX_W as u32,
        data: &tex_bytes,
    })?;

    let dst_x = (info.width / 2).saturating_sub(TEX_W / 2);
    let dst_y = (info.height / 2).saturating_sub(TEX_H / 2);
    let red = Rgba::new(0xFF, 0x00, 0x00, 0xFF);

    device
        .begin_frame()
        .fill_rect_unclipped(
            Rect::new(0, 0, info.width, info.height),
            Rgba::BLACK,
            BlendMode::Opaque,
        )?
        .copy_rect(
            &tex,
            Rect::new(0, 0, TEX_W, TEX_H),
            Rect::new(dst_x, dst_y, TEX_W, TEX_H),
            CopyOpts {
                blend: BlendMode::Opaque,
                tint: Some(red),
                ..CopyOpts::default()
            },
        )?
        .present()?
        .submit()?
        .wait_presented(Duration::from_millis(10_000))?;

    println!(
        "M2c3.2: check HDMI — {TEX_W}×{TEX_H} horizontal gradient (black→red) on black"
    );
    Ok(())
}

fn blend_test(base: u32) -> Result<(), DeviceError> {
    let mut device = open_ready(base)?;
    let info = device.video_info();
    println!("Video: {}×{}", info.width, info.height);

    const TEX_W: u16 = 64;
    const TEX_H: u16 = 64;
    let mut tex_bytes = vec![0u8; (TEX_W as usize) * (TEX_H as usize)];
    for y in 0..TEX_H {
        for x in 0..TEX_W {
            tex_bytes[(y as usize) * (TEX_W as usize) + (x as usize)] = (x as u8).saturating_mul(4);
        }
    }
    let tex = device.upload_texture(&TextureSpec {
        format: TextureFormat::A8,
        width: TEX_W,
        height: TEX_H,
        stride: TEX_W as u32,
        data: &tex_bytes,
    })?;

    let dst_x = (info.width / 2).saturating_sub(TEX_W / 2);
    let dst_y = (info.height / 2).saturating_sub(TEX_H / 2);
    let blue = Rgba::new(0x00, 0x00, 0xFF, 0xFF);
    let red = Rgba::new(0xFF, 0x00, 0x00, 0xFF);

    device
        .begin_frame()
        .fill_rect_unclipped(
            Rect::new(0, 0, info.width, info.height),
            blue,
            BlendMode::Opaque,
        )?
        .copy_rect(
            &tex,
            Rect::new(0, 0, TEX_W, TEX_H),
            Rect::new(dst_x, dst_y, TEX_W, TEX_H),
            CopyOpts {
                blend: BlendMode::SrcAlpha,
                tint: Some(red),
                ..CopyOpts::default()
            },
        )?
        .present()?
        .submit()?
        .wait_presented(Duration::from_millis(10_000))?;

    println!(
        "M2c3.3: check HDMI — {TEX_W}×{TEX_H} smooth fade blue→red over blue"
    );
    Ok(())
}

/// Bundled Latin Noto Sans, SIL OFL — ~27 KB.
const NOTO_SANS: &[u8] = include_bytes!("../../fonts/NotoSans-Regular.ttf");

fn build_text_atlas() -> Result<menu_core::text::FontAtlas, Box<dyn std::error::Error>> {
    let charset: String = (b' '..=b'~').map(|b| b as char).collect();
    let atlas = menu_core::text::build_atlas(NOTO_SANS, 48.0, &charset, 512, 512)?;
    Ok(atlas)
}

fn text_test(base: u32) -> Result<(), Box<dyn std::error::Error>> {
    let mut device = open_ready(base)?;
    let info = device.video_info();
    println!("Video: {}×{}", info.width, info.height);

    let atlas = build_text_atlas()?;
    println!(
        "Atlas: {}×{}, line_height={}, ascent={}",
        atlas.width, atlas.height, atlas.line_height, atlas.ascent
    );
    let tex = device.upload_texture(&TextureSpec {
        format: TextureFormat::A8,
        width: atlas.width,
        height: atlas.height,
        stride: atlas.width as u32,
        data: &atlas.bytes,
    })?;

    let text = "Hello, 1FPGA!";
    let dark_blue = Rgba::new(0x10, 0x10, 0x40, 0xFF);
    let white = Rgba::new(0xFF, 0xFF, 0xFF, 0xFF);
    let text_w = atlas.measure(text) as u16;
    let pen_x_start = info.width.saturating_sub(text_w) / 2;
    let pen_y_top = (info.height / 2).saturating_sub(atlas.line_height / 2);

    let mut frame = device.begin_frame().fill_rect_unclipped(
        Rect::new(0, 0, info.width, info.height),
        dark_blue,
        BlendMode::Opaque,
    )?;
    let mut pen_x = pen_x_start as i32;
    let mut glyphs = 0u32;
    for ch in text.chars() {
        let g = match atlas.glyph(ch) {
            Some(g) => *g,
            None => continue,
        };
        if g.width > 0 && g.height > 0 {
            let dst_x = (pen_x + g.bearing_x as i32).max(0) as u16;
            let dst_y_off = (atlas.ascent as i32) - (g.ymin as i32) - (g.height as i32);
            let dst_y = (pen_y_top as i32 + dst_y_off).max(0) as u16;
            frame = frame.copy_rect(
                &tex,
                Rect::new(g.atlas_x, g.atlas_y, g.width, g.height),
                Rect::new(dst_x, dst_y, g.width, g.height),
                CopyOpts {
                    blend: BlendMode::SrcAlpha,
                    tint: Some(white),
                    ..CopyOpts::default()
                },
            )?;
            glyphs += 1;
        }
        pen_x += g.advance as i32;
    }
    println!("Submitted {glyphs} glyphs");
    frame.present()?
        .submit()?
        .wait_presented(Duration::from_millis(15_000))?;

    println!("M2c3.4: check HDMI — \"{text}\" in white on dark blue");
    Ok(())
}

/// Cubic ease-in-out: t in [0,1] -> [0,1] with smooth start/end.
fn ease_in_out_cubic(t: f32) -> f32 {
    if t < 0.5 {
        4.0 * t * t * t
    } else {
        let f = -2.0 * t + 2.0;
        1.0 - f * f * f / 2.0
    }
}

fn text_anim(base: u32) -> Result<(), Box<dyn std::error::Error>> {
    let mut device = open_ready(base)?;
    let info = device.video_info();
    println!("Video: {}×{}", info.width, info.height);

    let atlas = build_text_atlas()?;
    println!(
        "Atlas: {}×{}, line_height={}, ascent={}",
        atlas.width, atlas.height, atlas.line_height, atlas.ascent
    );
    let tex = device.upload_texture(&TextureSpec {
        format: TextureFormat::A8,
        width: atlas.width,
        height: atlas.height,
        stride: atlas.width as u32,
        data: &atlas.bytes,
    })?;

    let text = "Hello, 1FPGA!";
    let dark_blue = Rgba::new(0x10, 0x10, 0x40, 0xFF);
    let white = Rgba::new(0xFF, 0xFF, 0xFF, 0xFF);
    let text_w = atlas.measure(text) as i32;
    let pen_y_top = (info.height / 2).saturating_sub(atlas.line_height / 2);

    let margin = 32_i32;
    let pen_x_min = margin;
    let pen_x_max = (info.width as i32) - text_w - margin;
    let travel = (pen_x_max - pen_x_min).max(0) as f32;

    // Dirty-rect strip — only the line band changes per frame after warm-up.
    let strip_y = pen_y_top;
    let strip_h = atlas.line_height;

    let anim_duration = Duration::from_secs(5);
    let half_period = Duration::from_millis(1500);

    let frame_count_at_start = device.frame_count();
    let start = Instant::now();
    let mut frame_idx: u32 = 0;

    while start.elapsed() < anim_duration {
        let elapsed = start.elapsed().as_secs_f32();
        let phase = (elapsed / half_period.as_secs_f32()) % 2.0;
        let normalised = if phase < 1.0 { phase } else { 2.0 - phase };
        let eased = ease_in_out_cubic(normalised);
        let pen_x = pen_x_min + (eased * travel) as i32;

        let clear_rect = if frame_idx < 3 {
            Rect::new(0, 0, info.width, info.height)
        } else {
            Rect::new(0, strip_y, info.width, strip_h)
        };
        let mut frame = device.begin_frame().fill_rect_unclipped(
            clear_rect,
            dark_blue,
            BlendMode::Opaque,
        )?;

        let mut x = pen_x;
        for ch in text.chars() {
            let g = match atlas.glyph(ch) {
                Some(g) => *g,
                None => continue,
            };
            if g.width > 0 && g.height > 0 {
                let dst_x = (x + g.bearing_x as i32).max(0) as u16;
                let dst_y_off = (atlas.ascent as i32) - (g.ymin as i32) - (g.height as i32);
                let dst_y = (pen_y_top as i32 + dst_y_off).max(0) as u16;
                frame = frame.copy_rect(
                    &tex,
                    Rect::new(g.atlas_x, g.atlas_y, g.width, g.height),
                    Rect::new(dst_x, dst_y, g.width, g.height),
                    CopyOpts {
                        blend: BlendMode::SrcAlpha,
                        tint: Some(white),
                        ..CopyOpts::default()
                    },
                )?;
            }
            x += g.advance as i32;
        }

        frame.present()?
            .submit()?
            .wait(Duration::from_millis(500))?;
        frame_idx += 1;
    }

    let total = start.elapsed().as_secs_f32();
    let displayed = device.frame_count() - frame_count_at_start;
    println!(
        "Submitted {frame_idx} frames in {total:.2} s — submit FPS: {:.1}",
        frame_idx as f32 / total
    );
    println!(
        "Displayed {displayed} frames — display FPS: {:.1} (capped by HDMI vsync)",
        displayed as f32 / total
    );
    Ok(())
}

fn clip_test(base: u32) -> Result<(), DeviceError> {
    let mut device = open_ready(base)?;
    let info = device.video_info();
    println!("Video: {}×{}", info.width, info.height);

    let green = Rgba::new(0x00, 0xC0, 0x00, 0xFF);
    let red = Rgba::new(0xFF, 0x00, 0x00, 0xFF);
    let blue = Rgba::new(0x20, 0x40, 0xFF, 0xFF);

    let clip_w = info.width / 3;
    let clip_h = info.height / 3;
    let clip_x = (info.width - clip_w) / 2;
    let clip_y = (info.height - clip_h) / 2;

    device
        .begin_frame()
        .fill_rect_unclipped(
            Rect::new(0, 0, info.width, info.height),
            green,
            BlendMode::Opaque,
        )?
        .set_clip(Rect::new(clip_x, clip_y, clip_w, clip_h))?
        .fill_rect(
            Rect::new(0, 0, info.width, info.height),
            red,
            BlendMode::Opaque,
        )?
        .fill_rect_unclipped(Rect::new(20, 20, 80, 80), blue, BlendMode::Opaque)?
        .clear_clip()?
        .fill_rect(
            Rect::new(info.width.saturating_sub(100), 20, 80, 80),
            blue,
            BlendMode::Opaque,
        )?
        .present()?
        .submit()?
        .wait_presented(Duration::from_millis(5000))?;

    println!(
        "M2c2: check HDMI — green bg, red {clip_w}×{clip_h} centre, blue 80×80 \
         top-left (ignore_clip) and top-right (after CLEAR_CLIP)"
    );
    Ok(())
}

fn rtt_test(base: u32) -> Result<(), DeviceError> {
    let mut device = open_ready(base)?;
    let info = device.video_info();
    println!("Video: {}×{}", info.width, info.height);

    // Allocate a 200×200 render target.
    let rt = device.create_render_target(200, 200)?;
    println!("RT allocated: tex_id={}, phys={:#010X}", rt.id, rt.phys_addr);

    // Paint four quadrants into the RT, then blit it onto the FB.
    let q0 = Rgba::new(0xFF, 0x40, 0x40, 0xFF); // red TL
    let q1 = Rgba::new(0x40, 0xFF, 0x40, 0xFF); // green TR
    let q2 = Rgba::new(0x40, 0x80, 0xFF, 0xFF); // blue BL
    let q3 = Rgba::new(0xFF, 0xC0, 0x40, 0xFF); // amber BR

    let bg = Rgba::new(0x10, 0x10, 0x18, 0xFF);
    let dst_x = info.width.saturating_sub(200) / 2;
    let dst_y = info.height.saturating_sub(200) / 2;

    device
        .begin_frame()
        // Render-into-texture phase.
        .set_target(&rt)?
        .fill_rect_unclipped(Rect::new(0,   0,   100, 100), q0, BlendMode::Opaque)?
        .fill_rect_unclipped(Rect::new(100, 0,   100, 100), q1, BlendMode::Opaque)?
        .fill_rect_unclipped(Rect::new(0,   100, 100, 100), q2, BlendMode::Opaque)?
        .fill_rect_unclipped(Rect::new(100, 100, 100, 100), q3, BlendMode::Opaque)?
        // Switch back to the framebuffer and use the RT as a source.
        .set_target_framebuffer()?
        .fill_rect_unclipped(
            Rect::new(0, 0, info.width, info.height),
            bg,
            BlendMode::Opaque,
        )?
        .copy_rect(
            &rt,
            Rect::new(0, 0, 200, 200),
            Rect::new(dst_x, dst_y, 200, 200),
            CopyOpts::default(),
        )?
        .present()?
        .submit()?
        .wait_presented(Duration::from_millis(5000))?;

    println!(
        "rtt-test: check HDMI — 200×200 four-quadrant tile (red TL, green TR, \
         blue BL, amber BR) centred on dark navy"
    );
    Ok(())
}

/// Phase 2a step 2 verification.
///
/// Confirms three things end-to-end:
///   1. `Device::set_layer` causes a write into the layer-table region
///      and `Device::commit_layers` makes it visible to the FPGA via
///      LAYER_COMMIT (single 32-bit atomic).
///   2. The on-FPGA `layer_dma` actually fires once per vsync and
///      reads `count` × 32 bytes from DDR3 — observed by LAYER_DEBUG
///      advancing at frame_rate × count.
///   3. Setting LAYER_COMMIT.count = 0 halts the DMA (the IDLE-state
///      guard `count_i != 0` works).
///
/// No visual change: the renderer is not wired to the cache yet
/// (that's step 3), so the HDMI output stays on the colour-bar test
/// pattern regardless of what we commit.
fn layer_probe(base: u32) -> Result<(), DeviceError> {
    let mut device = Device::open_with(DeviceConfig {
        base_phys_addr: base,
        ..DeviceConfig::default()
    })?;

    let initial = device.layer_dma_descriptors();
    println!("LAYER_DEBUG (initial): {initial}");

    // Step 1: write one solid-blue layer and commit.
    let layer = protocol::LayerDescriptor::solid(
        0xFF_FF_00_00, // BGRA bytes [B=00, G=00, R=FF, A=FF] = red
        100, 100, 200, 200,
    );
    device.set_layer(0, &layer)?;
    let active_after = device.layer_back_idx() ^ 1; // about-to-be-active
    device.commit_layers();
    println!(
        "Committed: 1 solid layer at (100,100, 200×200), active table = {}",
        if active_after == 0 { "A" } else { "B" }
    );

    // Step 2: sample DMA tick rate over ~1 second. Expected:
    //   delta ≈ count × frame_rate
    // For 1280×720@30 (the compositor's nominal timing),
    // count=1 → ~30 ticks per second.
    let s0 = device.layer_dma_descriptors();
    let t0 = Instant::now();
    std::thread::sleep(Duration::from_secs(1));
    let s1 = device.layer_dma_descriptors();
    let elapsed = t0.elapsed();
    let delta_running = s1.wrapping_sub(s0);
    let rate = (delta_running as f32) / elapsed.as_secs_f32();
    println!(
        "running: LAYER_DEBUG {s0} → {s1} (Δ={delta_running} over {:.3}s ⇒ ~{:.1} desc/s)",
        elapsed.as_secs_f32(),
        rate
    );
    if delta_running == 0 {
        println!(
            "FAIL: DMA didn't tick. Check that vsync is reaching layer_dma \
             (LAYER_TABLE_BASE programmed? LAYER_COMMIT count > 0?)."
        );
        return Ok(());
    }

    // Step 3: re-commit with count=0 (no intervening set_layer means
    // back_valid_count is still 0 after the previous commit reset it).
    device.commit_layers();
    println!("Committed: 0 layers (count=0). DMA should halt.");

    let s2 = device.layer_dma_descriptors();
    let t1 = Instant::now();
    std::thread::sleep(Duration::from_secs(1));
    let s3 = device.layer_dma_descriptors();
    let halted_delta = s3.wrapping_sub(s2);
    println!(
        "halted: LAYER_DEBUG {s2} → {s3} (Δ={halted_delta} over {:.3}s; expected 0)",
        t1.elapsed().as_secs_f32()
    );
    if halted_delta != 0 {
        println!(
            "FAIL: DMA still ticking with count=0 — the count-gating guard \
             in layer_dma's S_IDLE → S_REQ transition isn't holding."
        );
        return Ok(());
    }

    println!("PASS: layer plumbing + DMA + LAYER_COMMIT round-trip verified.");
    Ok(())
}

/// Phase 2a visual demo: 5-layer scene + compositor FPS probe.
///
/// Layer stack (slot 0 = back, native-1080p coordinates):
///   0  navy    1920×1080 background
///   1  header  1920×120  bar (top)
///   2  cyan     420×280  panel at (160, 320)
///   3  magenta  420×280  panel at (360, 400) — overlaps cyan
///   4  yellow   420×280  panel at (560, 480) — overlaps magenta
///
/// Static (committed once); the loop just samples LAYER_DEBUG every
/// second to compute the compositor's frame rate. Each frame the FPGA
/// reads `count` (= 5) descriptors, so descriptors-per-second / count
/// is the actual frame rate. Expected ≈ 41 fps at the compositor's
/// native-1080p timing (100 MHz / (2200 × 1100)).
fn layer_draw(base: u32) -> Result<(), DeviceError> {
    let mut device = Device::open_with(DeviceConfig {
        base_phys_addr: base,
        ..DeviceConfig::default()
    })?;

    let info = device.video_info();
    println!(
        "Framework HDMI mode (VIDEO_INFO): {}x{}",
        info.width, info.height
    );
    println!(
        "Compositor native:                 1920x1080 @ ~27 Hz (100 MHz pixel clock, Phase 2c step 3)"
    );
    if info.width != 1920 || info.height != 1080 {
        println!(
            "NOTE: ASCAL is feeding HDMI at {}x{}, not 1080p. Our 1920x1080 \n\
             output will be either downscaled or clipped depending on the \n\
             MiSTer.ini `vscale_mode`. To see the demo fill the screen, set \n\
             `video_mode=` to a 1080p mode in /media/fat/MiSTer.ini.",
            info.width, info.height
        );
    }

    // BGRA byte order (PROTOCOL.md §11): u32 LE bytes are [B, G, R, A].
    let navy    = 0xFF_05_10_40u32; // dark navy: B=40 G=10 R=05
    let header  = 0xFF_10_30_80u32; // muted steel-blue header
    let cyan    = 0xFF_00_FF_FFu32; // B=FF G=FF R=00
    let magenta = 0xFF_FF_00_FFu32; // B=FF G=00 R=FF
    let yellow  = 0xFF_FF_FF_00u32; // B=00 G=FF R=FF

    const COUNT: u32 = 5;

    device.set_layer(0, &protocol::LayerDescriptor::solid(navy,      0,   0, 1920, 1080))?;
    device.set_layer(1, &protocol::LayerDescriptor::solid(header,    0,   0, 1920,  120))?;
    device.set_layer(2, &protocol::LayerDescriptor::solid(cyan,     160, 320,  420,  280))?;
    device.set_layer(3, &protocol::LayerDescriptor::solid(magenta,  360, 400,  420,  280))?;
    device.set_layer(4, &protocol::LayerDescriptor::solid(yellow,   560, 480,  420,  280))?;
    device.commit_layers();

    println!("Committed 5-layer scene (navy bg + header + cyan/magenta/yellow panels).");
    println!("Compositor FPS sampled every second. Ctrl-C to exit (max 60 s).");

    let running = Arc::new(AtomicBool::new(true));
    {
        let r = running.clone();
        ctrlc::set_handler(move || r.store(false, Ordering::SeqCst))
            .map_err(|e| DeviceError::Io(std::io::Error::other(e.to_string())))?;
    }

    let start = Instant::now();
    let mut last_t = start;
    let mut last_d = device.layer_dma_descriptors();
    let max_dur = Duration::from_secs(60);

    while running.load(Ordering::SeqCst) && start.elapsed() < max_dur {
        std::thread::sleep(Duration::from_millis(1000));
        let now = Instant::now();
        let d = device.layer_dma_descriptors();
        let elapsed = (now - last_t).as_secs_f32();
        let desc_delta = d.wrapping_sub(last_d) as f32;
        let frames = desc_delta / (COUNT as f32);
        let fps = frames / elapsed;
        println!(
            "compositor: {fps:5.1} fps  ({frames:>5.0} frames / {elapsed:.3}s, \
             LAYER_DEBUG Δ={desc_delta:.0})"
        );
        last_t = now;
        last_d = d;
    }

    // Clean exit: black screen.
    device.commit_layers();
    println!("Cleared layers (count=0). Screen returns to black.");
    Ok(())
}

/// Phase 2b step 2 verification.
///
/// Builds a 4-layer scene with one textured layer in the mix. Step 2
/// wires up the texture_unit + line_buffer pipeline, so the textured
/// slot now renders ACTUAL pixels from the uploaded texture (a 256×256
/// red/yellow checkerboard) instead of the step-1 debug magenta.
///
/// Sizing notes:
///   - Texture is 256×256 BGRA so the textured layer's src rect
///     (= dst rect since `textured()` defaults src=dst) stays in
///     bounds.
///   - dst_x and dst_y are even — line_buffer alignment requirement.
fn layer_tex_probe(base: u32) -> Result<(), DeviceError> {
    let mut device = Device::open_with(DeviceConfig {
        base_phys_addr: base,
        ..DeviceConfig::default()
    })?;

    let info = device.video_info();
    println!(
        "Framework HDMI mode (VIDEO_INFO): {}x{}",
        info.width, info.height
    );

    // Upload a 256×256 BGRA texture: a red disc with a smooth alpha
    // fall-off from the centre. Demonstrates SrcAlpha blending against
    // the navy background — opaque red in the middle, fading into the
    // background colour at the edges.
    const TEX_W: u16 = 256;
    const TEX_H: u16 = 256;
    let cx: f32 = (TEX_W as f32) / 2.0;
    let cy: f32 = (TEX_H as f32) / 2.0;
    let r_max: f32 = (TEX_W as f32) / 2.0;
    let mut tex_bytes = vec![0u8; (TEX_W as usize) * (TEX_H as usize) * 4];
    for y in 0..TEX_H {
        for x in 0..TEX_W {
            let dx = (x as f32) - cx;
            let dy = (y as f32) - cy;
            let dist = (dx * dx + dy * dy).sqrt();
            let t = (1.0 - (dist / r_max)).clamp(0.0, 1.0);
            // Smoothstep-ish curve so the falloff isn't perfectly
            // linear. alpha = t^2 (sharper centre, softer edge).
            let alpha = (t * t * 255.0) as u8;
            let color = protocol::Rgba::new(0xFF, 0x40, 0x40, alpha);
            let off = ((y as usize) * (TEX_W as usize) + (x as usize)) * 4;
            tex_bytes[off..off + 4].copy_from_slice(&color.to_u32().to_le_bytes());
        }
    }
    let tex = device.upload_texture(&TextureSpec {
        format: TextureFormat::Rgba8888,
        width: TEX_W,
        height: TEX_H,
        stride: (TEX_W as u32) * 4,
        data: &tex_bytes,
    })?;
    println!(
        "Uploaded 256×256 alpha-disc texture: id={}, base={:#010X}",
        tex.id, tex.phys_addr
    );

    let navy   = 0xFF_05_10_40u32;
    let header = 0xFF_10_30_80u32;
    let cyan   = 0xFF_00_FF_FFu32;

    // Slot 2 is the textured layer — 256×256 panel slightly right of
    // centre (even dst_x for line-buffer alignment).
    device.set_layer(0, &protocol::LayerDescriptor::solid(navy,    0,   0, 1920, 1080))?;
    device.set_layer(1, &protocol::LayerDescriptor::solid(header,  0,   0, 1920,  120))?;
    device.set_layer(2, &protocol::LayerDescriptor::textured(tex.id, 832, 412, 256, 256))?;
    device.set_layer(3, &protocol::LayerDescriptor::solid(cyan,   200, 700,  500, 300))?;
    device.commit_layers();
    const COUNT: u32 = 4;

    println!(
        "Committed 4 layers (slot 2 is textured, tex_id={}). \n\
         Expect: navy bg, blue header, soft red disc fading into the navy \n\
         around the centre (SrcAlpha blend), cyan panel bottom-left. \n\
         Ctrl-C to exit (max 60 s).",
        tex.id
    );

    let running = Arc::new(AtomicBool::new(true));
    {
        let r = running.clone();
        ctrlc::set_handler(move || r.store(false, Ordering::SeqCst))
            .map_err(|e| DeviceError::Io(std::io::Error::other(e.to_string())))?;
    }

    let start = Instant::now();
    let mut last_t = start;
    let mut last_d = device.layer_dma_descriptors();
    let max_dur = Duration::from_secs(60);

    while running.load(Ordering::SeqCst) && start.elapsed() < max_dur {
        std::thread::sleep(Duration::from_millis(1000));
        let now = Instant::now();
        let d = device.layer_dma_descriptors();
        let elapsed = (now - last_t).as_secs_f32();
        let frames = (d.wrapping_sub(last_d) as f32) / (COUNT as f32);
        let fps = frames / elapsed;
        println!("compositor: {fps:5.1} fps ({frames:>5.0} frames / {elapsed:.3}s)");
        last_t = now;
        last_d = d;
    }

    device.commit_layers();
    println!("Cleared layers (count=0). Screen returns to black.");
    Ok(())
}

/// Phase 2c step 2 verification — A8 textures + tint.
///
/// Uploads a 256×256 A8 alpha-disc texture (same shape as
/// layer-tex-probe's BGRA disc but only the alpha channel — one byte
/// per pixel). The textured layer's `color` field is the tint
/// (white). The on-FPGA texture_unit pre-bakes each pixel as
/// `(tint.rgb, alpha)` into the line buffer; the painter
/// SrcAlpha-blends that over the topmost solid (the navy background).
/// PASS = a soft white disc fading smoothly into navy, no fringing.
fn layer_a8_probe(base: u32) -> Result<(), DeviceError> {
    let mut device = Device::open_with(DeviceConfig {
        base_phys_addr: base,
        ..DeviceConfig::default()
    })?;

    let info = device.video_info();
    println!(
        "Framework HDMI mode (VIDEO_INFO): {}x{}",
        info.width, info.height
    );

    // Build a 256×256 A8 disc (alpha varies with radial distance).
    const TEX_W: u16 = 256;
    const TEX_H: u16 = 256;
    let cx: f32 = (TEX_W as f32) / 2.0;
    let cy: f32 = (TEX_H as f32) / 2.0;
    let r_max: f32 = (TEX_W as f32) / 2.0;
    let mut tex_bytes = vec![0u8; (TEX_W as usize) * (TEX_H as usize)];
    for y in 0..TEX_H {
        for x in 0..TEX_W {
            let dx = (x as f32) - cx;
            let dy = (y as f32) - cy;
            let dist = (dx * dx + dy * dy).sqrt();
            let t = (1.0 - (dist / r_max)).clamp(0.0, 1.0);
            tex_bytes[(y as usize) * (TEX_W as usize) + (x as usize)] =
                (t * t * 255.0) as u8;
        }
    }
    let tex = device.upload_texture(&TextureSpec {
        format: TextureFormat::A8,
        width: TEX_W,
        height: TEX_H,
        stride: TEX_W as u32, // 1 byte/pixel
        data: &tex_bytes,
    })?;
    println!(
        "Uploaded 256×256 A8 alpha-disc texture: id={}, base={:#010X}",
        tex.id, tex.phys_addr
    );

    let navy   = 0xFF_05_10_40u32;
    let header = 0xFF_10_30_80u32;
    // Tint colour for the A8 layer (white = R=FF G=FF B=FF). The
    // top byte (alpha) is overwritten by the per-pixel A8 sample
    // inside texture_unit.
    let tint   = 0xFF_FF_FF_FFu32;

    device.set_layer(0, &protocol::LayerDescriptor::solid(navy,   0,   0, 1920, 1080))?;
    device.set_layer(1, &protocol::LayerDescriptor::solid(header, 0,   0, 1920,  120))?;
    let a8_layer = protocol::LayerDescriptor {
        flags:    protocol::layer::flag::ENABLED
                | protocol::layer::flag::TINT_FROM_A8,
        tex_id:   tex.id,
        dst_x:    832, dst_y: 412,
        dst_w:    256, dst_h: 256,
        src_x:    0,   src_y: 0,
        src_w:    256, src_h: 256,
        color:    tint,
        opacity:  0xFF,
        _reserved: [0; 7],
    };
    device.set_layer(2, &a8_layer)?;
    device.commit_layers();
    const COUNT: u32 = 3;

    println!(
        "Committed 3 layers (slot 2 is A8-tinted, tex_id={}, tint=#FFFFFFFF). \n\
         Expect: navy background, blue header, soft WHITE disc fading \n\
         into the navy. Ctrl-C to exit (max 60 s).",
        tex.id
    );

    let running = Arc::new(AtomicBool::new(true));
    {
        let r = running.clone();
        ctrlc::set_handler(move || r.store(false, Ordering::SeqCst))
            .map_err(|e| DeviceError::Io(std::io::Error::other(e.to_string())))?;
    }

    let start = Instant::now();
    let mut last_t = start;
    let mut last_d = device.layer_dma_descriptors();
    let max_dur = Duration::from_secs(60);

    while running.load(Ordering::SeqCst) && start.elapsed() < max_dur {
        std::thread::sleep(Duration::from_millis(1000));
        let now = Instant::now();
        let d = device.layer_dma_descriptors();
        let elapsed = (now - last_t).as_secs_f32();
        let frames = (d.wrapping_sub(last_d) as f32) / (COUNT as f32);
        let fps = frames / elapsed;
        println!("compositor: {fps:5.1} fps ({frames:>5.0} frames / {elapsed:.3}s)");
        last_t = now;
        last_d = d;
    }

    device.commit_layers();
    println!("Cleared layers (count=0). Screen returns to black.");
    Ok(())
}

/// Phase 2c step 3 verification — 4-buffer back-to-front blend.
///
/// Scene: 4 textured layers in z-order — one isolated and three
/// overlapping in a chain. Each stage of the painter's 4-stage
/// pipeline composites one buffer onto the running accumulator.
///
/// Layers:
///   2: BGRA red disc, isolated  (buffer 0)
///   3: BGRA red disc, overlapping with 4   (buffer 1)
///   4: A8 white disc, overlapping with 3 + 5 (buffer 2)
///   5: A8 white disc, overlapping with 4   (buffer 3)
///
/// PASS = leftmost red disc on its own, then a 3-disc chain
/// where each disc smoothly blends into the next via SrcAlpha
/// (red → reddish-white → whitish → white).
///
/// FAIL modes:
///  - All black: pipeline broken (sync misalignment or reset stuck).
///  - Topmost disc fully covers underlying ones: back-to-front not
///    running; some `contributes` bit stuck low.
///  - Discs in wrong colours or wrong positions: line buffer
///    routing or LSB select inverted.
fn layer_multitex_probe(base: u32) -> Result<(), DeviceError> {
    let mut device = Device::open_with(DeviceConfig {
        base_phys_addr: base,
        ..DeviceConfig::default()
    })?;

    let info = device.video_info();
    println!(
        "Framework HDMI mode (VIDEO_INFO): {}x{}",
        info.width, info.height
    );

    // BGRA 200×200 red disc.
    const BGRA_W: u16 = 200;
    const BGRA_H: u16 = 200;
    let cx = (BGRA_W as f32) / 2.0;
    let cy = (BGRA_H as f32) / 2.0;
    let r_max = (BGRA_W as f32) / 2.0;
    let mut bgra_bytes = vec![0u8; (BGRA_W as usize) * (BGRA_H as usize) * 4];
    for y in 0..BGRA_H {
        for x in 0..BGRA_W {
            let dx = (x as f32) - cx;
            let dy = (y as f32) - cy;
            let dist = (dx * dx + dy * dy).sqrt();
            let t = (1.0 - (dist / r_max)).clamp(0.0, 1.0);
            let alpha = (t * t * 255.0) as u8;
            let color = protocol::Rgba::new(0xFF, 0x40, 0x40, alpha);
            let off = ((y as usize) * (BGRA_W as usize) + (x as usize)) * 4;
            bgra_bytes[off..off + 4].copy_from_slice(&color.to_u32().to_le_bytes());
        }
    }
    let tex_bgra = device.upload_texture(&TextureSpec {
        format: TextureFormat::Rgba8888,
        width: BGRA_W,
        height: BGRA_H,
        stride: (BGRA_W as u32) * 4,
        data: &bgra_bytes,
    })?;

    // A8 200×200 alpha disc (smaller than the BGRA disc so the
    // overlay is visibly inside the middle red one).
    const A8_W: u16 = 200;
    const A8_H: u16 = 200;
    let cx = (A8_W as f32) / 2.0;
    let cy = (A8_H as f32) / 2.0;
    let r_max = (A8_W as f32) / 2.0;
    let mut a8_bytes = vec![0u8; (A8_W as usize) * (A8_H as usize)];
    for y in 0..A8_H {
        for x in 0..A8_W {
            let dx = (x as f32) - cx;
            let dy = (y as f32) - cy;
            let dist = (dx * dx + dy * dy).sqrt();
            let t = (1.0 - (dist / r_max)).clamp(0.0, 1.0);
            a8_bytes[(y as usize) * (A8_W as usize) + (x as usize)] =
                (t * t * 255.0) as u8;
        }
    }
    let tex_a8 = device.upload_texture(&TextureSpec {
        format: TextureFormat::A8,
        width: A8_W,
        height: A8_H,
        stride: A8_W as u32,
        data: &a8_bytes,
    })?;
    println!(
        "Uploaded textures: BGRA disc id={} @{:#X}, A8 disc id={} @{:#X}",
        tex_bgra.id, tex_bgra.phys_addr, tex_a8.id, tex_a8.phys_addr
    );

    let navy   = 0xFF_05_10_40u32;
    let header = 0xFF_10_30_80u32;
    let white  = 0xFF_FF_FF_FFu32;

    // Layout:
    //   Layer 2: red disc at x=200  (isolated, just to verify buffer 0
    //            isolation; doesn't overlap with anything to its right)
    //   Layer 3: red disc at x=850  (start of the overlap chain)
    //   Layer 4: white disc at x=970 (overlaps with 3 AND 5)
    //   Layer 5: white disc at x=1100 (overlaps with 4)
    //
    // Discs are 200×200, so successive ones spaced ~120px apart give
    // ~80px overlap zones.
    let y_band: i16 = 440;
    let a8_at = |x: i16| protocol::LayerDescriptor {
        flags:    protocol::layer::flag::ENABLED
                | protocol::layer::flag::TINT_FROM_A8,
        tex_id:   tex_a8.id,
        dst_x:    x,    dst_y: y_band,
        dst_w:    A8_W, dst_h: A8_H,
        src_x:    0,    src_y: 0,
        src_w:    A8_W, src_h: A8_H,
        color:    white,
        opacity:  0xFF,
        _reserved: [0; 7],
    };

    device.set_layer(0, &protocol::LayerDescriptor::solid(navy,   0, 0, 1920, 1080))?;
    device.set_layer(1, &protocol::LayerDescriptor::solid(header, 0, 0, 1920,  120))?;
    device.set_layer(2, &protocol::LayerDescriptor::textured(tex_bgra.id, 200, y_band, BGRA_W, BGRA_H))?;
    device.set_layer(3, &protocol::LayerDescriptor::textured(tex_bgra.id, 850, y_band, BGRA_W, BGRA_H))?;
    device.set_layer(4, &a8_at(970))?;
    device.set_layer(5, &a8_at(1100))?;
    device.commit_layers();
    const COUNT: u32 = 6;

    println!(
        "Committed 6 layers: navy + header + 4 textured (2 BGRA red, 2 A8 white). \n\
         Layout: isolated red at x=200, then overlap chain of red→white→white \n\
         starting at x=850. Expect each overlap region to show SrcAlpha blending \n\
         (red shading through pink to white). \n\
         Ctrl-C to exit (max 60 s)."
    );

    let running = Arc::new(AtomicBool::new(true));
    {
        let r = running.clone();
        ctrlc::set_handler(move || r.store(false, Ordering::SeqCst))
            .map_err(|e| DeviceError::Io(std::io::Error::other(e.to_string())))?;
    }

    let start = Instant::now();
    let mut last_t = start;
    let mut last_d = device.layer_dma_descriptors();
    let max_dur = Duration::from_secs(60);

    while running.load(Ordering::SeqCst) && start.elapsed() < max_dur {
        std::thread::sleep(Duration::from_millis(1000));
        let now = Instant::now();
        let d = device.layer_dma_descriptors();
        let elapsed = (now - last_t).as_secs_f32();
        let frames = (d.wrapping_sub(last_d) as f32) / (COUNT as f32);
        let fps = frames / elapsed;
        println!("compositor: {fps:5.1} fps ({frames:>5.0} frames / {elapsed:.3}s)");
        last_t = now;
        last_d = d;
    }

    device.commit_layers();
    println!("Cleared layers (count=0). Screen returns to black.");
    Ok(())
}

fn print_layout(base: u32) {
    println!("Reserved DDR3 carve-out (256 MB):");
    println!("  base                {:#010X}", base);
    println!(
        "  end                 {:#010X}",
        base + mem::REGION_SIZE as u32
    );
    println!();
    println!("Regions:");
    for (name, off, size) in [
        ("framebuffer 0", mem::FB0_OFFSET, mem::FB_SLOT_SIZE),
        ("framebuffer 1", mem::FB1_OFFSET, mem::FB_SLOT_SIZE),
        ("framebuffer 2", mem::FB2_OFFSET, mem::FB_SLOT_SIZE),
        ("command ring", mem::RING_OFFSET, mem::RING_SIZE),
        (
            "tex descriptors",
            mem::TEX_TABLE_OFFSET,
            mem::TEX_TABLE_SIZE,
        ),
        ("tex data pool", mem::TEX_POOL_OFFSET, mem::TEX_POOL_SIZE),
    ] {
        println!(
            "  {:<18} {:#010X}  size {:>10}  ({})",
            name,
            base + off as u32,
            size,
            humanize(size)
        );
    }
    println!();
    println!(
        "Control register window (LW_H2F): {:#010X}, {} bytes",
        device::REGS_PHYS_ADDR,
        registers::REGISTER_WINDOW_SIZE
    );
    println!();
    println!(
        "Video mode: {}×{} BGRA8888 (v0 fixed)",
        protocol::FB_WIDTH,
        protocol::FB_HEIGHT
    );
}

fn humanize(bytes: usize) -> String {
    const KB: usize = 1024;
    const MB: usize = KB * 1024;
    if bytes >= MB && bytes.is_multiple_of(MB) {
        format!("{} MB", bytes / MB)
    } else if bytes >= KB && bytes.is_multiple_of(KB) {
        format!("{} KB", bytes / KB)
    } else {
        format!("{bytes} B")
    }
}

