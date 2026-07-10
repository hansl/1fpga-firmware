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

use std::sync::atomic::{Ordering, fence};
use std::time::{Duration, Instant};

use crate::allocator::{AllocError, BumpAllocator};
use crate::bridge;
use crate::devmem::{DevMemMap, volatile_copy_to_devmem, volatile_copy_within_devmem};
use crate::error::{DeviceError, HardwareError};
use crate::mem;
use crate::protocol::descriptors::{DESCRIPTOR_SIZE, TextureDescriptor, TextureFormat};
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

/// Physical page holding the HPS SDRAM controller scheduler registers
/// (Cyclone V HPS TRM ch. 12, CTRLGRP block at 0xFFC20000; the
/// scheduler cluster starts at +0x5000).
const SDR_SCHED_PHYS: u32 = 0xFFC2_5000;

/// `mppriority` offset within that page. 30-bit `userpriority` field:
/// 3-bit absolute priority per MPFE command port, ports 0-9 packed
/// LSB-first. 0 = lowest, 7 = highest; equal priorities fall back to
/// weighted round-robin.
const SDR_MPPRIORITY_OFF: usize = 0xAC;

/// Priority word that lifts the HDMI scanout above everything else.
///
/// Command-port mapping (Cyclone V HPS TRM table 12-4; matches the
/// RocketBoards SDRAM performance design): ports 0/1 = f2h_sdram0
/// read/write (sys_top `ram1`), 2/3 = f2h_sdram1 (`ram2`), 4/5 =
/// f2h_sdram2 (`vbuf` — the HDMI scanout's DDR3 reads), 6-9 = L3/MPU.
/// u-boot leaves the whole register at 0, i.e. round-robin.
///
/// The menu core's bulk masters (blit engines, ring fetcher) put
/// sustained multi-burst traffic on ram1/ram2; under round-robin
/// arbitration they starve the scanout's real-time reads and the
/// display re-emits its last 128-byte burst — full-width bands of
/// 32-pixel-period vertical stripes across whatever rows were being
/// scanned during the stall (diagnosed 2026-07-09 on the
/// compositor-v2 branch; the mechanism applies to any design whose
/// scanout reads DDR3). Highest priority for the scanout port fixes
/// it; bulk masters absorb the latency.
const MPPRIORITY_VBUF_REALTIME: u32 = (7 << (3 * 4)) | (7 << (3 * 5));

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
    /// Mapping covering both layer tables (A and B). The compositor
    /// scanout (Phase 2+) reads whichever is active per the
    /// `LAYER_ACTIVE` register; in Phase 0 this just provides write
    /// targets for the host.
    layer_table_map: Option<DevMemMap>,
    /// Which layer table the host is currently writing to (0 = A,
    /// 1 = B). Toggled by [`Self::commit_layers`].
    layer_back_idx: u8,
    /// Highest-written-slot+1 in the current back table — what gets
    /// shipped to the FPGA in `LAYER_COMMIT.count` on the next
    /// commit. Reset to 0 after each commit (immediate-mode in
    /// Phase 2a; retained-mode would require copying the active
    /// table into the back).
    back_valid_count: u16,
    tex_alloc: BumpAllocator,
    next_tex_id: u16,
    tex_table_capacity: u16,
    /// Ping-pong physical addresses of the two content-coverage-mask
    /// buffers, allocated lazily from the texture pool on first upload.
    /// Double-buffered so the host never overwrites the mask the
    /// compositor is currently latched on. `mask_idx` selects the next
    /// buffer to write.
    mask_bufs: Option<[u32; 2]>,
    mask_idx: usize,
    /// Physical address of the boxart overlay FB, allocated lazily from the
    /// texture pool on first `upload_boxart` and reused (content updates on
    /// selection; position animates via registers).
    boxart_fb: Option<u32>,
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

        // Make the HDMI scanout the highest-priority SDRAM port before
        // we start generating bulk DDR3 traffic — see
        // MPPRIORITY_VBUF_REALTIME for the full story.
        {
            let mut sched_map = DevMemMap::create(SDR_SCHED_PHYS, 0x100)?;
            // SAFETY: SDR_MPPRIORITY_OFF (0xAC) lies within the mapped
            // 0x100-byte region; volatile single-word store as required
            // for device memory.
            unsafe {
                let p = sched_map
                    .as_mut_ptr()
                    .add(SDR_MPPRIORITY_OFF)
                    .cast::<u32>();
                p.write_volatile(MPPRIORITY_VBUF_REALTIME);
            }
        }

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
            layer_table_map: None,
            layer_back_idx: 1, // active starts at A (0); host writes B first.
            back_valid_count: 0,
            tex_alloc: BumpAllocator::new(tex_pool_phys, mem::TEX_POOL_SIZE as u32),
            next_tex_id: 0,
            tex_table_capacity: mem::DEFAULT_TEX_TABLE_COUNT as u16,
            mask_bufs: None,
            mask_idx: 0,
            boxart_fb: None,
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

        let id = self.write_descriptor(phys, spec.stride, spec.width, spec.height, spec.format);

        Ok(TextureHandle {
            id,
            width: spec.width,
            height: spec.height,
            format: spec.format,
            phys_addr: phys,
        })
    }

    /// Write the next descriptor-table entry. Caller must have checked
    /// `next_tex_id < tex_table_capacity` and mapped the table.
    fn write_descriptor(
        &mut self,
        phys: u32,
        stride: u32,
        width: u16,
        height: u16,
        format: TextureFormat,
    ) -> u16 {
        let descriptor = TextureDescriptor::new(phys, stride, width, height, format);
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
        let table_map = self.tex_table_map.as_mut().expect("table mapped by caller");
        // SAFETY: table_map is sized to TEX_TABLE_SIZE >= capacity * 32,
        // and id < capacity (checked by caller).
        unsafe {
            let dst = table_map.as_mut_ptr().add(table_offset);
            volatile_copy_to_devmem(dst, descriptor_bytes);
        }
        self.next_tex_id = id.wrapping_add(1);
        id
    }

    /// Allocate an UNINITIALISED blitter-renderable RGBA8888 surface in
    /// the texture pool (tightly packed, `w*4` bytes/row). Contents are
    /// undefined until the blit engine writes them (`SetRenderTarget` +
    /// fills/copies). This is the backing store for overlay planes and
    /// any other render-to-texture use; there is no pixel upload.
    pub fn create_render_texture(&mut self, w: u16, h: u16) -> Result<TextureHandle, DeviceError> {
        if self.tex_pool_map.is_none() {
            self.init_texture_storage()?;
        }
        if self.next_tex_id >= self.tex_table_capacity {
            return Err(DeviceError::DescriptorTableFull {
                capacity: self.tex_table_capacity as u32,
            });
        }
        let stride = (w as u32) * 4;
        let needed = stride * (h as u32);
        let phys = self
            .tex_alloc
            .alloc(needed, TEX_BURST_ALIGN)
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
        let id = self.write_descriptor(phys, stride, w, h, TextureFormat::Rgba8888);
        Ok(TextureHandle {
            id,
            width: w,
            height: h,
            format: TextureFormat::Rgba8888,
            phys_addr: phys,
        })
    }

    /// Upload a wallpaper image into DDR and point the scanout
    /// compositor's opaque wallpaper layer at it (Phase B). `pixels` is
    /// BGRA8888 packed at `stride` bytes/row for `height` rows; `stride`
    /// MUST equal the content framebuffer's `FB_STRIDE` (the compositor
    /// shares a single stride across both layers). The buffer is
    /// allocated once from the texture pool and persists for the
    /// device's lifetime. Returns its physical address. Pair with
    /// [`Self::set_composite`] to turn the blend on.
    pub fn upload_wallpaper(
        &mut self,
        pixels: &[u8],
        stride: u32,
        height: u16,
    ) -> Result<u32, DeviceError> {
        let needed = (stride as usize) * (height as usize);
        if pixels.len() < needed {
            return Err(DeviceError::TextureDataTruncated {
                expected: needed,
                got: pixels.len(),
            });
        }
        if self.tex_pool_map.is_none() {
            self.init_texture_storage()?;
        }
        let phys = self
            .tex_alloc
            .alloc(needed as u32, TEX_BURST_ALIGN)
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
        let pool_base_phys = self.cfg.base_phys_addr + mem::TEX_POOL_OFFSET as u32;
        let offset_in_pool = (phys - pool_base_phys) as usize;
        let pool_map = self.tex_pool_map.as_mut().expect("init checked above");
        // SAFETY: bump allocator keeps `phys + needed <= pool end`, so the
        // dst window is in bounds; volatile byte copy into /dev/mem.
        unsafe {
            let dst = pool_map.as_mut_ptr().add(offset_in_pool);
            volatile_copy_to_devmem(dst, &pixels[..needed]);
        }
        self.regs.write32(registers::WALLPAPER_ADDR, phys);
        Ok(phys)
    }

    /// Enable or disable the scanout compositor's content-over-wallpaper
    /// blend (`CONTROL[8]`, Phase B). Read-modify-write so `CONTROL.ENABLE`
    /// is preserved — call after [`Self::start`].
    pub fn set_composite(&mut self, on: bool) {
        let cur = self.regs.read32(registers::CONTROL);
        let next = if on {
            cur | registers::CONTROL_COMPOSITE
        } else {
            cur & !registers::CONTROL_COMPOSITE
        };
        self.regs.write32(registers::CONTROL, next);
    }

    fn alloc_mask_buf(&mut self) -> Result<u32, DeviceError> {
        self.tex_alloc
            .alloc(mem::MASK_BYTES as u32, TEX_BURST_ALIGN)
            .map_err(|e| match e {
                AllocError::OutOfMemory {
                    requested,
                    remaining,
                } => DeviceError::TexturePoolExhausted {
                    needed: requested,
                    free: remaining,
                },
                AllocError::BadAlignment(_) => unreachable!("64 is power of two"),
            })
    }

    /// Upload the content coverage mask (task #15) and point the compositor
    /// at it. `words` holds one u32 per tile row (low 30 bits = tile-x
    /// coverage); at most `mem::MASK_ROWS` are used, the rest zero-padded.
    /// Double-buffered: writes the alternate buffer then updates
    /// `CONTENT_MASK_ADDR`, so the compositor (which latches the address at
    /// frame start) reads a complete mask. Pair with
    /// [`Self::set_content_mask`] to enable.
    pub fn upload_content_mask(&mut self, words: &[u32]) -> Result<(), DeviceError> {
        if self.tex_pool_map.is_none() {
            self.init_texture_storage()?;
        }
        let bufs = match self.mask_bufs {
            Some(b) => b,
            None => {
                let pair = [self.alloc_mask_buf()?, self.alloc_mask_buf()?];
                self.mask_bufs = Some(pair);
                pair
            }
        };
        let idx = self.mask_idx & 1;
        let phys = bufs[idx];
        let pool_base_phys = self.cfg.base_phys_addr + mem::TEX_POOL_OFFSET as u32;
        let offset_in_pool = (phys - pool_base_phys) as usize;

        let mut bytes = [0u8; mem::MASK_BYTES];
        let n = words.len().min(mem::MASK_ROWS);
        for (i, w) in words[..n].iter().enumerate() {
            bytes[i * 4..i * 4 + 4].copy_from_slice(&w.to_le_bytes());
        }
        let pool_map = self.tex_pool_map.as_mut().expect("init checked above");
        // SAFETY: the buffer is MASK_BYTES inside the pool (bump-allocated),
        // so the dst window is in bounds; volatile byte copy into /dev/mem.
        unsafe {
            let dst = pool_map.as_mut_ptr().add(offset_in_pool);
            volatile_copy_to_devmem(dst, &bytes);
        }
        self.regs.write32(registers::CONTENT_MASK_ADDR, phys);
        self.mask_idx ^= 1;
        Ok(())
    }

    /// Enable or disable the content coverage mask (`CONTROL[9]`, task #15).
    /// Upload at least one mask via [`Self::upload_content_mask`] before
    /// enabling. Read-modify-write so other CONTROL bits are preserved.
    pub fn set_content_mask(&mut self, on: bool) {
        let cur = self.regs.read32(registers::CONTROL);
        let next = if on {
            cur | registers::CONTROL_CONTENT_MASK
        } else {
            cur & !registers::CONTROL_CONTENT_MASK
        };
        self.regs.write32(registers::CONTROL, next);
    }

    /// Upload boxart panel pixels (BGRA8888 premultiplied, tightly packed at
    /// `w*4` bytes/row) to the overlay FB and program its size + stride
    /// (Phase D). The FB is allocated once from the texture pool and reused;
    /// re-upload only when the art changes. Position is set separately via
    /// [`Self::set_boxart_pos`] (cheap, per frame for animation); enable via
    /// [`Self::set_boxart`].
    pub fn upload_boxart(&mut self, pixels: &[u8], w: u16, h: u16) -> Result<(), DeviceError> {
        if (w as usize) > mem::BOXART_MAX_W || (h as usize) > mem::BOXART_MAX_H {
            return Err(DeviceError::BoxartTooLarge {
                width: w,
                height: h,
                max: mem::BOXART_MAX_W as u16,
            });
        }
        let needed = (w as usize) * (h as usize) * 4;
        if pixels.len() < needed {
            return Err(DeviceError::TextureDataTruncated {
                expected: needed,
                got: pixels.len(),
            });
        }
        if self.tex_pool_map.is_none() {
            self.init_texture_storage()?;
        }
        let phys = match self.boxart_fb {
            Some(p) => p,
            None => {
                let p = self
                    .tex_alloc
                    .alloc(mem::BOXART_FB_BYTES as u32, TEX_BURST_ALIGN)
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
                self.boxart_fb = Some(p);
                p
            }
        };
        let pool_base_phys = self.cfg.base_phys_addr + mem::TEX_POOL_OFFSET as u32;
        let offset_in_pool = (phys - pool_base_phys) as usize;
        let pool_map = self.tex_pool_map.as_mut().expect("init checked above");
        // SAFETY: the boxart FB is BOXART_FB_BYTES inside the pool and
        // `needed <= BOXART_FB_BYTES` (size checked above).
        unsafe {
            let dst = pool_map.as_mut_ptr().add(offset_in_pool);
            volatile_copy_to_devmem(dst, &pixels[..needed]);
        }
        self.regs.write32(registers::BOXART_BASE, phys);
        self.regs
            .write32(registers::BOXART_SIZE, ((h as u32) << 16) | (w as u32));
        self.regs.write32(registers::BOXART_STRIDE, (w as u32) * 4);
        Ok(())
    }

    /// Set the boxart panel's top-left position (signed; may be off-screen
    /// for slide animations). Cheap single register write — call per frame
    /// to animate without touching the blit engine.
    pub fn set_boxart_pos(&mut self, x: i16, y: i16) {
        let packed = (((y as u16) as u32) << 16) | ((x as u16) as u32);
        self.regs.write32(registers::BOXART_POS, packed);
    }

    /// Enable or disable the boxart overlay layer (`CONTROL[10]`, Phase D).
    /// Upload art + set position first. Read-modify-write so other CONTROL
    /// bits are preserved.
    pub fn set_boxart(&mut self, on: bool) {
        let cur = self.regs.read32(registers::CONTROL);
        let next = if on {
            cur | registers::CONTROL_BOXART
        } else {
            cur & !registers::CONTROL_BOXART
        };
        self.regs.write32(registers::CONTROL, next);
    }

    // ---- Overlay plane (generalised boxart layer) --------------------
    //
    // The scanout compositor's third layer, driven from any
    // blitter-renderable texture: the blit engine composes the plane's
    // content into the texture (SetRenderTarget), the scanout blends it
    // over wallpaper+content at its programmed position, and MOVING the
    // plane is a single register write — no pixel traffic. The RTL
    // fetches only the on-screen window of each row, so the plane may
    // be WIDER than the screen (e.g. a carousel strip that slides by
    // position writes alone).

    /// RTL cap: plane width register is 12 bits.
    pub const PLANE_MAX_W: u16 = 4095;
    /// RTL cap: the plane row index is 9 bits.
    pub const PLANE_MAX_H: u16 = 512;

    /// Point the scanout overlay plane at `tex` (typically from
    /// [`Self::create_render_texture`]) and program its geometry.
    /// Position via [`Self::set_plane_pos`]; enable via
    /// [`Self::set_plane_enabled`]. All three are frame-latched by the
    /// compositor, so reprogramming mid-frame is safe.
    pub fn set_plane_surface(&mut self, tex: &TextureHandle) -> Result<(), DeviceError> {
        if tex.width > Self::PLANE_MAX_W || tex.height > Self::PLANE_MAX_H {
            return Err(DeviceError::BoxartTooLarge {
                width: tex.width,
                height: tex.height,
                max: Self::PLANE_MAX_W,
            });
        }
        self.regs.write32(registers::BOXART_BASE, tex.phys_addr);
        self.regs.write32(
            registers::BOXART_SIZE,
            ((tex.height as u32) << 16) | (tex.width as u32),
        );
        self.regs
            .write32(registers::BOXART_STRIDE, (tex.width as u32) * 4);
        Ok(())
    }

    /// Move the overlay plane (signed screen coordinates; partial or
    /// full off-screen in any direction is fine). One register write —
    /// the per-frame animation path.
    #[inline]
    pub fn set_plane_pos(&mut self, x: i16, y: i16) {
        self.set_boxart_pos(x, y);
    }

    /// Show or hide the overlay plane.
    #[inline]
    pub fn set_plane_enabled(&mut self, on: bool) {
        self.set_boxart(on);
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

    fn init_layer_storage(&mut self) -> Result<(), DeviceError> {
        if self.layer_table_map.is_some() {
            return Ok(());
        }
        let phys = self.cfg.base_phys_addr + mem::LAYER_TABLE_OFFSET as u32;
        let map = DevMemMap::create(phys, mem::LAYER_REGION_SIZE)?;
        self.layer_table_map = Some(map);
        // Zero-initialise both tables so any future scanout sees only
        // disabled (flags == 0) descriptors until the host populates
        // real layers.
        let zero = [0u8; mem::LAYER_DESCRIPTOR_SIZE];
        let map = self
            .layer_table_map
            .as_mut()
            .expect("just inserted");
        // SAFETY: map covers `LAYER_REGION_SIZE` bytes; we zero each
        // 32-byte descriptor in both tables in turn.
        unsafe {
            let base = map.as_mut_ptr();
            for i in 0..(2 * mem::LAYERS_PER_TABLE as usize) {
                let dst = base.add(i * mem::LAYER_DESCRIPTOR_SIZE);
                volatile_copy_to_devmem(dst, &zero);
            }
        }
        // Tell the FPGA where the region lives. LAYER_COMMIT stays at
        // its reset value (active=A, count=0) until the first commit.
        self.regs.write32(registers::LAYER_TABLE_BASE, phys);
        Ok(())
    }

    /// Write `desc` into the **back** layer table at `slot`. Has no
    /// visible effect until [`Self::commit_layers`] makes the back
    /// table active. Returns `Err` if `slot >= LAYERS_PER_TABLE`.
    pub fn set_layer(
        &mut self,
        slot: u32,
        desc: &protocol::LayerDescriptor,
    ) -> Result<(), DeviceError> {
        if slot >= mem::LAYERS_PER_TABLE {
            return Err(DeviceError::LayerSlotOutOfRange {
                slot,
                capacity: mem::LAYERS_PER_TABLE,
            });
        }
        if self.layer_table_map.is_none() {
            self.init_layer_storage()?;
        }
        let back_offset = if self.layer_back_idx == 0 {
            0
        } else {
            mem::LAYER_TABLE_SIZE
        };
        let entry_offset =
            back_offset + (slot as usize) * mem::LAYER_DESCRIPTOR_SIZE;
        // SAFETY: layer_table is repr(C), 32 bytes, no padding holes
        // in the public layout (asserted at compile time).
        let bytes = unsafe {
            core::slice::from_raw_parts(
                (desc as *const protocol::LayerDescriptor) as *const u8,
                mem::LAYER_DESCRIPTOR_SIZE,
            )
        };
        let map = self
            .layer_table_map
            .as_mut()
            .expect("init_layer_storage just ran");
        // SAFETY: layer_table_map covers LAYER_REGION_SIZE bytes;
        // entry_offset+32 stays within (slot < LAYERS_PER_TABLE,
        // table_size = LAYERS_PER_TABLE * 32, two tables fit inside
        // LAYER_REGION_SIZE).
        unsafe {
            let dst = map.as_mut_ptr().add(entry_offset);
            volatile_copy_to_devmem(dst, bytes);
        }
        let want_count = (slot as u16).saturating_add(1);
        if want_count > self.back_valid_count {
            self.back_valid_count = want_count;
        }
        Ok(())
    }

    /// Disable the layer at `slot` in the back table — equivalent to
    /// `set_layer` with a zeroed descriptor. The compositor will skip
    /// the slot once the change is committed.
    pub fn clear_layer(&mut self, slot: u32) -> Result<(), DeviceError> {
        let zero = protocol::LayerDescriptor::default();
        self.set_layer(slot, &zero)
    }

    /// Promote the back layer table to active, atomically swapping
    /// what the compositor reads. The next scanline observes the
    /// just-committed layout.
    ///
    /// Writes [`registers::LAYER_COMMIT`] in a single 32-bit store
    /// (bit 31 = active table, bits 8..0 = valid layer count). The
    /// FPGA latches both fields on the same clock edge, so a scanline
    /// in flight cannot observe a torn commit (PROTOCOL.md §11.2).
    ///
    /// **Retained-mode contract**: after the swap, this mirrors the
    /// just-committed (now-active) table into the new back table so
    /// the next [`set_layer`] sees the previous frame's state and
    /// only needs to write the slots that actually change. Callers
    /// who want to clear all layers should call [`Self::clear_layers`]
    /// instead — issuing `commit_layers` with no intervening
    /// `set_layer` calls re-commits the same state.
    ///
    /// The mirror copies `count` descriptors (where `count` is the
    /// committed valid-layer count). It reads from the new active
    /// table — which the FPGA also reads, but concurrent readers do
    /// not conflict — and writes to the new back table. If a prior
    /// frame's `layer_dma` is still draining when this is called,
    /// it is reading the now-new-back; the writes here could
    /// theoretically race that read for a single descriptor on a
    /// single frame. Even-rate UIs (≤30 Hz) leave more than enough
    /// idle time between vsync and the next commit for this to be a
    /// non-issue in practice.
    pub fn commit_layers(&mut self) {
        // Pair with the volatile descriptor writes above: the DDR3
        // stores must be globally visible before the FPGA observes
        // the new LAYER_COMMIT and starts walking the freshly-active
        // table. Same pattern Frame::submit uses around RING_TAIL.
        fence(Ordering::Release);

        // The table the host has been writing to is what the FPGA
        // will now read. `layer_back_idx` will flip to point at the
        // OTHER table for the next frame.
        let new_active: u32 = self.layer_back_idx as u32;
        let count: u32 = (self.back_valid_count as u32) & 0x1FF;
        let commit: u32 = (new_active << 31) | count;
        self.regs.write32(registers::LAYER_COMMIT, commit);

        self.layer_back_idx ^= 1;
        self.mirror_active_to_back(count as u16);
    }

    /// Atomically clear all layers (count = 0). The previously-active
    /// table is gated out by the count field; layer_dma stays in
    /// S_IDLE on the next vsync. Resets the host's retained state so
    /// the next [`set_layer`] starts from a blank table.
    pub fn clear_layers(&mut self) {
        fence(Ordering::Release);
        let new_active: u32 = self.layer_back_idx as u32;
        let commit: u32 = new_active << 31; // count = 0
        self.regs.write32(registers::LAYER_COMMIT, commit);
        self.layer_back_idx ^= 1;
        self.back_valid_count = 0;
    }

    /// Copy the first `count` descriptors from the new active table
    /// into the new back table so the next frame's `set_layer` calls
    /// see the just-committed state. No-op when `count == 0` or
    /// when the layer storage hasn't been mapped yet (e.g. caller
    /// committed without ever writing a layer).
    fn mirror_active_to_back(&mut self, count: u16) {
        if count == 0 {
            self.back_valid_count = 0;
            return;
        }
        let Some(map) = self.layer_table_map.as_mut() else {
            self.back_valid_count = 0;
            return;
        };
        // After the flip, `layer_back_idx` points at the new back. The
        // new active sits in the opposite half of the layer region.
        let (active_off, back_off) = if self.layer_back_idx == 0 {
            (mem::LAYER_TABLE_SIZE, 0)
        } else {
            (0, mem::LAYER_TABLE_SIZE)
        };
        let bytes = (count as usize) * mem::LAYER_DESCRIPTOR_SIZE;
        // SAFETY: layer_table_map covers LAYER_REGION_SIZE bytes; each
        // table is LAYER_TABLE_SIZE bytes, and `count <=
        // LAYERS_PER_TABLE` is enforced by set_layer's bounds check
        // bumping back_valid_count. Both offsets are 32-byte aligned
        // (a descriptor boundary), so the 4-byte alignment requirement
        // of `volatile_copy_within_devmem` is satisfied.
        unsafe {
            let base = map.as_mut_ptr();
            volatile_copy_within_devmem(base.add(active_off), base.add(back_off), bytes);
        }
        self.back_valid_count = count;
    }

    /// Diagnostic: which layer table the host will write to next
    /// (0 = A, 1 = B). The other one is the active (compositor-read)
    /// table.
    #[inline]
    pub fn layer_back_idx(&self) -> u8 {
        self.layer_back_idx
    }

    /// Diagnostic: free-running count of layer descriptors the on-FPGA
    /// DMA has shipped into the cache (PROTOCOL.md §3.1 LAYER_DEBUG).
    /// Should advance by `count` every frame once `commit_layers` has
    /// been called and the compositor is running. A static value means
    /// the DMA isn't ticking — most often because LAYER_COMMIT.count
    /// is still 0 or because the FPGA is in error.
    #[inline]
    pub fn layer_dma_descriptors(&self) -> u32 {
        self.regs.read32(registers::LAYER_DEBUG)
    }

    /// Reset the texture pool: drop all uploaded handles, return the
    /// allocator to empty, reuse descriptor slot 0 next. Existing
    /// [`TextureHandle`]s become invalid — calling `copy_rect` with a
    /// stale handle is a logic bug (the FPGA may render garbage). Use
    /// only at well-defined transition points (e.g. menu reload).
    pub fn reset_textures(&mut self) {
        self.tex_alloc.reset();
        self.next_tex_id = 0;
        // The mask + boxart buffers came from this pool; force re-alloc.
        self.mask_bufs = None;
        self.boxart_fb = None;
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

    /// Non-blocking check: has fence `target` retired? Same wrap-safe
    /// "at or past" comparison as [`Self::wait_fence`], single register
    /// read. Used to opportunistically drain the pipelined-fence queue
    /// so `FB_STATE.render` can be trusted without blocking.
    pub fn fence_reached(&self, target: u32) -> bool {
        let current = self.regs.read32(registers::FENCE_VALUE);
        (current.wrapping_sub(target) as i32) >= 0
    }

    /// Spin-wait until `FENCE_VALUE` reaches `target`, or the device
    /// reports an error, or `timeout` elapses.
    pub fn wait_fence(&self, target: u32, timeout: Duration) -> Result<(), DeviceError> {
        let start = Instant::now();
        loop {
            // FENCE_VALUE holds the MOST RECENTLY RETIRED fence value.
            // With pipelined submit, by the time we poll the FPGA may
            // have retired our target AND several after it — the
            // value has moved past `target`. Check "at or past" via
            // a signed difference so wrap-around is handled correctly
            // (works as long as we never have more than 2^31 fences
            // in flight, which we never will).
            let current = self.regs.read32(registers::FENCE_VALUE);
            if (current.wrapping_sub(target) as i32) >= 0 {
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
