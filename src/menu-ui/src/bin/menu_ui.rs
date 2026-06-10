//! `menu_ui` — launcher binary for the React-on-Boa UI framework.
//!
//! The binary is intentionally thin: parse CLI flags, init tracing,
//! pin to a CPU, then hand off to [`menu_ui::run`].

use std::path::PathBuf;

use clap::Parser;
use clap_verbosity_flag::Level as VerbosityLevel;
use clap_verbosity_flag::Verbosity;

// Crates only used by the lib target; declared here to satisfy the
// workspace `unused_crate_dependencies` lint on the bin target.
use boa_engine as _;
use boa_gc as _;
use boa_macros as _;
use boa_runtime as _;
use ctrlc as _;
use evdev as _;
use fontdue as _;
use menu_core_host as _;
use png as _;
use taffy as _;
use thiserror as _;
use tracing::error;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::fmt::Subscriber;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Flags {
    #[command(flatten)]
    verbose: Verbosity<clap_verbosity_flag::InfoLevel>,

    /// Override the embedded JS bundle with one read from disk.
    /// Convenient during development — iterate on the JS without
    /// rebuilding the Rust binary.
    #[clap(long)]
    bundle: Option<PathBuf>,

    /// Override the reserved DDR3 base address (default: 0x30000000).
    #[clap(long, value_parser = parse_u32_hex_or_dec)]
    base_addr: Option<u32>,

    /// Render resolution as WIDTHxHEIGHT (e.g. `1280x720`, `1920x1080`,
    /// `640x480`). Defaults to the HDMI mode's native resolution
    /// (read from VIDEO_INFO). The framework's ASCAL block handles
    /// the scale-to-HDMI step, so any resolution ≤ the HDMI mode is
    /// safe. Smaller render resolutions reduce per-frame DDR3 work
    /// proportionally — useful on slower mash-FPS at 1080p.
    #[clap(long, value_parser = parse_resolution)]
    render_res: Option<(u16, u16)>,

    /// Wallpaper PNG uploaded to the scanout compositor's wallpaper layer.
    /// When it loads, the wallpaper becomes a hardware layer and content is
    /// composited over it — the content framebuffer no longer redraws the
    /// wallpaper every frame. Defaults to the standard assets path; point
    /// it at a missing file to disable compositing (content clears opaque).
    #[clap(long, default_value = "/media/fat/menu_ui_assets/bg.png")]
    wallpaper: PathBuf,

    /// Content coverage mask (task #15): when compositing, the host hands
    /// the compositor a 64x64-tile bitmap so it skips reading the
    /// transparent majority of the content layer. Default OFF — for the
    /// scattered main-menu content the tile-masked read fragments the DDR
    /// access and is net-slower than the full sequential read; it's kept
    /// for contiguous upper layers (Phase D). Pass `--content-mask true`
    /// to experiment.
    #[clap(long = "content-mask", default_value_t = false, action = clap::ArgAction::Set)]
    content_mask: bool,
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

    let cfg = menu_ui::RunConfig {
        bundle_override: flags.bundle,
        base_phys_addr: flags.base_addr,
        render_res: flags.render_res,
        wallpaper: Some(flags.wallpaper),
        content_mask: flags.content_mask,
    };
    if let Err(e) = menu_ui::run(cfg) {
        error!("{e}");
        std::process::exit(1);
    }
}
