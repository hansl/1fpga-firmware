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
    };
    if let Err(e) = menu_ui::run(cfg) {
        error!("{e}");
        std::process::exit(1);
    }
}
