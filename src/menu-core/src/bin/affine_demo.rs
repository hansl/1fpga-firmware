//! `affine_demo` — visual example for the `BLIT_AFFINE` opcode.
//!
//! Uploads one small RGBA icon and draws it every frame under an affine
//! transform: a large central icon that continuously rotates and gently
//! pulses in scale, surrounded by four satellites spinning at different
//! rates. Exercises `Frame::blit_affine_rotate` (rotation + scale, with
//! bilinear sampling) end-to-end. Loops until Ctrl+C.
//!
//! NOTE: this requires a menu-core bitstream that implements
//! `BLIT_AFFINE` (opcode 0x12). Against an older bitstream the first
//! affine command raises `ERR_BAD_OPCODE` and halts the fetcher.
//!
//! The icon is uploaded fully opaque; combined with `SrcAlpha` blend the
//! rotated-out corners (which the engine samples as transparent) fade to
//! the background, and the bilinear-sampled edges come out correctly
//! premultiplied — so the spinning square has clean antialiased edges
//! over the backdrop.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use clap::Parser;
use clap_verbosity_flag::Level as VerbosityLevel;
use clap_verbosity_flag::Verbosity;
use tracing::info;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::fmt::Subscriber;

use menu_core_host::device::{Device, DeviceConfig, FramebufferConfig};
use menu_core_host::error::DeviceError;
use menu_core_host::mem;
use menu_core_host::protocol::{BlendMode, Rect, Rgba, TextureFormat};
use menu_core_host::texture::TextureSpec;

// `unused_crate_dependencies` is workspace-wide; mirror menu_demo.
use cyclone_v as _;
use fontdue as _;
use menu_core as _;
use thiserror as _;
#[cfg(test)]
use pretty_assertions as _;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Flags {
    #[command(flatten)]
    verbose: Verbosity<clap_verbosity_flag::InfoLevel>,

    /// Override the reserved DDR3 base address (default: 0x30000000).
    #[clap(long, value_parser = parse_u32_hex_or_dec)]
    base_addr: Option<u32>,

    /// Source icon side length, in pixels (capped at 128 by the engine).
    #[clap(long, default_value_t = 64)]
    size: u16,

    /// Rotation speed of the central icon, in degrees/second.
    #[clap(long, default_value_t = 60.0)]
    speed: f64,

    /// Maximum duration before auto-exit. 0 = unbounded (Ctrl+C only).
    #[clap(long, default_value_t = 0)]
    max_seconds: u32,
}

fn parse_u32_hex_or_dec(s: &str) -> Result<u32, std::num::ParseIntError> {
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u32::from_str_radix(hex, 16)
    } else {
        s.parse::<u32>()
    }
}

/// Build a fully-opaque RGBA icon (in-memory BGRA order, §6.3): a color
/// gradient with a bright border and an upward-pointing triangle so the
/// orientation is unmistakable as it rotates.
fn make_icon(size: u16) -> Vec<u8> {
    let s = size as i32;
    let mut data = vec![0u8; (size as usize) * (size as usize) * 4];
    let border = (s / 12).max(2);
    for y in 0..s {
        for x in 0..s {
            // Base gradient.
            let mut r = (x * 255 / s) as u8;
            let mut g = (y * 255 / s) as u8;
            let mut b = 0xA0u8;
            // Upward triangle (apex at top-center) painted bright red.
            let half = (y * (s / 2)) / s;
            if x >= s / 2 - half && x <= s / 2 + half {
                r = 0xFF;
                g = 0x30;
                b = 0x30;
            }
            // Bright border ring.
            if x < border || x >= s - border || y < border || y >= s - border {
                r = 0xF0;
                g = 0xF0;
                b = 0xFF;
            }
            let idx = ((y * s + x) as usize) * 4;
            // In-memory order is B, G, R, A (PROTOCOL.md §6.3); opaque.
            data[idx] = b;
            data[idx + 1] = g;
            data[idx + 2] = r;
            data[idx + 3] = 0xFF;
        }
    }
    data
}

fn main() {
    if let Some(cores) = core_affinity::get_core_ids() {
        let core = cores.get(1).or_else(|| cores.first()).copied();
        if let Some(c) = core {
            core_affinity::set_for_current(c);
        }
    }

    let flags = Flags::parse();

    let level_filter = match flags.verbose.log_level() {
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

    if let Err(e) = run(flags) {
        tracing::error!("{e}");
        std::process::exit(1);
    }
}

fn run(flags: Flags) -> Result<(), DeviceError> {
    let size = flags.size.clamp(8, 128);
    let base = flags.base_addr.unwrap_or(mem::DEFAULT_BASE);
    let mut device = Device::open_with(DeviceConfig {
        base_phys_addr: base,
        ..DeviceConfig::default()
    })?;
    let info = device.video_info();
    let fb = FramebufferConfig::for_video(info, base);
    device.configure_framebuffer(fb)?;
    device.start()?;

    let icon = make_icon(size);
    let tex = device.upload_texture(&TextureSpec {
        format: TextureFormat::Rgba8888,
        width: size,
        height: size,
        stride: size as u32 * 4,
        data: &icon,
    })?;

    info!(
        "affine_demo: {}×{} fb, {}×{} icon, {:.0} deg/s",
        info.width, info.height, size, size, flags.speed
    );

    let bg = Rgba::new(0x0A, 0x0A, 0x12, 0xFF);
    let src = Rect::new(0, 0, size, size);
    let cx = info.width as i32 / 2;
    let cy = info.height as i32 / 2;
    // Satellites: offset from center, each spins at its own rate/phase.
    let orbit = (info.height as i32 / 3).min(info.width as i32 / 3);
    let satellites = [
        (-orbit, -orbit, 1.7_f64, 0.0_f64),
        (orbit, -orbit, -2.3, 90.0),
        (-orbit, orbit, -1.3, 180.0),
        (orbit, orbit, 2.9, 270.0),
    ];

    let running = Arc::new(AtomicBool::new(true));
    {
        let r = running.clone();
        ctrlc::set_handler(move || r.store(false, Ordering::SeqCst))
            .map_err(|e| DeviceError::Io(std::io::Error::other(e.to_string())))?;
    }

    let max_dur = if flags.max_seconds == 0 {
        Duration::MAX
    } else {
        Duration::from_secs(flags.max_seconds as u64)
    };

    let start = Instant::now();
    let mut frames: u32 = 0;
    let mut frames_since_report: u32 = 0;
    let mut last_report = start;

    while running.load(Ordering::SeqCst) {
        if start.elapsed() >= max_dur {
            break;
        }
        let t = start.elapsed().as_secs_f64();
        let angle = (t * flags.speed) % 360.0;
        // Gentle scale pulse for the central icon (2.0 .. 3.0).
        let scale = 2.5 + 0.5 * (t * 1.5).sin();

        let mut frame = device.begin_frame().fill_rect_unclipped(
            Rect::new(0, 0, info.width, info.height),
            bg,
            BlendMode::Opaque,
        )?;
        // Satellites first, then the central icon on top.
        for (dx, dy, rate, phase) in satellites {
            let a = (phase + t * flags.speed * rate).rem_euclid(360.0);
            frame = frame.blit_affine_rotate(
                &tex,
                src,
                a,
                1.2,
                cx + dx,
                cy + dy,
                BlendMode::SrcAlpha,
            )?;
        }
        frame = frame.blit_affine_rotate(&tex, src, angle, scale, cx, cy, BlendMode::SrcAlpha)?;

        frame
            .present()?
            .submit()?
            .wait(Duration::from_millis(500))?;

        frames += 1;
        frames_since_report += 1;
        if last_report.elapsed() >= Duration::from_secs(1) {
            let elapsed = last_report.elapsed().as_secs_f32();
            eprintln!(
                "fps: {:.1} (frame {})",
                frames_since_report as f32 / elapsed,
                frames
            );
            frames_since_report = 0;
            last_report = Instant::now();
        }
    }

    let total = start.elapsed().as_secs_f32();
    eprintln!(
        "affine_demo: {} frames in {:.2} s — average submit FPS: {:.1}",
        frames,
        total,
        frames as f32 / total
    );
    device.stop()?;
    Ok(())
}
