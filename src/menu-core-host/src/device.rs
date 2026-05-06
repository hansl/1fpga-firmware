//! Device runtime handle.
//!
//! [`Device`] owns the LW_H2F register mmap and the command-ring mmap,
//! tracks the host-side ring tail and the next fence value, and exposes
//! primitives for framebuffer configuration, engine start/stop, and
//! polling. Per-frame command building lives in [`crate::frame::Frame`],
//! returned by [`Device::begin_frame`].
//!
//! The device handle is not `Send` or `Sync`: the FPGA shares physical
//! memory with us and command submission requires single-producer
//! discipline against the ring. Wrap it in a `Mutex` if you need to
//! cross threads.

use std::time::{Duration, Instant};

use crate::allocator::{AllocError, BumpAllocator};
use crate::bridge;
use crate::devmem::{DevMemMap, volatile_copy_to_devmem};
use crate::error::{DeviceError, HardwareError};
use crate::mem;
use crate::protocol::descriptors::{DESCRIPTOR_SIZE, TextureDescriptor};
use crate::protocol::{self, registers};
use crate::ring::RingError;
use crate::texture::{TextureHandle, TextureSpec};

/// Default LW_H2F register block physical address (PROTOCOL.md §3).
pub const REGS_PHYS_ADDR: u32 = 0xFF21_0000;

/// Alignment in bytes used for both the base address and the row stride
/// of texture allocations. 64 = 16 RGBA pixels = the largest burst the
/// FPGA blit engine emits, so every aligned-row pixel-zero qualifies for
/// the maximum-throughput burst tier.
const TEX_BURST_ALIGN: u32 = 64;

/// Round `x` up to the next multiple of `align`. `align` must be a
/// power of two.
#[inline]
fn align_up(x: u32, align: u32) -> u32 {
    debug_assert!(align.is_power_of_two());
    (x + align - 1) & !(align - 1)
}

/// Configuration knobs for [`Device::open_with`]. Defaults match the
/// production layout used by the firmware.
#[derive(Debug, Clone, Copy)]
pub struct DeviceConfig {
    /// Base physical address of the reserved DDR3 carve-out. Must be
    /// 32-MB-aligned (PROTOCOL.md §2).
    pub base_phys_addr: u32,
    /// Physical address of the LW_H2F register block.
    pub regs_phys_addr: u32,
    /// Command ring size in bytes (must be a power of two).
    pub ring_size: usize,
}

impl Default for DeviceConfig {
    fn default() -> Self {
        Self {
            base_phys_addr: mem::DEFAULT_BASE,
            regs_phys_addr: REGS_PHYS_ADDR,
            ring_size: mem::RING_SIZE,
        }
    }
}

/// Active HDMI mode dimensions, as latched into `VIDEO_INFO` by the
/// scanout engine. Both fields are zero before the framework supplies
/// a valid mode.
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub struct VideoInfo {
    pub width: u16,
    pub height: u16,
}

/// Decoded `FB_STATE` register snapshot (PROTOCOL.md §3.3).
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub struct FbState {
    pub display: u8,
    pub render: u8,
    /// `3` indicates "no frame pending" — the ready slot is sentinel.
    pub ready: u8,
}

/// Framebuffer geometry plus per-slot physical addresses. Built from a
/// [`VideoInfo`] via [`FramebufferConfig::for_video`] for the standard
/// triple-buffer layout, or constructed manually for custom setups.
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub struct FramebufferConfig {
    pub width: u16,
    pub height: u16,
    pub stride: u32,
    pub fb0_phys: u32,
    pub fb1_phys: u32,
    pub fb2_phys: u32,
}

impl FramebufferConfig {
    /// Build the standard triple-buffer config for the given video mode
    /// rooted at `base`. Stride defaults to `width * 4` (BGRA8888).
    pub fn for_video(info: VideoInfo, base: u32) -> Self {
        Self {
            width: info.width,
            height: info.height,
            stride: (info.width as u32) * 4,
            fb0_phys: base + mem::FB0_OFFSET as u32,
            fb1_phys: base + mem::FB1_OFFSET as u32,
            fb2_phys: base + mem::FB2_OFFSET as u32,
        }
    }
}

/// Owning device handle — opened once at startup, dropped on exit.
pub struct Device {
    cfg: DeviceConfig,
    // Kept alive so the underlying mmap regions stay valid; the regs
    // pointer and ring pointer reach into these mappings.
    regs_map: DevMemMap,
    ring_map: DevMemMap,
    regs: registers::RegisterBlock,
    fb_config: Option<FramebufferConfig>,
    ring_tail: u32,
    next_fence: u32,
    started: bool,
    // Texture storage is mapped lazily on the first `upload_texture`
    // call — opening 224 MB of /dev/mem is wasted work for callers
    // that only need solid-color drawing.
    tex_pool_map: Option<DevMemMap>,
    tex_table_map: Option<DevMemMap>,
    tex_alloc: BumpAllocator,
    next_tex_id: u16,
    tex_table_capacity: u16,
}

impl Device {
    /// Open the device with default configuration.
    ///
    /// Steps performed: enable the LW_H2F bridge, mmap the register
    /// window, validate the `ID` register, mmap the command ring,
    /// clear any prior error, and program the ring base/size/tail.
    /// The engine is **not** started; call [`Device::start`] after
    /// configuring the framebuffer (and uploading any startup
    /// textures) to begin command processing.
    pub fn open() -> Result<Self, DeviceError> {
        Self::open_with(DeviceConfig::default())
    }

    /// Open the device with a custom configuration.
    pub fn open_with(cfg: DeviceConfig) -> Result<Self, DeviceError> {
        if cfg.base_phys_addr & ((32 * 1024 * 1024) - 1) != 0 {
            return Err(DeviceError::BadBaseAlignment(cfg.base_phys_addr));
        }
        if !cfg.ring_size.is_power_of_two() {
            return Err(DeviceError::Ring(RingError::InvalidSize(cfg.ring_size)));
        }

        bridge::enable_lwh2f()?;

        let mut regs_map = DevMemMap::create(cfg.regs_phys_addr, registers::REGISTER_WINDOW_SIZE)?;
        // SAFETY: `regs_map` covers REGISTER_WINDOW_SIZE bytes and lives
        // as long as `Device`; the RegisterBlock only reads/writes
        // within that window.
        let regs = unsafe { registers::RegisterBlock::new(regs_map.as_mut_ptr()) };

        let id = regs.read32(registers::ID);
        if id != protocol::ID_VALUE {
            return Err(DeviceError::IdMismatch {
                got: id,
                expected: protocol::ID_VALUE,
            });
        }

        let ring_phys = cfg.base_phys_addr + mem::RING_OFFSET as u32;
        let ring_map = DevMemMap::create(ring_phys, cfg.ring_size)?;

        regs.write32(registers::CONTROL, registers::CONTROL_CLEAR_ERROR);
        regs.write32(registers::CONTROL, 0);

        regs.write32(registers::RING_BASE, ring_phys);
        regs.write32(registers::RING_SIZE, cfg.ring_size as u32);
        regs.write32(registers::RING_TAIL, 0);

        let tex_pool_phys = cfg.base_phys_addr + mem::TEX_POOL_OFFSET as u32;
        Ok(Self {
            cfg,
            regs_map,
            ring_map,
            regs,
            fb_config: None,
            ring_tail: 0,
            next_fence: 1,
            started: false,
            tex_pool_map: None,
            tex_table_map: None,
            tex_alloc: BumpAllocator::new(tex_pool_phys, mem::TEX_POOL_SIZE as u32),
            next_tex_id: 0,
            tex_table_capacity: mem::DEFAULT_TEX_TABLE_COUNT as u16,
        })
    }

    /// The configuration this device was opened with.
    #[inline]
    pub fn config(&self) -> DeviceConfig {
        self.cfg
    }

    /// Read the active video mode from `VIDEO_INFO`. Returns
    /// `(0, 0)` if the framework has not latched a valid mode yet.
    pub fn video_info(&self) -> VideoInfo {
        let v = self.regs.read32(registers::VIDEO_INFO);
        VideoInfo {
            width: (v & 0xFFFF) as u16,
            height: (v >> 16) as u16,
        }
    }

    /// Program `FB0/1/2_ADDR`, `FB_WIDTH`, `FB_HEIGHT`, `FB_STRIDE` to
    /// match `fb`. Validates that the framebuffer fits an 8 MB slot and
    /// that the dimensions are non-zero.
    pub fn configure_framebuffer(&mut self, fb: FramebufferConfig) -> Result<(), DeviceError> {
        if fb.width == 0 || fb.height == 0 {
            return Err(DeviceError::VideoNotActive {
                width: fb.width,
                height: fb.height,
            });
        }
        let bytes = (fb.width as usize) * (fb.height as usize) * 4;
        if bytes > mem::FB_SLOT_SIZE {
            return Err(DeviceError::FramebufferTooLarge {
                width: fb.width,
                height: fb.height,
                bytes,
                slot_bytes: mem::FB_SLOT_SIZE,
            });
        }

        self.regs.write32(registers::FB0_ADDR, fb.fb0_phys);
        self.regs.write32(registers::FB1_ADDR, fb.fb1_phys);
        self.regs.write32(registers::FB2_ADDR, fb.fb2_phys);
        self.regs.write32(registers::FB_WIDTH, fb.width as u32);
        self.regs.write32(registers::FB_HEIGHT, fb.height as u32);
        self.regs.write32(registers::FB_STRIDE, fb.stride);
        self.fb_config = Some(fb);
        Ok(())
    }

    /// Last-applied [`FramebufferConfig`], if any.
    #[inline]
    pub fn framebuffer(&self) -> Option<&FramebufferConfig> {
        self.fb_config.as_ref()
    }

    /// Begin a new frame. The returned [`Frame`](crate::frame::Frame)
    /// builds a command batch in a host-side staging buffer; call
    /// `submit()` on it to ship the batch to the device.
    pub fn begin_frame(&mut self) -> crate::frame::Frame<'_> {
        crate::frame::Frame::new(self)
    }

    /// Upload a texture into the DDR3 pool. Lazily maps the pool +
    /// descriptor table on first call.
    pub fn upload_texture(&mut self, spec: &TextureSpec) -> Result<TextureHandle, DeviceError> {
        if self.tex_pool_map.is_none() {
            self.init_texture_storage()?;
        }

        let bpp = spec.format.bytes_per_pixel();
        let row_bytes = (spec.width as u32) * bpp;
        if spec.stride < row_bytes {
            return Err(DeviceError::TextureStrideTooSmall {
                stride: spec.stride,
                row_bytes,
            });
        }
        let needed_bytes = (spec.stride as usize) * (spec.height as usize);
        if spec.data.len() < needed_bytes {
            return Err(DeviceError::TextureDataTruncated {
                expected: needed_bytes,
                got: spec.data.len(),
            });
        }
        if self.next_tex_id >= self.tex_table_capacity {
            return Err(DeviceError::DescriptorTableFull {
                capacity: self.tex_table_capacity as u32,
            });
        }

        // Allocate with TEX_BURST_ALIGN. Caller-supplied `spec.stride`
        // is honoured as-is — uploaded textures (fonts, PNGs) own their
        // pixel layout. Higher-burst eligibility on the source side
        // depends on the user picking a stride that's a TEX_BURST_ALIGN
        // multiple; otherwise the dispatch falls back to smaller bursts.
        let phys = self
            .tex_alloc
            .alloc(needed_bytes as u32, TEX_BURST_ALIGN)
            .map_err(|e| match e {
                AllocError::OutOfMemory {
                    requested,
                    remaining,
                } => DeviceError::TexturePoolExhausted {
                    needed: requested,
                    free: remaining,
                },
                AllocError::BadAlignment(_) => unreachable!("64 is power of two"),
            })?;

        // Copy pixel data.
        let pool_base_phys = self.cfg.base_phys_addr + mem::TEX_POOL_OFFSET as u32;
        let offset_in_pool = (phys - pool_base_phys) as usize;
        let pool_map = self.tex_pool_map.as_mut().expect("init checked above");
        // SAFETY: bump allocator guarantees `phys + needed_bytes <=
        // pool_base_phys + TEX_POOL_SIZE`, so the dst window is in
        // bounds. `volatile_copy_to_devmem` writes byte-by-byte volatile.
        unsafe {
            let dst = pool_map.as_mut_ptr().add(offset_in_pool);
            volatile_copy_to_devmem(dst, &spec.data[..needed_bytes]);
        }

        // Build descriptor and copy into the descriptor table.
        let descriptor =
            TextureDescriptor::new(phys, spec.stride, spec.width, spec.height, spec.format);
        // SAFETY: TextureDescriptor is repr(C), 32 bytes, no padding holes
        // in the public layout (asserted at compile time).
        let descriptor_bytes = unsafe {
            core::slice::from_raw_parts(
                (&descriptor as *const TextureDescriptor) as *const u8,
                DESCRIPTOR_SIZE,
            )
        };
        let id = self.next_tex_id;
        let table_offset = (id as usize) * DESCRIPTOR_SIZE;
        let table_map = self.tex_table_map.as_mut().expect("init checked above");
        // SAFETY: table_map is sized to TEX_TABLE_SIZE >= capacity * 32,
        // and id < capacity (checked above).
        unsafe {
            let dst = table_map.as_mut_ptr().add(table_offset);
            volatile_copy_to_devmem(dst, descriptor_bytes);
        }

        self.next_tex_id = id.wrapping_add(1);

        Ok(TextureHandle {
            id,
            width: spec.width,
            height: spec.height,
            format: spec.format,
            phys_addr: phys,
        })
    }

    fn init_texture_storage(&mut self) -> Result<(), DeviceError> {
        let pool_phys = self.cfg.base_phys_addr + mem::TEX_POOL_OFFSET as u32;
        let table_phys = self.cfg.base_phys_addr + mem::TEX_TABLE_OFFSET as u32;

        let pool = DevMemMap::create(pool_phys, mem::TEX_POOL_SIZE)?;
        let table = DevMemMap::create(table_phys, mem::TEX_TABLE_SIZE)?;

        self.tex_pool_map = Some(pool);
        self.tex_table_map = Some(table);

        // Tell the FPGA where the descriptor table lives.
        self.regs.write32(registers::TEX_TABLE_ADDR, table_phys);
        self.regs.write32(
            registers::TEX_TABLE_COUNT,
            self.tex_table_capacity as u32,
        );
        Ok(())
    }

    /// Reset the texture pool: drop all uploaded handles, return the
    /// allocator to empty, reuse descriptor slot 0 next. Existing
    /// [`TextureHandle`]s become invalid — calling `copy_rect` with a
    /// stale handle is a logic bug (the FPGA may render garbage). Use
    /// only at well-defined transition points (e.g. menu reload).
    pub fn reset_textures(&mut self) {
        self.tex_alloc.reset();
        self.next_tex_id = 0;
    }

    /// Allocate an RGBA8888 render-target texture with the given
    /// dimensions. Pixel data is left undefined (the caller must fully
    /// paint the target before sampling from it). The returned
    /// [`TextureHandle`] can be passed to `Frame::set_target` and to
    /// `Frame::copy_rect`.
    pub fn create_render_target(
        &mut self,
        width: u16,
        height: u16,
    ) -> Result<TextureHandle, DeviceError> {
        if self.tex_pool_map.is_none() {
            self.init_texture_storage()?;
        }
        if width == 0 || height == 0 {
            return Err(DeviceError::TextureStrideTooSmall {
                stride: 0,
                row_bytes: 0,
            });
        }
        if self.next_tex_id >= self.tex_table_capacity {
            return Err(DeviceError::DescriptorTableFull {
                capacity: self.tex_table_capacity as u32,
            });
        }

        // Round the row stride up to TEX_BURST_ALIGN bytes so every row
        // starts on a 64-byte (= 16 RGBA pixel) boundary. The blit
        // engine's burst-COPY path picks the largest aligned burst at
        // each cur_x — with both base and pitch 64-aligned, the start
        // of every row qualifies for the maximum (16-px) tier.
        let row_bytes: u32 = (width as u32) * 4;
        let stride: u32 = align_up(row_bytes, TEX_BURST_ALIGN);
        let needed_bytes: u32 = stride.saturating_mul(height as u32);
        let phys = self
            .tex_alloc
            .alloc(needed_bytes, TEX_BURST_ALIGN)
            .map_err(|e| match e {
                AllocError::OutOfMemory {
                    requested,
                    remaining,
                } => DeviceError::TexturePoolExhausted {
                    needed: requested,
                    free: remaining,
                },
                AllocError::BadAlignment(_) => unreachable!("64 is power of two"),
            })?;

        // Write the descriptor (data isn't initialized — the FPGA only
        // reads the texture after the host has painted into it).
        let descriptor = TextureDescriptor::new(
            phys,
            stride,
            width,
            height,
            crate::protocol::TextureFormat::Rgba8888,
        );
        // SAFETY: TextureDescriptor is repr(C) 32-byte, no padding.
        let descriptor_bytes = unsafe {
            core::slice::from_raw_parts(
                (&descriptor as *const TextureDescriptor) as *const u8,
                DESCRIPTOR_SIZE,
            )
        };
        let id = self.next_tex_id;
        let table_offset = (id as usize) * DESCRIPTOR_SIZE;
        let table_map = self.tex_table_map.as_mut().expect("init checked above");
        // SAFETY: table_map sized to capacity*32 ≥ id*32+32.
        unsafe {
            let dst = table_map.as_mut_ptr().add(table_offset);
            volatile_copy_to_devmem(dst, descriptor_bytes);
        }
        self.next_tex_id = id.wrapping_add(1);

        Ok(TextureHandle {
            id,
            width,
            height,
            format: crate::protocol::TextureFormat::Rgba8888,
            phys_addr: phys,
        })
    }

    /// Capacity (in slots) of the texture descriptor table.
    #[inline]
    pub fn texture_capacity(&self) -> u16 {
        self.tex_table_capacity
    }

    /// Number of texture slots used so far.
    #[inline]
    pub fn texture_count(&self) -> u16 {
        self.next_tex_id
    }

    /// Bytes still free in the texture pool.
    #[inline]
    pub fn texture_pool_free(&self) -> u32 {
        self.tex_alloc.remaining()
    }

    /// Set `CONTROL.ENABLE`. Idempotent.
    pub fn start(&mut self) -> Result<(), DeviceError> {
        if !self.started {
            self.regs
                .write32(registers::CONTROL, registers::CONTROL_ENABLE);
            self.started = true;
        }
        Ok(())
    }

    /// Clear `CONTROL.ENABLE`. Idempotent.
    pub fn stop(&mut self) -> Result<(), DeviceError> {
        if self.started {
            self.regs.write32(registers::CONTROL, 0);
            self.started = false;
        }
        Ok(())
    }

    /// Read `FRAME_COUNT` directly. The framework increments this on
    /// every successful display swap (PROTOCOL.md §3.2).
    #[inline]
    pub fn frame_count(&self) -> u32 {
        self.regs.read32(registers::FRAME_COUNT)
    }

    /// Decoded snapshot of `FB_STATE`.
    #[inline]
    pub fn fb_state(&self) -> FbState {
        let s = self.regs.read32(registers::FB_STATE);
        FbState {
            display: registers::fb_state_display(s),
            render: registers::fb_state_render(s),
            ready: registers::fb_state_ready(s),
        }
    }

    /// If `STATUS.ER` is set, decode `ERROR_INFO` and return it; else
    /// `None`.
    pub fn last_error(&self) -> Option<HardwareError> {
        let status = self.regs.read32(registers::STATUS);
        if status & registers::STATUS_ERROR == 0 {
            return None;
        }
        HardwareError::from_register(self.regs.read32(registers::ERROR_INFO))
    }

    /// Spin-wait until `FENCE_VALUE` reaches `target`, or the device
    /// reports an error, or `timeout` elapses.
    pub fn wait_fence(&self, target: u32, timeout: Duration) -> Result<(), DeviceError> {
        let start = Instant::now();
        loop {
            if self.regs.read32(registers::FENCE_VALUE) == target {
                return Ok(());
            }
            if let Some(err) = self.last_error() {
                return Err(DeviceError::Hardware {
                    kind: err.kind,
                    info: err.info,
                });
            }
            if start.elapsed() >= timeout {
                return Err(DeviceError::Timeout {
                    what: "FENCE_VALUE",
                    waited: start.elapsed(),
                });
            }
        }
    }

    /// Spin-wait until `FRAME_COUNT >= target`. Returns the observed
    /// value on success. Sub-microsecond polling — callers should
    /// only use this where a brief blocking wait is acceptable.
    pub fn wait_frame_count(&self, target: u32, timeout: Duration) -> Result<u32, DeviceError> {
        let start = Instant::now();
        loop {
            let v = self.frame_count();
            if v >= target {
                return Ok(v);
            }
            if let Some(err) = self.last_error() {
                return Err(DeviceError::Hardware {
                    kind: err.kind,
                    info: err.info,
                });
            }
            if start.elapsed() >= timeout {
                return Err(DeviceError::Timeout {
                    what: "FRAME_COUNT",
                    waited: start.elapsed(),
                });
            }
        }
    }

    /// Direct, low-level access to the register block. Most users
    /// should prefer the higher-level methods; this is exposed for
    /// diagnostics and pattern tests that intentionally poke at every
    /// slot.
    #[inline]
    pub fn register_block(&self) -> registers::RegisterBlock {
        self.regs
    }

    // === crate-internal accessors used by Frame ===

    #[inline]
    pub(crate) fn regs(&self) -> registers::RegisterBlock {
        self.regs
    }

    #[inline]
    pub(crate) fn ring_devmem_ptr(&mut self) -> *mut u8 {
        self.ring_map.as_mut_ptr()
    }

    #[inline]
    pub(crate) fn ring_size_bytes(&self) -> usize {
        self.cfg.ring_size
    }

    #[inline]
    pub(crate) fn ring_tail(&self) -> u32 {
        self.ring_tail
    }

    #[inline]
    pub(crate) fn read_ring_head(&self) -> u32 {
        self.regs.read32(registers::RING_HEAD)
    }

    #[inline]
    pub(crate) fn set_ring_tail(&mut self, new_tail: u32) {
        self.ring_tail = new_tail;
    }

    /// Allocate a non-zero fence value, wrapping past 0.
    pub(crate) fn allocate_fence(&mut self) -> u32 {
        let v = self.next_fence;
        let next = v.wrapping_add(1);
        self.next_fence = if next == 0 { 1 } else { next };
        v
    }
}

// `regs_map` and `ring_map` are kept alive purely for the validity of
// the raw pointers we hand to `RegisterBlock` and the ring writer. The
// fields look unused from the outside, so silence any linter that may
// otherwise flag them.
#[allow(dead_code)]
fn _device_invariants(d: &Device) {
    let _ = (&d.regs_map, &d.ring_map);
}
