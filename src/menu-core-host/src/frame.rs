//! Per-frame command builder + fence token.
//!
//! [`Frame`] is acquired via [`Device::begin_frame`], populated with
//! draw calls, then handed off to [`Frame::submit`] which advances
//! `RING_TAIL`, kicks the engine, and returns a [`FenceToken`] for
//! synchronisation.
//!
//! Commands stream **directly** into the device's ring buffer in DDR3
//! as they're appended — there is no staging buffer or bulk copy at
//! submit time. Each `push_cmd` call encodes the command into a small
//! stack-allocated scratch and `volatile_copy_to_devmem`s it into the
//! ring at the host-tracked tail offset.
//!
//! [`Device::begin_frame`]: crate::device::Device::begin_frame

use std::sync::atomic::{Ordering, fence};
use std::time::{Duration, Instant};

use crate::device::Device;
use crate::devmem::volatile_copy_to_devmem;
use crate::error::DeviceError;
use crate::protocol::commands::TARGET_FRAMEBUFFER;
use crate::protocol::{BlendMode, Command, Filter, Rect, Rgba, registers};
use crate::ring::RingError;
use crate::texture::TextureHandle;

/// Optional parameters for [`Frame::copy_rect`].
#[derive(Debug, Copy, Clone)]
pub struct CopyOpts {
    pub blend: BlendMode,
    pub filter: Filter,
    /// When `Some`, the texture is multiplied by this color before
    /// blending (PROTOCOL.md §7.5).
    pub tint: Option<Rgba>,
}

impl Default for CopyOpts {
    fn default() -> Self {
        Self {
            blend: BlendMode::Opaque,
            filter: Filter::Nearest,
            tint: None,
        }
    }
}

/// Per-frame command-buffer builder.
///
/// All `fill_rect` / `copy_rect` / etc. methods return `&mut Self` so
/// they can be chained:
///
/// ```ignore
/// device
///     .begin_frame()
///     .fill_rect(bg, Rgba::BLACK, BlendMode::Opaque)?
///     .fill_rect(fg, Rgba::WHITE, BlendMode::SrcAlpha)?
///     .present()?
///     .submit()?
///     .wait_presented(Duration::from_secs(1))?;
/// ```
pub struct Frame<'a> {
    device: &'a mut Device,
    /// Ring size in bytes (power of two; cached at frame start).
    ring_size: u32,
    /// `ring_size - 1` for cheap modulo via `& mask`.
    ring_mask: u32,
    /// Host-tracked write offset into the ring. Advanced by every
    /// `push_cmd`; written into `RING_TAIL` at `submit`.
    tail: u32,
    /// Cached FPGA-side `RING_HEAD` at frame start, used for free-space
    /// accounting. We don't re-read it mid-frame; for typical workloads
    /// the ring is far larger than one frame's commands.
    head: u32,
    pre_frame_count: u32,
    will_present: bool,
}

/// Encoded length of the largest single command (COPY_RECT with tint =
/// 7 words = 28 bytes). The push scratch buffer is sized to this so any
/// command — and any wrap-pad NOP, which is bounded by the same upper
/// limit — fits without heap allocation.
const MAX_CMD_BYTES: usize = 32;

impl<'a> Frame<'a> {
    pub(crate) fn new(device: &'a mut Device) -> Self {
        let ring_size = device.ring_size_bytes() as u32;
        debug_assert!(ring_size.is_power_of_two());
        let tail = device.ring_tail();
        let head = device.read_ring_head();
        let pre_frame_count = device.frame_count();
        Self {
            device,
            ring_size,
            ring_mask: ring_size - 1,
            tail,
            head,
            pre_frame_count,
            will_present: false,
        }
    }

    /// Encode `cmd` and stream it into the ring at the current tail,
    /// inserting a NOP-pad first if the command would otherwise straddle
    /// the physical end of the ring (see PROTOCOL.md §4 — the FPGA
    /// fetcher requires every command to live contiguously).
    ///
    /// Mirrors the algorithm in [`crate::ring::RingWriter::push`] but
    /// targets `/dev/mem` directly via `volatile_copy_to_devmem` instead
    /// of an in-memory `&mut [u8]` slice.
    fn push_cmd(&mut self, cmd: &Command) -> Result<(), DeviceError> {
        let need = cmd.encoded_len();
        debug_assert!(
            need <= MAX_CMD_BYTES,
            "command encoded_len ({need}) exceeds MAX_CMD_BYTES",
        );

        // NOP-pad if the command would cross the physical end of the
        // ring. `pad_bytes` is the gap between tail and ring_end; the
        // pad itself is encoded as a single NOP whose length covers
        // exactly that span.
        let to_end = (self.ring_size - self.tail) as usize;
        let pad_bytes = if need > to_end { to_end } else { 0 };
        let total = pad_bytes + need;

        // Free-space check (one word of margin so empty/full are
        // distinguishable, per §4.2).
        let free = self
            .head
            .wrapping_sub(self.tail)
            .wrapping_sub(4)
            & self.ring_mask;
        if total as u32 > free {
            return Err(DeviceError::Ring(RingError::Full {
                need: total,
                free: free as usize,
            }));
        }

        let mut buf = [0u8; MAX_CMD_BYTES];
        let ring_ptr = self.device.ring_devmem_ptr();

        if pad_bytes > 0 {
            debug_assert_eq!(pad_bytes % 4, 0);
            // Pad-NOP: padding_words counts argument words, so its total
            // encoded size is 4 + 4*padding_words = pad_bytes.
            let padding_words = ((pad_bytes / 4) - 1) as u8;
            let nop = Command::Nop { padding_words };
            nop.encode(&mut buf[..pad_bytes]).map_err(RingError::from)?;
            // SAFETY: ring_ptr maps `ring_size` bytes; tail < ring_size
            // and tail+pad_bytes <= ring_size (pad fills exactly to end).
            unsafe { volatile_copy_to_devmem(ring_ptr.add(self.tail as usize), &buf[..pad_bytes]) };
            self.tail = (self.tail + pad_bytes as u32) & self.ring_mask;
            debug_assert_eq!(self.tail, 0, "NOP-pad should wrap tail to 0");
        }

        cmd.encode(&mut buf[..need]).map_err(RingError::from)?;
        // SAFETY: ring_ptr maps `ring_size` bytes; tail+need <= ring_size
        // (we either just wrapped to 0 or `to_end >= need`).
        unsafe { volatile_copy_to_devmem(ring_ptr.add(self.tail as usize), &buf[..need]) };
        self.tail = (self.tail + need as u32) & self.ring_mask;
        Ok(())
    }

    /// Append a `FILL_RECT` honouring the user clip.
    pub fn fill_rect(
        mut self,
        dst: Rect,
        color: Rgba,
        blend: BlendMode,
    ) -> Result<Self, DeviceError> {
        self.push_cmd(&Command::FillRect {
            dst,
            color,
            blend,
            ignore_clip: false,
        })?;
        Ok(self)
    }

    /// Append a `FILL_RECT` that bypasses the user clip (framebuffer
    /// bounds are still enforced).
    pub fn fill_rect_unclipped(
        mut self,
        dst: Rect,
        color: Rgba,
        blend: BlendMode,
    ) -> Result<Self, DeviceError> {
        self.push_cmd(&Command::FillRect {
            dst,
            color,
            blend,
            ignore_clip: true,
        })?;
        Ok(self)
    }

    /// Append a `COPY_RECT` referring to `tex`.
    pub fn copy_rect(
        mut self,
        tex: &TextureHandle,
        src: Rect,
        dst: Rect,
        opts: CopyOpts,
    ) -> Result<Self, DeviceError> {
        self.push_cmd(&Command::CopyRect {
            tex_id: tex.id as u32,
            src,
            dst,
            blend: opts.blend,
            filter: opts.filter,
            tint: opts.tint,
        })?;
        Ok(self)
    }

    /// Set the user clip rectangle.
    pub fn set_clip(mut self, rect: Rect) -> Result<Self, DeviceError> {
        self.push_cmd(&Command::SetClip(rect))?;
        Ok(self)
    }

    /// Clear the user clip.
    pub fn clear_clip(mut self) -> Result<Self, DeviceError> {
        self.push_cmd(&Command::ClearClip)?;
        Ok(self)
    }

    /// Append a `PRESENT`. A frame may be submitted without one — useful
    /// for off-screen draws or initial setup.
    pub fn present(mut self) -> Result<Self, DeviceError> {
        self.push_cmd(&Command::Present)?;
        self.will_present = true;
        Ok(self)
    }

    /// Redirect subsequent draws to render into `target`'s pixel data
    /// instead of the framebuffer. Pair with
    /// [`Self::set_target_framebuffer`] to switch back. The target
    /// must be RGBA8888 with `pitch_bytes == width * 4` (PROTOCOL.md
    /// §5.6).
    pub fn set_target(mut self, target: &TextureHandle) -> Result<Self, DeviceError> {
        self.push_cmd(&Command::SetRenderTarget {
            tex_id: target.id,
        })?;
        Ok(self)
    }

    /// Restore the framebuffer as the active render target. Default at
    /// the start of every frame; only needed after a
    /// [`Self::set_target`] call.
    pub fn set_target_framebuffer(mut self) -> Result<Self, DeviceError> {
        self.push_cmd(&Command::SetRenderTarget {
            tex_id: TARGET_FRAMEBUFFER,
        })?;
        Ok(self)
    }

    /// Mark scanlines `y..y+h` dirty in the compositor's per-scanline
    /// bitmask (COMPOSITOR_V2.md §6 / §8.1). `x` and `w` are reserved
    /// for future per-rect dirty tracking; today they're encoded but
    /// unused on the FPGA side.
    ///
    /// Multiple calls in one frame accumulate (OR-into-place). The
    /// host typically calls [`Self::mask_commit`] once at the end of
    /// the frame to promote the rectangles into the compositor's
    /// active bank.
    pub fn invalidate_rect(mut self, rect: Rect) -> Result<Self, DeviceError> {
        self.push_cmd(&Command::InvalidateRect(rect))?;
        Ok(self)
    }

    /// Mark every scanline dirty in one op (§8.2). Used on boot and
    /// after any operation that changes the entire scene (e.g.
    /// resolution switch).
    pub fn invalidate_all(mut self) -> Result<Self, DeviceError> {
        self.push_cmd(&Command::InvalidateAll)?;
        Ok(self)
    }

    /// Atomically promote queued INVALIDATE_RECT / INVALIDATE_ALL
    /// writes into the compositor's active dirty bitmask (§8.3). The
    /// previously-active bank is zeroed so the next frame's host
    /// invalidations start from a clean slate.
    pub fn mask_commit(mut self) -> Result<Self, DeviceError> {
        self.push_cmd(&Command::MaskCommit)?;
        Ok(self)
    }

    /// Append a trailing `FENCE`, advance `RING_TAIL`, and pulse
    /// `RING_KICK`.
    ///
    /// Commands have already streamed into the ring as they were
    /// pushed (see [`Self::push_cmd`]); submit just publishes the new
    /// tail to the FPGA after a release fence so the DDR3 writes are
    /// visible before the consumer can observe the bump.
    pub fn submit(mut self) -> Result<FenceToken<'a>, DeviceError> {
        let fence_value = self.device.allocate_fence();
        self.push_cmd(&Command::Fence {
            value: fence_value,
        })?;

        let new_tail = self.tail;

        // dsb-st: ensure DDR3 writes are visible before RING_TAIL advance.
        fence(Ordering::Release);

        let regs = self.device.regs();
        regs.write32(registers::RING_TAIL, new_tail);
        regs.write32(registers::RING_KICK, 1);
        self.device.set_ring_tail(new_tail);

        Ok(FenceToken {
            device: self.device,
            fence_value,
            pre_frame_count: self.pre_frame_count,
            will_present: self.will_present,
        })
    }
}

/// Handle to an in-flight frame. Wait for it via [`FenceToken::wait`]
/// or [`FenceToken::wait_presented`]; dropping the token without
/// waiting is allowed but means the host doesn't synchronize on
/// completion.
pub struct FenceToken<'a> {
    device: &'a mut Device,
    fence_value: u32,
    pre_frame_count: u32,
    will_present: bool,
}

impl FenceToken<'_> {
    /// The value the FPGA will write into `FENCE_VALUE` once the fence
    /// retires.
    #[inline]
    pub fn fence_value(&self) -> u32 {
        self.fence_value
    }

    /// True if this frame contained a `PRESENT`. Only then is
    /// [`wait_presented`](Self::wait_presented) meaningful.
    #[inline]
    pub fn will_present(&self) -> bool {
        self.will_present
    }

    /// Spin-wait until the fence retires.
    pub fn wait(self, timeout: Duration) -> Result<(), DeviceError> {
        self.device.wait_fence(self.fence_value, timeout)
    }

    /// Spin-wait until the fence retires AND `FRAME_COUNT` has
    /// advanced past the value observed at frame start. Returns the
    /// observed `FRAME_COUNT`.
    pub fn wait_presented(self, timeout: Duration) -> Result<u32, DeviceError> {
        let start = Instant::now();
        self.device.wait_fence(self.fence_value, timeout)?;
        let remaining = timeout.saturating_sub(start.elapsed());
        self.device
            .wait_frame_count(self.pre_frame_count.wrapping_add(1), remaining)
    }

    /// Like [`wait_presented`](Self::wait_presented) but also returns
    /// `(fence_wait, frame_count_wait)`. Useful for instrumentation:
    /// the first measures FPGA command-stream completion, the second
    /// measures the wait for the next HDMI vsync.
    pub fn wait_presented_timed(
        self,
        timeout: Duration,
    ) -> Result<(u32, Duration, Duration), DeviceError> {
        let start = Instant::now();
        self.device.wait_fence(self.fence_value, timeout)?;
        let fence_done = Instant::now();
        let remaining = timeout.saturating_sub(fence_done - start);
        let count = self
            .device
            .wait_frame_count(self.pre_frame_count.wrapping_add(1), remaining)?;
        let scanout_done = Instant::now();
        Ok((count, fence_done - start, scanout_done - fence_done))
    }
}
