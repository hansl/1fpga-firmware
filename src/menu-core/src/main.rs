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

// `menu_core_host` is reached transitively via this package's `lib.rs`
// re-exports; the binary doesn't reference it directly yet (Task #20
// will rewrite the subcommands to use its runtime API).
use menu_core_host as _;

// Used inside the library's `text` module — declared here to satisfy
// `unused_crate_dependencies` on the binary target.
use fontdue as _;

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

    /// Upload a 64×64 A8 alpha gradient, COPY_RECT it with a red
    /// tint and SrcAlpha blend over a blue background. Visual
    /// verification: smooth fade from blue (alpha=0) to red
    /// (alpha=255). Requires the M2c3.3 blit engine support for
    /// SrcAlpha blending.
    BlendTest,

    /// Rasterise an ASCII font atlas from the bundled Noto Sans TTF,
    /// upload it as an A8 texture, then render a "Hello, 1FPGA!"
    /// string in white over a dark blue background using one
    /// COPY_RECT per glyph with SrcAlpha blend.
    TextTest,

    /// Animate "Hello, 1FPGA!" sliding left-right with ease-in-out
    /// cubic timing for ~5 seconds, reporting FPS at the end. Stresses
    /// the blit engine + ring throughput.
    TextAnim,

    /// Visual test for SET_CLIP / CLEAR_CLIP. Paints a green
    /// background, then sets a clip rect in the centre and tries to
    /// fill the whole screen with red — only the clipped centre
    /// region should change colour.
    ClipTest,
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
        Some(Command::BlendTest) => match blend_test(base) {
            Ok(()) => {}
            Err(e) => {
                tracing::error!("blend-test failed: {e}");
                std::process::exit(1);
            }
        },
        Some(Command::TextTest) => match text_test(base) {
            Ok(()) => {}
            Err(e) => {
                tracing::error!("text-test failed: {e}");
                std::process::exit(1);
            }
        },
        Some(Command::TextAnim) => match text_anim(base) {
            Ok(()) => {}
            Err(e) => {
                tracing::error!("text-anim failed: {e}");
                std::process::exit(1);
            }
        },
        Some(Command::ClipTest) => match clip_test(base) {
            Ok(()) => {}
            Err(e) => {
                tracing::error!("clip-test failed: {e}");
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

fn blend_test(base: u32) -> Result<(), Box<dyn std::error::Error>> {
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

    // Same A8 gradient as a8-test.
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

    let dst_x = (mode.width / 2).saturating_sub(TEX_W as u16 / 2);
    let dst_y = (mode.height / 2).saturating_sub(TEX_H as u16 / 2);

    let blue = Rgba::new(0x00, 0x00, 0xFF, 0xFF);
    let red  = Rgba::new(0xFF, 0x00, 0x00, 0xFF);

    let mut staging = vec![0u8; mem::RING_SIZE];
    let mut writer = menu_core::ring::RingWriter::new(&mut staging)?;
    writer.observe_head(0);

    let commands = [
        ProtoCommand::FillRect {
            dst: Rect::new(0, 0, mode.width, mode.height),
            color: blue,
            blend: BlendMode::Opaque,
            ignore_clip: true,
        },
        ProtoCommand::CopyRect {
            tex_id: 0,
            src: Rect::new(0, 0, TEX_W as u16, TEX_H as u16),
            dst: Rect::new(dst_x, dst_y, TEX_W as u16, TEX_H as u16),
            blend: BlendMode::SrcAlpha,
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
    println!(
        "Pushed FILL(blue) + COPY_RECT(A8 gradient, red tint, SrcAlpha); \
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
        "M2c3.3: check HDMI — should see a {TEX_W}×{TEX_H} smooth fade \
         from blue (alpha=0) to red (alpha=255), blended over a blue background"
    );
    Ok(())
}

/// Bundled Latin Noto Sans, SIL OFL — ~27 KB.
const NOTO_SANS: &[u8] = include_bytes!("../fonts/NotoSans-Regular.ttf");

fn text_test(base: u32) -> Result<(), Box<dyn std::error::Error>> {
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

    // ---- Build the font atlas ----
    let charset: String = (b' '..=b'~').map(|b| b as char).collect();
    let px_size = 48.0_f32;
    let atlas = menu_core::text::build_atlas(NOTO_SANS, px_size, &charset, 512, 512)?;
    println!(
        "Atlas: {}×{}, {} glyphs, line_height={}, ascent={}",
        atlas.width,
        atlas.height,
        charset.len(),
        atlas.line_height,
        atlas.ascent
    );

    // ---- Upload atlas to texture pool ----
    let tex_phys = base + mem::TEX_POOL_OFFSET as u32;
    let mut tex_mapper = DevMemMemoryMapper::create(tex_phys as usize, atlas.bytes.len())
        .map_err(|d| format!("mmap texture pool at {tex_phys:#X}: {d}"))?;
    volatile_copy_to_devmem(tex_mapper.as_mut_ptr::<u8>(), &atlas.bytes);

    // ---- Write the descriptor (one entry covering the whole atlas) ----
    let tex_table_phys = base + mem::TEX_TABLE_OFFSET as u32;
    let descriptor = TextureDescriptor::new(
        tex_phys,
        atlas.width as u32,    // pitch = width for tightly packed A8
        atlas.width,
        atlas.height,
        TextureFormat::A8,
    );
    let descriptor_bytes: [u8; DESCRIPTOR_SIZE] = unsafe { core::mem::transmute(descriptor) };
    let mut desc_mapper = DevMemMemoryMapper::create(tex_table_phys as usize, DESCRIPTOR_SIZE)
        .map_err(|d| format!("mmap descriptor table at {tex_table_phys:#X}: {d}"))?;
    volatile_copy_to_devmem(desc_mapper.as_mut_ptr::<u8>(), &descriptor_bytes);

    regs.write32(registers::TEX_TABLE_ADDR, tex_table_phys);
    regs.write32(registers::TEX_TABLE_COUNT, 1);

    // ---- Lay out the string ----
    let text = "Hello, 1FPGA!";
    let dark_blue = Rgba::new(0x10, 0x10, 0x40, 0xFF);
    let white     = Rgba::new(0xFF, 0xFF, 0xFF, 0xFF);

    // Pen position: horizontally centred, vertically a bit above middle.
    let text_width = atlas.measure(text) as u16;
    let pen_x_start = (mode.width.saturating_sub(text_width)) / 2;
    // Top of the line box. The glyph for each char draws at
    //   top_y = pen_y + (ascent - ymin - height)
    // where pen_y is the line top.
    let pen_y_top = (mode.height / 2).saturating_sub(atlas.line_height / 2);

    let mut staging = vec![0u8; mem::RING_SIZE];
    let mut writer = menu_core::ring::RingWriter::new(&mut staging)?;
    writer.observe_head(0);

    // Background.
    writer.push(&ProtoCommand::FillRect {
        dst: Rect::new(0, 0, mode.width, mode.height),
        color: dark_blue,
        blend: BlendMode::Opaque,
        ignore_clip: true,
    })?;

    // One COPY_RECT per glyph.
    let mut pen_x: i32 = pen_x_start as i32;
    let mut glyph_count = 0_u32;
    for ch in text.chars() {
        let g = match atlas.glyph(ch) {
            Some(g) => *g,
            None => continue,
        };
        if g.width > 0 && g.height > 0 {
            let dst_x = (pen_x + g.bearing_x as i32).max(0) as u16;
            let dst_y_offset = (atlas.ascent as i32) - (g.ymin as i32) - (g.height as i32);
            let dst_y = (pen_y_top as i32 + dst_y_offset).max(0) as u16;
            writer.push(&ProtoCommand::CopyRect {
                tex_id: 0,
                src: Rect::new(g.atlas_x, g.atlas_y, g.width, g.height),
                dst: Rect::new(dst_x, dst_y, g.width, g.height),
                blend: BlendMode::SrcAlpha,
                filter: Filter::Nearest,
                tint: Some(white),
            })?;
            glyph_count += 1;
        }
        pen_x += g.advance as i32;
    }

    writer.push(&ProtoCommand::Present)?;
    writer.push(&ProtoCommand::Fence { value: 0x00C0_FFEE })?;

    let final_tail = writer.tail();
    volatile_copy_to_devmem(ring_devmem_ptr, &staging[..final_tail as usize]);
    println!(
        "Pushed FILL(bg) + {glyph_count}× COPY_RECT(glyph, SrcAlpha) + PRESENT + FENCE; \
         RING_TAIL={final_tail:#X}"
    );

    core::sync::atomic::fence(core::sync::atomic::Ordering::Release);

    regs.write32(registers::CONTROL, registers::CONTROL_ENABLE);
    regs.write32(registers::RING_TAIL, final_tail);
    regs.write32(registers::RING_KICK, 1);

    let target_fence = 0x00C0_FFEEu32;
    let start = std::time::Instant::now();
    let timeout = std::time::Duration::from_millis(15000);
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
        "M2c3.4: check HDMI — should see \"{text}\" in white, centred on a \
         dark blue background"
    );
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

    // Build font atlas + upload + descriptor (same as text-test).
    let charset: String = (b' '..=b'~').map(|b| b as char).collect();
    let atlas = menu_core::text::build_atlas(NOTO_SANS, 48.0, &charset, 512, 512)?;
    println!(
        "Atlas: {}×{}, line_height={}, ascent={}",
        atlas.width, atlas.height, atlas.line_height, atlas.ascent
    );

    let tex_phys = base + mem::TEX_POOL_OFFSET as u32;
    let mut tex_mapper = DevMemMemoryMapper::create(tex_phys as usize, atlas.bytes.len())
        .map_err(|d| format!("mmap texture pool at {tex_phys:#X}: {d}"))?;
    volatile_copy_to_devmem(tex_mapper.as_mut_ptr::<u8>(), &atlas.bytes);

    let tex_table_phys = base + mem::TEX_TABLE_OFFSET as u32;
    let descriptor = TextureDescriptor::new(
        tex_phys,
        atlas.width as u32,
        atlas.width,
        atlas.height,
        TextureFormat::A8,
    );
    let descriptor_bytes: [u8; DESCRIPTOR_SIZE] = unsafe { core::mem::transmute(descriptor) };
    let mut desc_mapper = DevMemMemoryMapper::create(tex_table_phys as usize, DESCRIPTOR_SIZE)
        .map_err(|d| format!("mmap descriptor table at {tex_table_phys:#X}: {d}"))?;
    volatile_copy_to_devmem(desc_mapper.as_mut_ptr::<u8>(), &descriptor_bytes);

    regs.write32(registers::TEX_TABLE_ADDR, tex_table_phys);
    regs.write32(registers::TEX_TABLE_COUNT, 1);

    // ---- Animation parameters ----
    let text = "Hello, 1FPGA!";
    let dark_blue = Rgba::new(0x10, 0x10, 0x40, 0xFF);
    let white     = Rgba::new(0xFF, 0xFF, 0xFF, 0xFF);
    let text_width = atlas.measure(text) as i32;
    let pen_y_top = ((mode.height / 2).saturating_sub(atlas.line_height / 2)) as u16;

    // Travel between left edge and right edge, with margin.
    let margin = 32_i32;
    let pen_x_min = margin;
    let pen_x_max = (mode.width as i32) - text_width - margin;
    let travel = (pen_x_max - pen_x_min).max(0) as f32;

    // Dirty-rect strip: the text only moves horizontally within a band
    // `line_height` tall. With triple-buffering we re-render the strip
    // each frame; the area outside it stays at the background colour
    // we paint into all 3 FBs during the warm-up phase.
    let strip_x: u16 = 0;
    let strip_w: u16 = mode.width;
    let strip_y: u16 = pen_y_top;
    let strip_h: u16 = atlas.line_height;

    let anim_duration = std::time::Duration::from_secs(5);
    let half_period = std::time::Duration::from_millis(1500); // each direction

    // Enable engine ONCE. Don't reset between frames — that would also
    // reset fb_swapper, briefly switching scanout back to FB0 each
    // iteration and producing visible flicker. Instead, append each
    // frame's commands to the ring at the current tail position.
    regs.write32(registers::CONTROL, registers::CONTROL_ENABLE);

    let start = std::time::Instant::now();
    let mut frame_idx: u32 = 0;
    let frame_count_at_start = regs.read32(registers::FRAME_COUNT);
    let mut ring_pos: u32 = 0; // host-side ring write cursor

    // One frame's commands: 1 FILL + ~13 COPYs + PRESENT + FENCE,
    // each ≤ 28 bytes. ~500 bytes is comfortable; round up to 4 KB.
    let frame_buf_size: usize = 4 * 1024;

    while start.elapsed() < anim_duration {
        let elapsed = start.elapsed().as_secs_f32();
        let phase = (elapsed / half_period.as_secs_f32()) % 2.0;
        let normalised = if phase < 1.0 { phase } else { 2.0 - phase };
        let eased = ease_in_out_cubic(normalised);
        let pen_x = pen_x_min + (eased * travel) as i32;

        // Build this frame's command stream into a small per-frame
        // buffer (not a ring — RingWriter would otherwise track its
        // own tail and we'd lose sync with the device side).
        let mut buf = Vec::with_capacity(frame_buf_size);
        let mut emit = |cmd: &ProtoCommand| -> Result<(), Box<dyn std::error::Error>> {
            let n = cmd.encoded_len();
            let start_off = buf.len();
            buf.resize(start_off + n, 0);
            cmd.encode(&mut buf[start_off..start_off + n])?;
            Ok(())
        };

        // Warm-up: paint the full bg into each of the 3 FBs once
        // (frames 0/1/2 land on render_idx 1/2/0 respectively under
        // the swapper's rotation). After that, only refresh the strip
        // around the text. Saves ~95% of the pixels per frame.
        if frame_idx < 3 {
            emit(&ProtoCommand::FillRect {
                dst: Rect::new(0, 0, mode.width, mode.height),
                color: dark_blue,
                blend: BlendMode::Opaque,
                ignore_clip: true,
            })?;
        } else {
            emit(&ProtoCommand::FillRect {
                dst: Rect::new(strip_x, strip_y, strip_w, strip_h),
                color: dark_blue,
                blend: BlendMode::Opaque,
                ignore_clip: true,
            })?;
        }

        let mut x: i32 = pen_x;
        for ch in text.chars() {
            let g = match atlas.glyph(ch) {
                Some(g) => *g,
                None => continue,
            };
            if g.width > 0 && g.height > 0 {
                let dst_x = (x + g.bearing_x as i32).max(0) as u16;
                let dst_y_off = (atlas.ascent as i32) - (g.ymin as i32) - (g.height as i32);
                let dst_y = (pen_y_top as i32 + dst_y_off).max(0) as u16;
                emit(&ProtoCommand::CopyRect {
                    tex_id: 0,
                    src: Rect::new(g.atlas_x, g.atlas_y, g.width, g.height),
                    dst: Rect::new(dst_x, dst_y, g.width, g.height),
                    blend: BlendMode::SrcAlpha,
                    filter: Filter::Nearest,
                    tint: Some(white),
                })?;
            }
            x += g.advance as i32;
        }

        emit(&ProtoCommand::Present)?;
        let fence_value = 0x0000_F000 + frame_idx;
        emit(&ProtoCommand::Fence { value: fence_value })?;

        let frame_size = buf.len() as u32;

        // Bail if a wrap would be needed — for our 5 s × ~60 fps × ~500 B
        // we never wrap the 1 MB ring, but make this explicit.
        if ring_pos as usize + buf.len() > mem::RING_SIZE {
            return Err(format!(
                "ring would wrap at frame {frame_idx}; need wrap-handling for longer runs"
            )
            .into());
        }

        // Volatile-copy this frame's bytes to the ring at the current
        // host-side tail.
        let dst_ptr = unsafe { ring_devmem_ptr.add(ring_pos as usize) };
        volatile_copy_to_devmem(dst_ptr, &buf);

        // Publish: ensure ring writes are visible, then advance TAIL.
        core::sync::atomic::fence(core::sync::atomic::Ordering::Release);
        let new_tail = ring_pos + frame_size;
        regs.write32(registers::RING_TAIL, new_tail);
        regs.write32(registers::RING_KICK, 1);
        ring_pos = new_tail;

        // Wait for this frame's fence.
        let frame_start = std::time::Instant::now();
        let frame_timeout = std::time::Duration::from_millis(500);
        loop {
            let f = regs.read32(registers::FENCE_VALUE);
            if f == fence_value {
                break;
            }
            let status = regs.read32(registers::STATUS);
            if status & registers::STATUS_ERROR != 0 {
                let err = regs.read32(registers::ERROR_INFO);
                return Err(format!(
                    "fetcher error at frame {frame_idx}: STATUS={status:#010X} ERROR_INFO={err:#010X}"
                )
                .into());
            }
            if frame_start.elapsed() > frame_timeout {
                return Err(format!("frame {frame_idx} fence timeout").into());
            }
        }

        frame_idx += 1;
    }

    let total_elapsed = start.elapsed().as_secs_f32();
    let displayed = regs.read32(registers::FRAME_COUNT) - frame_count_at_start;
    println!(
        "Submitted {frame_idx} frames in {total_elapsed:.2} s — submit FPS: {:.1}",
        frame_idx as f32 / total_elapsed
    );
    println!(
        "Displayed {displayed} frames — display FPS: {:.1} (capped at HDMI vsync rate)",
        displayed as f32 / total_elapsed
    );

    regs.write32(registers::CONTROL, 0);
    Ok(())
}

fn clip_test(base: u32) -> Result<(), Box<dyn std::error::Error>> {
    bridge::enable_lwh2f()?;

    let mut regs_mapper = DevMemMemoryMapper::create(REGS_PHYS_ADDR, 0x1000)
        .map_err(|d| format!("mmap regs at {REGS_PHYS_ADDR:#X}: {d}"))?;
    let regs = unsafe { registers::RegisterBlock::new(regs_mapper.as_mut_ptr::<u8>()) };

    if regs.read32(registers::ID) != protocol::ID_VALUE {
        return Err("bad ID".into());
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

    let green = Rgba::new(0x00, 0xC0, 0x00, 0xFF);
    let red   = Rgba::new(0xFF, 0x00, 0x00, 0xFF);
    let blue  = Rgba::new(0x20, 0x40, 0xFF, 0xFF);

    // Centre clip rect: 1/3 of the screen, centred.
    let clip_w = mode.width / 3;
    let clip_h = mode.height / 3;
    let clip_x = (mode.width  - clip_w) / 2;
    let clip_y = (mode.height - clip_h) / 2;

    let mut staging = vec![0u8; mem::RING_SIZE];
    let mut writer = menu_core::ring::RingWriter::new(&mut staging)?;
    writer.observe_head(0);

    // 1. Paint green over the whole screen with ignore_clip so the
    //    background is established regardless of any leftover clip.
    writer.push(&ProtoCommand::FillRect {
        dst: Rect::new(0, 0, mode.width, mode.height),
        color: green,
        blend: BlendMode::Opaque,
        ignore_clip: true,
    })?;

    // 2. Set the user clip rect in the centre.
    writer.push(&ProtoCommand::SetClip(Rect::new(clip_x, clip_y, clip_w, clip_h)))?;

    // 3. Try to paint red over the whole screen (NOT ignoring clip).
    //    Only the centre rect should turn red.
    writer.push(&ProtoCommand::FillRect {
        dst: Rect::new(0, 0, mode.width, mode.height),
        color: red,
        blend: BlendMode::Opaque,
        ignore_clip: false,
    })?;

    // 4. Paint a smaller blue rect with ignore_clip set — should
    //    bypass the user clip and show up even if it's outside the
    //    centre red region.
    writer.push(&ProtoCommand::FillRect {
        dst: Rect::new(20, 20, 80, 80),
        color: blue,
        blend: BlendMode::Opaque,
        ignore_clip: true,
    })?;

    // 5. CLEAR_CLIP and a final small marker to confirm clip is off.
    writer.push(&ProtoCommand::ClearClip)?;
    writer.push(&ProtoCommand::FillRect {
        dst: Rect::new(mode.width.saturating_sub(100), 20, 80, 80),
        color: blue,
        blend: BlendMode::Opaque,
        ignore_clip: false,
    })?;

    writer.push(&ProtoCommand::Present)?;
    writer.push(&ProtoCommand::Fence { value: 0x00C0_FFEE })?;

    let final_tail = writer.tail();
    volatile_copy_to_devmem(ring_devmem_ptr, &staging[..final_tail as usize]);

    core::sync::atomic::fence(core::sync::atomic::Ordering::Release);
    regs.write32(registers::CONTROL, registers::CONTROL_ENABLE);
    regs.write32(registers::RING_TAIL, final_tail);
    regs.write32(registers::RING_KICK, 1);

    let target_fence = 0x00C0_FFEEu32;
    let start = std::time::Instant::now();
    let timeout = std::time::Duration::from_millis(5000);
    loop {
        let fence = regs.read32(registers::FENCE_VALUE);
        if fence == target_fence {
            println!("FENCE reached in {} ms", start.elapsed().as_millis());
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
            return Err("FENCE timeout".into());
        }
    }

    while regs.read32(registers::FRAME_COUNT) < 1 {}
    regs.write32(registers::CONTROL, 0);
    println!(
        "M2c2: check HDMI — green background; red rect ({clip_w}×{clip_h}) \
         centred; one blue square top-left (drawn with ignore_clip while clip \
         was active); one blue square top-right (drawn after CLEAR_CLIP)"
    );
    Ok(())
}
