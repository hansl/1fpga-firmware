//! `jitter_probe` — minimal scanout-jitter isolation test.
//!
//! Paints the SAME high-contrast horizontal-stripe pattern into all
//! three FB slots, then loops: PRESENT, short sleep, PRESENT, ...
//! After the first three PRESENTs the slots all hold identical
//! content, so the only thing that changes from frame to frame is
//! which slot the fb_swapper has chosen for display. If the displayed
//! image still appears to shift vertically by 1-3 px, the jitter is
//! downstream of the blit engine (ASCAL / vbuf / MISTER_FB scanout)
//! and not anything the host or blit_engine is doing.
//!
//! Why horizontal stripes: a uniform colour wouldn't reveal a 1-px
//! vertical shift visually. Single-pixel-tall alternating stripes
//! turn any vertical scanout misalignment into a visible flicker or
//! colour change (stripe N at output line N+1 reads as colour swap).
//!
//! Usage:
//!   jitter_probe                  # default 1920x1080, default base addr
//!   jitter_probe --present-hz 60  # rate-limit PRESENTs (default: as fast as possible)
//!   jitter_probe --static         # paint once, no further PRESENTs
//!   jitter_probe --max-seconds 30 # bounded run

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

    /// Paint the slots once and then stop. Useful to confirm the
    /// pattern landed correctly without slot rotation.
    #[clap(long)]
    r#static: bool,

    /// Rate-limit PRESENTs to this many per second (0 = unlimited).
    #[clap(long, default_value_t = 0)]
    present_hz: u32,

    /// Height of each colour stripe in pixels.
    #[clap(long, default_value_t = 1)]
    stripe_h: u16,

    /// Maximum duration before auto-exit. 0 = unbounded (Ctrl+C only).
    #[clap(long, default_value_t = 0)]
    max_seconds: u32,

    /// Render resolution as WIDTHxHEIGHT (e.g. `1280x720`). Defaults to
    /// the HDMI native mode. Smaller = less vbuf scanout bandwidth, used
    /// to test whether DDR3 contention (not blit) drives the jitter.
    #[clap(long, value_parser = parse_resolution)]
    render_res: Option<(u16, u16)>,
}

fn parse_resolution(s: &str) -> Result<(u16, u16), String> {
    let (w, h) = s
        .split_once(['x', 'X'])
        .ok_or_else(|| format!("expected WIDTHxHEIGHT, got `{s}`"))?;
    let w: u16 = w.trim().parse().map_err(|e| format!("bad width: {e}"))?;
    let h: u16 = h.trim().parse().map_err(|e| format!("bad height: {e}"))?;
    if w == 0 || h == 0 {
        return Err(format!("resolution must be positive, got {w}x{h}"));
    }
    Ok((w, h))
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
    // Generous fence timeout: under a heavy DRAM stressor the blit
    // engine's ram1 writes are starved, so a full-screen paint can take
    // far longer than a frame. 10 s keeps the paint from spuriously
    // timing out while stress-testing scanout.
    let fence_timeout = Duration::from_secs(10);
    let base = flags.base_addr.unwrap_or(mem::DEFAULT_BASE);
    let mut device = Device::open_with(DeviceConfig {
        base_phys_addr: base,
        ..DeviceConfig::default()
    })?;
    let info = device.video_info();
    // Render resolution: override or HDMI native. ASCAL upscales to
    // the HDMI mode, so a smaller FB just means fewer vbuf bytes/line.
    let (w, h) = flags.render_res.unwrap_or((info.width, info.height));
    let fb = FramebufferConfig {
        width: w,
        height: h,
        stride: (w as u32) * 4,
        ..FramebufferConfig::for_video(info, base)
    };
    device.configure_framebuffer(fb)?;
    device.start()?;

    let stripe_h = flags.stripe_h.max(1);
    info!(
        "jitter_probe: {}x{} fb (HDMI {}x{}), stripe_h={} px, present_hz={}",
        w, h, info.width, info.height, stripe_h, flags.present_hz
    );

    // Paint the same striped pattern into each slot by issuing three
    // back-to-back full-screen paints + PRESENT. Each PRESENT rotates
    // render to the next slot, so frame N targets slot rotation[N].
    // After three frames, all three slots hold the same pattern.
    for fill_pass in 0..3 {
        let mut frame = device.begin_frame();
        let mut y: u16 = 0;
        let mut colour_idx: u32 = 0;
        while y < h {
            let band = stripe_h.min(h - y);
            // Six rotating colours so neighbouring stripes contrast
            // strongly in every channel — any 1-px vertical
            // misalignment makes a strong colour swap visible.
            let c = match colour_idx % 6 {
                0 => Rgba::new(0xFF, 0x00, 0x00, 0xFF), // red
                1 => Rgba::new(0x00, 0xFF, 0x00, 0xFF), // green
                2 => Rgba::new(0x00, 0x00, 0xFF, 0xFF), // blue
                3 => Rgba::new(0xFF, 0xFF, 0x00, 0xFF), // yellow
                4 => Rgba::new(0xFF, 0x00, 0xFF, 0xFF), // magenta
                _ => Rgba::new(0x00, 0xFF, 0xFF, 0xFF), // cyan
            };
            frame = frame.fill_rect_unclipped(
                Rect::new(0, y, w, band),
                c,
                BlendMode::Opaque,
            )?;
            y = y.saturating_add(band);
            colour_idx += 1;
        }
        frame.present()?.submit()?.wait(fence_timeout)?;
        info!("jitter_probe: slot fill pass {} complete", fill_pass);
    }

    if flags.r#static {
        info!("jitter_probe: static mode — holding display, Ctrl+C to exit");
        let running = Arc::new(AtomicBool::new(true));
        {
            let r = running.clone();
            ctrlc::set_handler(move || r.store(false, Ordering::SeqCst))
                .map_err(|e| DeviceError::Io(std::io::Error::other(e.to_string())))?;
        }
        while running.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(100));
        }
        return Ok(());
    }

    // Rotation loop: submit a frame that issues *no* blits, just a
    // PRESENT. The fetcher rotates render_q without writing anything
    // to the FB, so each displayed frame is one of the three slots
    // we painted above — all identical. Visual differences across
    // displayed frames are scanout jitter.
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
    let target_period = if flags.present_hz > 0 {
        Some(Duration::from_secs_f32(1.0 / flags.present_hz as f32))
    } else {
        None
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

        // No blits — just PRESENT. fb_swapper rotates render_q on
        // PRESENT and (at next vsync) swaps display := ready, so the
        // displayed image cycles through the three pre-painted slots.
        device.begin_frame().present()?.submit()?
            .wait(fence_timeout)?;

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

        if let Some(period) = target_period {
            let elapsed = last_step.elapsed();
            if elapsed < period {
                std::thread::sleep(period - elapsed);
            }
            last_step = Instant::now();
        }
    }

    let total = demo_start.elapsed().as_secs_f32();
    eprintln!(
        "jitter_probe: {} present-only frames in {:.2} s — avg {:.1} fps",
        frames,
        total,
        frames as f32 / total.max(0.001),
    );

    Ok(())
}
