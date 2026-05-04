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
use menu_core::protocol::{
    self, BlendMode, Command as ProtoCommand, Filter, Rect, Rgba, TextureDescriptor,
    TextureFormat, descriptors::DESCRIPTOR_SIZE, registers,
};
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

    /// Upload a 64×64 RGBA8888 checkerboard texture, COPY_RECT it
    /// onto the framebuffer, PRESENT + FENCE. Visual verification:
    /// a checkerboard appears on HDMI. Requires the M2c3.1 blit
    /// engine support for COPY_RECT.
    TextureTest,

    /// Upload a 64×64 A8 alpha gradient, COPY_RECT it with a red
    /// tint onto a black background. Visual verification: a
    /// horizontal gradient from black (left) to red (right).
    /// Requires the M2c3.2 blit engine support for A8 + tint.
    A8Test,
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
        Some(Command::TextureTest) => match texture_test(base) {
            Ok(()) => {}
            Err(e) => {
                tracing::error!("texture-test failed: {e}");
                std::process::exit(1);
            }
        },
        Some(Command::A8Test) => match a8_test(base) {
            Ok(()) => {}
            Err(e) => {
                tracing::error!("a8-test failed: {e}");
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

    let info = VideoMode::from_register(regs.read32(registers::VIDEO_INFO));
    println!("VIDEO_INFO: {}×{}", info.width, info.height);

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

/// Active HDMI mode resolution as reported by the framework.
#[derive(Copy, Clone, Debug)]
struct VideoMode {
    width: u16,
    height: u16,
}

impl VideoMode {
    fn from_register(value: u32) -> Self {
        Self {
            width: (value & 0xFFFF) as u16,
            height: (value >> 16) as u16,
        }
    }
}

/// Program the framebuffer geometry to match the active HDMI mode.
///
/// Reads VIDEO_INFO, picks the same dimensions for FB0, and writes
/// FB0_ADDR / FB_WIDTH / FB_HEIGHT / FB_STRIDE. The framework will pick
/// these up on the next FB_EN sample.
fn configure_framebuffer(
    regs: &registers::RegisterBlock,
    base: u32,
) -> Result<VideoMode, Box<dyn std::error::Error>> {
    let info = VideoMode::from_register(regs.read32(registers::VIDEO_INFO));
    if info.width == 0 || info.height == 0 {
        return Err(format!(
            "VIDEO_INFO reports {}×{} — HDMI mode may not be set yet (boot MiSTer once \
             with a valid video_mode in MiSTer.ini, then re-run)",
            info.width, info.height
        )
        .into());
    }

    let used_bytes = (info.width as usize) * (info.height as usize) * 4;
    if used_bytes > mem::FB_SLOT_SIZE {
        return Err(format!(
            "{}×{} ({} bytes) exceeds 8 MB FB slot — current cap is 1920×1080",
            info.width, info.height, used_bytes
        )
        .into());
    }

    regs.write32(registers::FB0_ADDR, base + mem::FB0_OFFSET as u32);
    regs.write32(registers::FB1_ADDR, base + mem::FB1_OFFSET as u32);
    regs.write32(registers::FB2_ADDR, base + mem::FB2_OFFSET as u32);
    regs.write32(registers::FB_WIDTH, info.width as u32);
    regs.write32(registers::FB_HEIGHT, info.height as u32);
    regs.write32(registers::FB_STRIDE, (info.width as u32) * 4);
    Ok(info)
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

    // Configure the FB so PRESENT has a coherent target even though
    // this test doesn't draw.
    let _ = configure_framebuffer(&regs, base)?;

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

    // FRAME_COUNT increments on the vsync that actually swaps the
    // queued framebuffer in (M2c4), which can lag the FENCE by up to
    // one frame interval (~16 ms at 60 Hz).
    let frames_target = 1u32;
    let frame_start = std::time::Instant::now();
    let frame_timeout = std::time::Duration::from_millis(40);
    let frames = loop {
        let f = regs.read32(registers::FRAME_COUNT);
        if f >= frames_target {
            break f;
        }
        if frame_start.elapsed() > frame_timeout {
            return Err(format!(
                "FRAME_COUNT did not reach {frames_target} within {} ms (got {f})",
                frame_timeout.as_millis()
            )
            .into());
        }
    };
    println!("FRAME_COUNT: {frames} (expected ≥ 1)");

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

    let mode = configure_framebuffer(&regs, base)?;
    println!(
        "Video: {}×{} (FB0={:#010X}, stride={})",
        mode.width,
        mode.height,
        base + mem::FB0_OFFSET as u32,
        mode.width as u32 * 4
    );

    // Test rect: centered, half-screen. Adapts to whatever the active
    // mode is so the same test exercises any resolution.
    let rect_w = mode.width / 2;
    let rect_h = mode.height / 2;
    let rect_x = mode.width / 4;
    let rect_y = mode.height / 4;

    let mut staging = vec![0u8; mem::RING_SIZE];
    let mut writer = menu_core::ring::RingWriter::new(&mut staging)?;
    writer.observe_head(0);

    let black = Rgba::BLACK;
    let red = Rgba::new(0xFF, 0x00, 0x00, 0xFF);
    let commands = [
        ProtoCommand::FillRect {
            dst: Rect::new(0, 0, mode.width, mode.height),
            color: black,
            blend: BlendMode::Opaque,
            ignore_clip: true,
        },
        ProtoCommand::FillRect {
            dst: Rect::new(rect_x, rect_y, rect_w, rect_h),
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
        "Pushed clear + FILL_RECT(red, {rect_x}, {rect_y}, {rect_w}×{rect_h}) + \
         PRESENT + FENCE; RING_TAIL={final_tail:#X}"
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

    // Wait for the vsync that completes the swap (FRAME_COUNT bumps
    // when the queued FB actually becomes DISPLAY, up to ~16 ms after
    // PRESENT retires).
    let frame_start = std::time::Instant::now();
    let frame_timeout = std::time::Duration::from_millis(40);
    let frames = loop {
        let f = regs.read32(registers::FRAME_COUNT);
        if f >= 1 {
            break f;
        }
        if frame_start.elapsed() > frame_timeout {
            return Err(format!(
                "FRAME_COUNT did not reach 1 within {} ms (got {f})",
                frame_timeout.as_millis()
            )
            .into());
        }
    };
    let fb_state = regs.read32(registers::FB_STATE);
    println!(
        "FRAME_COUNT: {frames}  FB_STATE: {fb_state:#010X} (display={}, render={}, ready={})",
        registers::fb_state_display(fb_state),
        registers::fb_state_render(fb_state),
        registers::fb_state_ready(fb_state)
    );

    regs.write32(registers::CONTROL, 0);

    println!(
        "M2c1: check HDMI — should see a {rect_w}×{rect_h} red square at \
         ({rect_x}, {rect_y})"
    );
    Ok(())
}

fn texture_test(base: u32) -> Result<(), Box<dyn std::error::Error>> {
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

    let mode = configure_framebuffer(&regs, base)?;
    println!("Video: {}×{}", mode.width, mode.height);

    // ---- Build a 64×64 RGBA8888 checkerboard in a regular Vec ----
    // 8×8 cells of alternating colours so the result is visually
    // unambiguous even at small sizes.
    const TEX_W: usize = 64;
    const TEX_H: usize = 64;
    const CELL: usize = 8;
    let red = Rgba::new(0xFF, 0x00, 0x00, 0xFF);
    let yellow = Rgba::new(0xFF, 0xFF, 0x00, 0xFF);
    let mut tex_bytes = vec![0u8; TEX_W * TEX_H * 4];
    for y in 0..TEX_H {
        for x in 0..TEX_W {
            let cell_x = x / CELL;
            let cell_y = y / CELL;
            let color = if (cell_x + cell_y) % 2 == 0 { red } else { yellow };
            let off = (y * TEX_W + x) * 4;
            let word = color.to_u32().to_le_bytes();
            tex_bytes[off..off + 4].copy_from_slice(&word);
        }
    }

    // ---- Upload texture to the texture pool in DDR3 ----
    let tex_phys = base + mem::TEX_POOL_OFFSET as u32;
    let mut tex_mapper = DevMemMemoryMapper::create(tex_phys as usize, tex_bytes.len())
        .map_err(|d| format!("mmap texture pool at {tex_phys:#X}: {d}"))?;
    volatile_copy_to_devmem(tex_mapper.as_mut_ptr::<u8>(), &tex_bytes);

    // ---- Build and upload the descriptor table (one entry: tex_id 0) ----
    let tex_table_phys = base + mem::TEX_TABLE_OFFSET as u32;
    let descriptor = TextureDescriptor::new(
        tex_phys,
        (TEX_W * 4) as u32,
        TEX_W as u16,
        TEX_H as u16,
        TextureFormat::Rgba8888,
    );
    let descriptor_bytes: [u8; DESCRIPTOR_SIZE] = unsafe {
        // SAFETY: TextureDescriptor is `#[repr(C)]` and exactly 32 bytes.
        core::mem::transmute(descriptor)
    };
    let mut desc_mapper =
        DevMemMemoryMapper::create(tex_table_phys as usize, DESCRIPTOR_SIZE)
            .map_err(|d| format!("mmap descriptor table at {tex_table_phys:#X}: {d}"))?;
    volatile_copy_to_devmem(desc_mapper.as_mut_ptr::<u8>(), &descriptor_bytes);

    regs.write32(registers::TEX_TABLE_ADDR, tex_table_phys);
    regs.write32(registers::TEX_TABLE_COUNT, 1);
    println!(
        "Uploaded {TEX_W}×{TEX_H} RGBA8888 texture at {tex_phys:#010X}; \
         descriptor at {tex_table_phys:#010X}"
    );

    // ---- Build the command stream ----
    // Centre the checkerboard on screen at native pixel size.
    let dst_x = (mode.width / 2).saturating_sub(TEX_W as u16 / 2);
    let dst_y = (mode.height / 2).saturating_sub(TEX_H as u16 / 2);

    let mut staging = vec![0u8; mem::RING_SIZE];
    let mut writer = menu_core::ring::RingWriter::new(&mut staging)?;
    writer.observe_head(0);

    let commands = [
        ProtoCommand::FillRect {
            dst: Rect::new(0, 0, mode.width, mode.height),
            color: Rgba::BLACK,
            blend: BlendMode::Opaque,
            ignore_clip: true,
        },
        ProtoCommand::CopyRect {
            tex_id: 0,
            src: Rect::new(0, 0, TEX_W as u16, TEX_H as u16),
            dst: Rect::new(dst_x, dst_y, TEX_W as u16, TEX_H as u16),
            blend: BlendMode::Opaque,
            filter: Filter::Nearest,
            tint: None,
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
        "Pushed clear + COPY_RECT(tex 0, {TEX_W}×{TEX_H} -> ({dst_x}, {dst_y})) + \
         PRESENT + FENCE; RING_TAIL={final_tail:#X}"
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

    let frame_start = std::time::Instant::now();
    let frame_timeout = std::time::Duration::from_millis(40);
    loop {
        if regs.read32(registers::FRAME_COUNT) >= 1 {
            break;
        }
        if frame_start.elapsed() > frame_timeout {
            return Err("FRAME_COUNT did not reach 1 within 40 ms".into());
        }
    }

    regs.write32(registers::CONTROL, 0);

    println!(
        "M2c3.1: check HDMI — should see a {TEX_W}×{TEX_H} red/yellow \
         checkerboard centred on a black background"
    );
    Ok(())
}

fn a8_test(base: u32) -> Result<(), Box<dyn std::error::Error>> {
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

    regs.write32(registers::CONTROL, registers::CONTROL_CLEAR_ERROR);
    regs.write32(registers::CONTROL, 0);
    regs.write32(registers::RING_BASE, ring_phys);
    regs.write32(registers::RING_SIZE, mem::RING_SIZE as u32);
    regs.write32(registers::RING_TAIL, 0);

    let mode = configure_framebuffer(&regs, base)?;
    println!("Video: {}×{}", mode.width, mode.height);

    // ---- 64×64 horizontal alpha gradient: alpha = x * 4 (0..252) ----
    const TEX_W: usize = 64;
    const TEX_H: usize = 64;
    let mut tex_bytes = vec![0u8; TEX_W * TEX_H];
    for y in 0..TEX_H {
        for x in 0..TEX_W {
            tex_bytes[y * TEX_W + x] = (x * 4) as u8;
        }
    }

    let tex_phys = base + mem::TEX_POOL_OFFSET as u32;
    let mut tex_mapper = DevMemMemoryMapper::create(tex_phys as usize, tex_bytes.len())
        .map_err(|d| format!("mmap texture pool at {tex_phys:#X}: {d}"))?;
    volatile_copy_to_devmem(tex_mapper.as_mut_ptr::<u8>(), &tex_bytes);

    let tex_table_phys = base + mem::TEX_TABLE_OFFSET as u32;
    let descriptor = TextureDescriptor::new(
        tex_phys,
        TEX_W as u32,
        TEX_W as u16,
        TEX_H as u16,
        TextureFormat::A8,
    );
    let descriptor_bytes: [u8; DESCRIPTOR_SIZE] = unsafe { core::mem::transmute(descriptor) };
    let mut desc_mapper = DevMemMemoryMapper::create(tex_table_phys as usize, DESCRIPTOR_SIZE)
        .map_err(|d| format!("mmap descriptor table at {tex_table_phys:#X}: {d}"))?;
    volatile_copy_to_devmem(desc_mapper.as_mut_ptr::<u8>(), &descriptor_bytes);

    regs.write32(registers::TEX_TABLE_ADDR, tex_table_phys);
    regs.write32(registers::TEX_TABLE_COUNT, 1);
    println!(
        "Uploaded {TEX_W}×{TEX_H} A8 gradient at {tex_phys:#010X}; \
         descriptor at {tex_table_phys:#010X}"
    );

    // Centre the gradient on screen.
    let dst_x = (mode.width / 2).saturating_sub(TEX_W as u16 / 2);
    let dst_y = (mode.height / 2).saturating_sub(TEX_H as u16 / 2);
    let red = Rgba::new(0xFF, 0x00, 0x00, 0xFF);

    let mut staging = vec![0u8; mem::RING_SIZE];
    let mut writer = menu_core::ring::RingWriter::new(&mut staging)?;
    writer.observe_head(0);

    let commands = [
        ProtoCommand::FillRect {
            dst: Rect::new(0, 0, mode.width, mode.height),
            color: Rgba::BLACK,
            blend: BlendMode::Opaque,
            ignore_clip: true,
        },
        ProtoCommand::CopyRect {
            tex_id: 0,
            src: Rect::new(0, 0, TEX_W as u16, TEX_H as u16),
            dst: Rect::new(dst_x, dst_y, TEX_W as u16, TEX_H as u16),
            blend: BlendMode::Opaque,
            filter: Filter::Nearest,
            tint: Some(red),
        },
        ProtoCommand::Present,
        ProtoCommand::Fence { value: 0x00C0_FFEE },
    ];

    for cmd in &commands {
        writer.push(cmd)?;
    }
    let final_tail = writer.tail();
    volatile_copy_to_devmem(ring_devmem_ptr, &staging[..final_tail as usize]);
    println!("Pushed clear + COPY_RECT(A8 gradient, red tint); RING_TAIL={final_tail:#X}");

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
            return Err("timeout waiting for FENCE".into());
        }
    }

    let frame_start = std::time::Instant::now();
    while regs.read32(registers::FRAME_COUNT) < 1 {
        if frame_start.elapsed() > std::time::Duration::from_millis(40) {
            return Err("FRAME_COUNT did not reach 1 within 40 ms".into());
        }
    }

    regs.write32(registers::CONTROL, 0);
    println!(
        "M2c3.2: check HDMI — should see a {TEX_W}×{TEX_H} horizontal gradient \
         (black left, red right) centred on a black background"
    );
    Ok(())
}
