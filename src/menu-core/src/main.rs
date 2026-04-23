//! `one_fpga_menu_core` — host-side driver for the menu-core FPGA
//! bitstream.
//!
//! This binary is the beginning of the ARM-side executable that will
//! program the menu-core RBF into the FPGA, configure the reserved
//! DDR3 carve-out, and drive the command ring to render the 1FPGA main
//! menu. At this stage it is a skeleton — CLI parsing, tracing, CPU
//! pinning, and a sanity-check that prints the protocol-layout
//! information resolved at compile time. Real hardware integration
//! follows in a subsequent session (requires the FPGA bitstream).

use clap::Parser;
use clap_verbosity_flag::Level as VerbosityLevel;
use clap_verbosity_flag::{LogLevel, Verbosity};
use tracing::info;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::fmt::Subscriber;

use menu_core::mem;
use menu_core::protocol::{self, registers};

// `thiserror` is used transitively through the library's error types;
// declare here to satisfy `unused_crate_dependencies` on the binary.
use thiserror as _;

// Only referenced from library test code; the binary target has no
// tests of its own but shares this package's Cargo.toml.
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
}

fn parse_u32_hex_or_dec(s: &str) -> Result<u32, std::num::ParseIntError> {
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u32::from_str_radix(hex, 16)
    } else {
        s.parse::<u32>()
    }
}

fn main() {
    // Mirror the firmware binary: pin to core 1 when available, else core 0.
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
    if base & ((32 * 1024 * 1024) - 1) != 0 {
        tracing::error!("base address {base:#X} is not 32-MB-aligned");
        std::process::exit(2);
    }

    info!(
        "menu-core host driver — protocol v{} (ID = {:#010X})",
        protocol::PROTOCOL_VERSION,
        protocol::ID_VALUE
    );

    if opts.print_layout {
        print_layout(base);
        return;
    }

    info!("hardware integration not yet implemented — re-run with --print-layout");
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
        "Control register window (LW_H2F): 0xFF210000, {} bytes",
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
