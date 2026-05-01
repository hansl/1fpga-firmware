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

use clap::{Parser, Subcommand};
use clap_verbosity_flag::Level as VerbosityLevel;
use clap_verbosity_flag::{LogLevel, Verbosity};
use cyclone_v::memory::{DevMemMemoryMapper, MemoryMapper};
use tracing::info;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::fmt::Subscriber;

use menu_core::bridge;
use menu_core::mem;
use menu_core::protocol::{self, BlendMode, Command as ProtoCommand, Rect, Rgba, registers};
use menu_core::ring::RingWriter;

/// Physical address of the menu-core control register block, per
/// PROTOCOL.md §3.
const REGS_PHYS_ADDR: usize = 0xFF21_0000;

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
    /// Requires the M2b ring fetcher to be live on the FPGA side.
    RingTest,

    /// Push a FILL_RECT(red, 100,100, 200×200) plus PRESENT + FENCE.
    /// Visual verification: a red rectangle should appear on HDMI.
    /// Requires the M2c1 blit engine on the FPGA side.
    DrawTest,
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

    match opts.command {
        Some(Command::Probe) => match probe() {
            Ok(()) => {}
            Err(e) => {
                tracing::error!("probe failed: {e}");
                std::process::exit(1);
            }
        },
        Some(Command::RingTest) => match ring_test(base) {
            Ok(()) => {}
            Err(e) => {
                tracing::error!("ring-test failed: {e}");
                std::process::exit(1);
            }
        },
        Some(Command::DrawTest) => match draw_test(base) {
            Ok(()) => {}
            Err(e) => {
                tracing::error!("draw-test failed: {e}");
                std::process::exit(1);
            }
        },
        None => {
            info!(
                "no subcommand given — re-run with `probe`, `ring-test`, `draw-test`, or `--print-layout`"
            );
        }
    }
}

fn probe() -> Result<(), Box<dyn std::error::Error>> {
    bridge::enable_lwh2f()?;

    // mmap the LW_H2F register window. `DevMemMemoryMapper` requires
    // page-aligned (4 KiB) requests on `/dev/mem` — the register block
    // is 256 bytes but the bridge window page that contains it is 4 KiB,
    // so we mmap the full page and only access the first
    // `REGISTER_WINDOW_SIZE` bytes.
    let mut mapper = DevMemMemoryMapper::create(REGS_PHYS_ADDR, 0x1000)
        .map_err(|d| format!("mmap {REGS_PHYS_ADDR:#X}: {d}"))?;
    let regs = unsafe { registers::RegisterBlock::new(mapper.as_mut_ptr::<u8>()) };

    // ID -- read-only constant.
    let id = regs.read32(registers::ID);
    let magic = id >> 16;
    let version = id & 0xFFFF;
    println!(
        "ID:     {id:#010X}  (MAGIC={magic:#06X} VERSION={version})"
    );
    if magic != 0x1FFA {
        return Err(format!("bad MAGIC {magic:#06X}, expected 0x1FFA").into());
    }
    if version != u32::from(protocol::PROTOCOL_VERSION) {
        return Err(format!(
            "version mismatch: FPGA={version}, host={}",
            protocol::PROTOCOL_VERSION
        )
        .into());
    }

    // STATUS -- read-only zero in M2a.
    let status = regs.read32(registers::STATUS);
    println!("STATUS: {status:#010X}");
    if status != 0 {
        return Err(format!("STATUS expected 0, got {status:#010X}").into());
    }

    // CONTROL -- R/W. Write 0x00000001 (enable), read back, then clear.
    regs.write32(registers::CONTROL, registers::CONTROL_ENABLE);
    let after_set = regs.read32(registers::CONTROL);
    if after_set != registers::CONTROL_ENABLE {
        return Err(format!(
            "CONTROL set: wrote {:#010X}, read {after_set:#010X}",
            registers::CONTROL_ENABLE
        )
        .into());
    }
    regs.write32(registers::CONTROL, 0);
    let after_clear = regs.read32(registers::CONTROL);
    if after_clear != 0 {
        return Err(format!(
            "CONTROL clear: wrote 0, read {after_clear:#010X}"
        )
        .into());
    }
    println!("CONTROL scratch round-trip: OK");

    // Pattern-test the rest of the R/W slots. Skip slots whose write
    // semantics aren't full-word scratch:
    //   - ID / STATUS / ERROR_INFO       — read-only constants/sideband
    //   - CONTROL                         — only bit 0 latches; tested above
    //   - VSYNC_COUNT / FRAME_COUNT       — read-only sideband counters
    //   - RING_HEAD / FENCE_VALUE         — read-only sideband from fetcher
    //   - RING_KICK                       — write-only pulse (read returns 0)
    let read_only = [
        registers::ID,
        registers::STATUS,
        registers::CONTROL,
        registers::ERROR_INFO,
        registers::VSYNC_COUNT,
        registers::FRAME_COUNT,
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
        // Restore to 0 so we leave the FPGA in a clean state.
        regs.write32(off, 0);
    }
    println!("Window R/W pattern: {}/{} slots OK", tested - failed, tested);
    if failed != 0 {
        return Err(format!("{failed} register slot(s) failed round-trip").into());
    }

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

/// Volatile byte-copy from a normal slice into a `/dev/mem` mmap'd
/// region. Required because the kernel maps `/dev/mem` opened with
/// `O_SYNC` as device-grade memory on ARMv7 — vectorized stores
/// generated by normal slice writes trip a SIGBUS.
fn volatile_copy_to_devmem(dst: *mut u8, src: &[u8]) {
    for (i, b) in src.iter().enumerate() {
        unsafe { core::ptr::write_volatile(dst.add(i), *b) };
    }
}

fn ring_test(base: u32) -> Result<(), Box<dyn std::error::Error>> {
    bridge::enable_lwh2f()?;

    let mut regs_mapper = DevMemMemoryMapper::create(REGS_PHYS_ADDR, 0x1000)
        .map_err(|d| format!("mmap regs at {REGS_PHYS_ADDR:#X}: {d}"))?;
    let regs = unsafe { registers::RegisterBlock::new(regs_mapper.as_mut_ptr::<u8>()) };

    let id = regs.read32(registers::ID);
    if id != protocol::ID_VALUE {
        return Err(format!("bad ID {id:#010X}, expected {:#010X}", protocol::ID_VALUE).into());
    }

    let ring_phys = base + mem::RING_OFFSET as u32;
    let mut ring_mapper = DevMemMemoryMapper::create(ring_phys as usize, mem::RING_SIZE)
        .map_err(|d| format!("mmap ring at {ring_phys:#X}: {d}"))?;
    let ring_devmem_ptr = ring_mapper.as_mut_ptr::<u8>();

    println!(
        "Ring init: RING_BASE={ring_phys:#010X} RING_SIZE={size}",
        size = mem::RING_SIZE
    );

    // Reset the fetcher first via CONTROL.CE so any previous error
    // state is cleared and HEAD is back at 0.
    regs.write32(registers::CONTROL, registers::CONTROL_CLEAR_ERROR);
    regs.write32(registers::CONTROL, 0);

    // Program the ring base + size and zero the tail.
    regs.write32(registers::RING_BASE, ring_phys);
    regs.write32(registers::RING_SIZE, mem::RING_SIZE as u32);
    regs.write32(registers::RING_TAIL, 0);

    let head_initial = regs.read32(registers::RING_HEAD);
    if head_initial != 0 {
        return Err(format!("RING_HEAD started at {head_initial:#010X}, expected 0").into());
    }

    // Stage commands into a regular heap-backed ring image, then
    // volatile-copy the touched bytes to DDR3. Encoding directly into
    // the /dev/mem mapping is unsafe because the kernel's O_SYNC
    // mapping is device-grade memory on ARMv7 — slice ops trip SIGBUS.
    let mut staging = vec![0u8; mem::RING_SIZE];
    let mut writer = RingWriter::new(&mut staging)?;
    writer.observe_head(0);

    let commands = [
        ProtoCommand::Nop { padding_words: 0 },
        ProtoCommand::Fence { value: 0xDEAD_BEEF },
        ProtoCommand::Nop { padding_words: 7 },
        ProtoCommand::Present,
        ProtoCommand::Fence { value: 0xCAFE_BABE },
    ];

    for cmd in &commands {
        writer.push(cmd)?;
    }
    let final_tail = writer.tail();

    // Volatile-copy only the bytes we wrote (no need to mirror the
    // whole 1 MB ring since RING_HEAD starts at 0 and the FPGA only
    // reads up to RING_TAIL).
    volatile_copy_to_devmem(ring_devmem_ptr, &staging[..final_tail as usize]);

    println!("Pushed {} commands; final RING_TAIL={final_tail:#X}", commands.len());

    // Ensure all ring writes are observed by DDR3 before we publish
    // RING_TAIL. The volatile writes above already bypass the cache
    // (O_SYNC mapping), but a release fence here documents intent.
    core::sync::atomic::fence(core::sync::atomic::Ordering::Release);

    regs.write32(registers::CONTROL, registers::CONTROL_ENABLE);
    regs.write32(registers::RING_TAIL, final_tail);
    regs.write32(registers::RING_KICK, 1);

    // Poll FENCE_VALUE for the second fence (last command).
    let target_fence = 0xCAFE_BABE_u32;
    let start = std::time::Instant::now();
    let timeout = std::time::Duration::from_millis(500);
    loop {
        let fence = regs.read32(registers::FENCE_VALUE);
        if fence == target_fence {
            let elapsed = start.elapsed();
            println!("FENCE_VALUE reached {fence:#010X} in {} µs", elapsed.as_micros());
            break;
        }
        let status = regs.read32(registers::STATUS);
        if status & registers::STATUS_ERROR != 0 {
            let err = regs.read32(registers::ERROR_INFO);
            return Err(format!(
                "fetcher reported error: STATUS={status:#010X} ERROR_INFO={err:#010X}"
            )
            .into());
        }
        if start.elapsed() > timeout {
            let head = regs.read32(registers::RING_HEAD);
            return Err(format!(
                "timeout waiting for FENCE_VALUE={target_fence:#010X} \
                 (got {fence:#010X}, RING_HEAD={head:#X}, RING_TAIL={final_tail:#X}, \
                 STATUS={status:#010X})"
            )
            .into());
        }
    }

    // Verify FRAME_COUNT bumped to 1 (the single PRESENT).
    let frames = regs.read32(registers::FRAME_COUNT);
    if frames != 1 {
        return Err(format!("FRAME_COUNT expected 1, got {frames}").into());
    }
    println!("FRAME_COUNT: {frames} (expected 1)");

    // Verify the consumer caught up.
    let head_final = regs.read32(registers::RING_HEAD);
    if head_final != final_tail {
        return Err(format!(
            "RING_HEAD ({head_final:#X}) did not catch up with RING_TAIL ({final_tail:#X})"
        )
        .into());
    }
    println!("RING_HEAD: {head_final:#X} (caught up)");

    println!("M2b: OK");
    Ok(())
}

fn draw_test(base: u32) -> Result<(), Box<dyn std::error::Error>> {
    bridge::enable_lwh2f()?;

    let mut regs_mapper = DevMemMemoryMapper::create(REGS_PHYS_ADDR, 0x1000)
        .map_err(|d| format!("mmap regs at {REGS_PHYS_ADDR:#X}: {d}"))?;
    let regs = unsafe { registers::RegisterBlock::new(regs_mapper.as_mut_ptr::<u8>()) };

    let id = regs.read32(registers::ID);
    if id != protocol::ID_VALUE {
        return Err(format!("bad ID {id:#010X}, expected {:#010X}", protocol::ID_VALUE).into());
    }

    let ring_phys = base + mem::RING_OFFSET as u32;
    let mut ring_mapper = DevMemMemoryMapper::create(ring_phys as usize, mem::RING_SIZE)
        .map_err(|d| format!("mmap ring at {ring_phys:#X}: {d}"))?;
    let ring_devmem_ptr = ring_mapper.as_mut_ptr::<u8>();

    // Reset fetcher and program ring registers.
    regs.write32(registers::CONTROL, registers::CONTROL_CLEAR_ERROR);
    regs.write32(registers::CONTROL, 0);
    regs.write32(registers::RING_BASE, ring_phys);
    regs.write32(registers::RING_SIZE, mem::RING_SIZE as u32);
    regs.write32(registers::RING_TAIL, 0);

    // Stage commands. M2c1 doesn't yet do triple-buffer swap, so the
    // PRESENT here is a no-op (just bumps FRAME_COUNT) — the FILL_RECT
    // writes directly to the framebuffer that scanout is already
    // reading at 0x3000_0000.
    let mut staging = vec![0u8; mem::RING_SIZE];
    let mut writer = menu_core::ring::RingWriter::new(&mut staging)?;
    writer.observe_head(0);

    let black = Rgba::BLACK;
    let red = Rgba::new(0xFF, 0x00, 0x00, 0xFF);
    let commands = [
        ProtoCommand::FillRect {
            dst: Rect::new(0, 0, 1920, 1080),
            color: black,
            blend: BlendMode::Opaque,
            ignore_clip: true,
        },
        ProtoCommand::FillRect {
            dst: Rect::new(100, 100, 200, 200),
            color: red,
            blend: BlendMode::Opaque,
            ignore_clip: true,
        },
        ProtoCommand::Present,
        ProtoCommand::Fence { value: 0x00C0_FFEE },
    ];

    for cmd in &commands {
        writer.push(cmd)?;
    }
    let final_tail = writer.tail();
    volatile_copy_to_devmem(ring_devmem_ptr, &staging[..final_tail as usize]);

    println!(
        "Pushed FILL_RECT(red, 100, 100, 200×200) + PRESENT + FENCE; \
         RING_TAIL={final_tail:#X}"
    );

    core::sync::atomic::fence(core::sync::atomic::Ordering::Release);

    regs.write32(registers::CONTROL, registers::CONTROL_ENABLE);
    regs.write32(registers::RING_TAIL, final_tail);
    regs.write32(registers::RING_KICK, 1);

    let target_fence = 0x00C0_FFEEu32;
    let start = std::time::Instant::now();
    let timeout = std::time::Duration::from_millis(10000);
    loop {
        let fence = regs.read32(registers::FENCE_VALUE);
        if fence == target_fence {
            println!(
                "FENCE_VALUE reached {fence:#010X} in {} ms",
                start.elapsed().as_millis()
            );
            break;
        }
        let status = regs.read32(registers::STATUS);
        if status & registers::STATUS_ERROR != 0 {
            let err = regs.read32(registers::ERROR_INFO);
            return Err(format!(
                "fetcher error: STATUS={status:#010X} ERROR_INFO={err:#010X}"
            )
            .into());
        }
        if start.elapsed() > timeout {
            let head = regs.read32(registers::RING_HEAD);
            return Err(format!(
                "timeout waiting for FENCE: got {fence:#010X}, RING_HEAD={head:#X}, \
                 RING_TAIL={final_tail:#X}, STATUS={status:#010X}"
            )
            .into());
        }
    }

    let frames = regs.read32(registers::FRAME_COUNT);
    println!("FRAME_COUNT: {frames}");

    // Disable the engine cleanly so the fetcher / blit are fully idle
    // after we exit. Rules out "engine still polling the ring" as a
    // contributor to any post-exit visual artifacts.
    regs.write32(registers::CONTROL, 0);

    println!("M2c1: check HDMI — should see a 200×200 red square at (100, 100)");
    Ok(())
}
