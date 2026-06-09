//! `menu_demo` — visual stress demo for the menu-core runtime.
//!
//! 64 small squares bounce around the framebuffer, each colored from a
//! sinusoidal palette so the frame is never solid. Per frame: one
//! full-FB clear + 64 `FILL_RECT`s + `PRESENT`. Loops until Ctrl+C; on
//! exit, prints aggregate FPS and stops the engine.
//!
//! Purpose: a longer-running consumer of the runtime API beyond the
//! one-shot test pattern. Catches issues that only show up across
//! sustained `begin_frame → submit → wait_presented` loops.

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
use menu_core_host::protocol::{BlendMode, Rect, Rgba};

// `unused_crate_dependencies` is workspace-wide; the binary only uses
// these crates transitively (or not at all on this binary).
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

    /// Number of bouncing particles.
    #[clap(long, default_value_t = 64)]
    particles: u16,

    /// Side length of each particle, in pixels.
    #[clap(long, default_value_t = 40)]
    size: u16,

    /// Maximum duration before auto-exit. Set to 0 for unbounded
    /// (only Ctrl+C terminates).
    #[clap(long, default_value_t = 0)]
    max_seconds: u32,

    /// Phase B blend test: upload a gradient wallpaper, enable the
    /// scanout compositor blend, and clear the content framebuffer to
    /// TRANSPARENT instead of opaque — the bouncing (opaque) squares
    /// then appear over the wallpaper wherever content alpha is zero.
    #[clap(long, default_value_t = false)]
    composite: bool,
}

fn parse_u32_hex_or_dec(s: &str) -> Result<u32, std::num::ParseIntError> {
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u32::from_str_radix(hex, 16)
    } else {
        s.parse::<u32>()
    }
}

#[derive(Clone, Copy)]
struct Particle {
    x: f32,
    y: f32,
    vx: f32,
    vy: f32,
    color: Rgba,
}

impl Particle {
    fn new(seed: u32, fb_w: f32, fb_h: f32, size: f32) -> Self {
        let mut state = seed.wrapping_mul(2654435761);
        let mut next = || {
            state = state.wrapping_mul(1664525).wrapping_add(1013904223);
            state
        };
        let x = (next() as f32 / u32::MAX as f32) * (fb_w - size);
        let y = (next() as f32 / u32::MAX as f32) * (fb_h - size);
        // Velocities in pixels/second; ±[100, 600].
        let vx = ((next() as f32 / u32::MAX as f32) * 1000.0) - 500.0;
        let vy = ((next() as f32 / u32::MAX as f32) * 1000.0) - 500.0;
        // Color from a 6-stop palette by hash of seed.
        let palette = [
            Rgba::new(0xFF, 0x40, 0x40, 0xFF),
            Rgba::new(0x40, 0xFF, 0x80, 0xFF),
            Rgba::new(0x40, 0x80, 0xFF, 0xFF),
            Rgba::new(0xFF, 0xC0, 0x40, 0xFF),
            Rgba::new(0xC0, 0x40, 0xFF, 0xFF),
            Rgba::new(0x40, 0xFF, 0xFF, 0xFF),
        ];
        let color = palette[(next() as usize) % palette.len()];
        Self { x, y, vx, vy, color }
    }

    fn step(&mut self, dt: f32, fb_w: f32, fb_h: f32, size: f32) {
        self.x += self.vx * dt;
        self.y += self.vy * dt;
        if self.x < 0.0 {
            self.x = 0.0;
            self.vx = -self.vx;
        } else if self.x + size > fb_w {
            self.x = fb_w - size;
            self.vx = -self.vx;
        }
        if self.y < 0.0 {
            self.y = 0.0;
            self.vy = -self.vy;
        } else if self.y + size > fb_h {
            self.y = fb_h - size;
            self.vy = -self.vy;
        }
    }
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
    let base = flags.base_addr.unwrap_or(mem::DEFAULT_BASE);
    let mut device = Device::open_with(DeviceConfig {
        base_phys_addr: base,
        ..DeviceConfig::default()
    })?;
    let info = device.video_info();
    let fb = FramebufferConfig::for_video(info, base);
    device.configure_framebuffer(fb)?;
    device.start()?;

    // Phase B: optionally upload a gradient wallpaper layer and enable the
    // scanout compositor blend. With compositing on, the content clear is
    // transparent so the wallpaper shows through behind the squares.
    if flags.composite {
        let w = info.width as usize;
        let h = info.height as usize;
        let stride = fb.stride as usize;
        let mut wp = vec![0u8; stride * h];
        for y in 0..h {
            let row = &mut wp[y * stride..y * stride + w * 4];
            for x in 0..w {
                let px = &mut row[x * 4..x * 4 + 4];
                // BGRA8888, little-endian: [B, G, R, A].
                px[0] = ((y * 255) / h.max(1)) as u8;   // B ramps top→bottom
                px[1] = 0x30;                            // G constant
                px[2] = ((x * 255) / w.max(1)) as u8;   // R ramps left→right
                px[3] = 0xFF;                            // opaque
            }
        }
        device.upload_wallpaper(&wp, fb.stride, info.height)?;
        device.set_composite(true);
        info!("menu_demo: compositing ON — gradient wallpaper behind transparent content");
    }

    info!(
        "menu_demo: {}×{} fb, {} particles, {} px squares",
        info.width, info.height, flags.particles, flags.size
    );

    let bg = if flags.composite {
        Rgba::new(0, 0, 0, 0) // transparent → wallpaper shows through
    } else {
        Rgba::new(0x10, 0x10, 0x18, 0xFF)
    };
    let fb_w = info.width as f32;
    let fb_h = info.height as f32;
    let size_f = flags.size as f32;

    let mut particles: Vec<Particle> = (0..flags.particles)
        .map(|i| Particle::new(0x517C_C001 + i as u32, fb_w, fb_h, size_f))
        .collect();

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

    let demo_start = Instant::now();
    let mut last_step = demo_start;
    let mut frames: u32 = 0;
    let mut frames_since_report: u32 = 0;
    let mut last_report = demo_start;

    while running.load(Ordering::SeqCst) {
        if demo_start.elapsed() >= max_dur {
            break;
        }

        let now = Instant::now();
        let dt = now.duration_since(last_step).as_secs_f32().min(0.1);
        last_step = now;

        for p in &mut particles {
            p.step(dt, fb_w, fb_h, size_f);
        }

        let mut frame = device
            .begin_frame()
            .fill_rect_unclipped(
                Rect::new(0, 0, info.width, info.height),
                bg,
                BlendMode::Opaque,
            )?;
        for p in &particles {
            frame = frame.fill_rect_unclipped(
                Rect::new(p.x as u16, p.y as u16, flags.size, flags.size),
                p.color,
                BlendMode::Opaque,
            )?;
        }
        frame.present()?
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

    let total = demo_start.elapsed().as_secs_f32();
    eprintln!(
        "menu_demo: {} frames in {:.2} s — average submit FPS: {:.1}",
        frames,
        total,
        frames as f32 / total
    );
    device.stop()?;
    Ok(())
}
