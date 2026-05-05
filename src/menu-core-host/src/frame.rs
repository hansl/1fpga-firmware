//! Per-frame command builder + fence token.
//!
//! [`Frame`] is acquired via [`Device::begin_frame`], populated with
//! draw calls, then handed off to [`Frame::submit`] which copies the
//! batched commands into the device-side ring, kicks the engine, and
//! returns a [`FenceToken`] for synchronisation.
//!
//! [`Device::begin_frame`]: crate::device::Device::begin_frame

use std::sync::atomic::{Ordering, fence};
use std::time::{Duration, Instant};

use crate::device::Device;
use crate::devmem::volatile_copy_to_devmem;
use crate::error::DeviceError;
use crate::protocol::{BlendMode, Command, Filter, Rect, Rgba, registers};
use crate::ring::RingWriter;
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
    staging: Vec<u8>,
    tail: u32,
    head: u32,
    pre_frame_count: u32,
    will_present: bool,
}

impl<'a> Frame<'a> {
    pub(crate) fn new(device: &'a mut Device) -> Self {
        let ring_size = device.ring_size_bytes();
        let staging = vec![0u8; ring_size];
        let tail = device.ring_tail();
        let head = device.read_ring_head();
        let pre_frame_count = device.frame_count();
        Self {
            device,
            staging,
            tail,
            head,
            pre_frame_count,
            will_present: false,
        }
    }

    fn push_cmd(&mut self, cmd: &Command) -> Result<(), DeviceError> {
        let new_tail = {
            let mut writer = RingWriter::with_state(&mut self.staging, self.tail, self.head)?;
            writer.push(cmd)?;
            writer.tail()
        };
        self.tail = new_tail;
        Ok(())
    }

    /// Append a `FILL_RECT` honouring the user clip.
    pub fn fill_rect(
        &mut self,
        dst: Rect,
        color: Rgba,
        blend: BlendMode,
    ) -> Result<&mut Self, DeviceError> {
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
        &mut self,
        dst: Rect,
        color: Rgba,
        blend: BlendMode,
    ) -> Result<&mut Self, DeviceError> {
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
        &mut self,
        tex: &TextureHandle,
        src: Rect,
        dst: Rect,
        opts: CopyOpts,
    ) -> Result<&mut Self, DeviceError> {
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
    pub fn set_clip(&mut self, rect: Rect) -> Result<&mut Self, DeviceError> {
        self.push_cmd(&Command::SetClip(rect))?;
        Ok(self)
    }

    /// Clear the user clip.
    pub fn clear_clip(&mut self) -> Result<&mut Self, DeviceError> {
        self.push_cmd(&Command::ClearClip)?;
        Ok(self)
    }

    /// Append a `PRESENT`. A frame may be submitted without one — useful
    /// for off-screen draws or initial setup.
    pub fn present(&mut self) -> Result<&mut Self, DeviceError> {
        self.push_cmd(&Command::Present)?;
        self.will_present = true;
        Ok(self)
    }

    /// Encode a trailing `FENCE`, copy the staging buffer into the
    /// device-side ring, advance `RING_TAIL`, and pulse `RING_KICK`.
    pub fn submit(mut self) -> Result<FenceToken<'a>, DeviceError> {
        let fence_value = self.device.allocate_fence();
        self.push_cmd(&Command::Fence {
            value: fence_value,
        })?;

        let prev_tail = self.device.ring_tail();
        let new_tail = self.tail;
        let ring_size = self.staging.len();
        let ring_ptr = self.device.ring_devmem_ptr();

        if new_tail >= prev_tail {
            let start = prev_tail as usize;
            let end = new_tail as usize;
            // SAFETY: `ring_ptr` is a /dev/mem mapping of `ring_size`
            // bytes; the slice indexes stay within the staging buffer
            // (also `ring_size` bytes); `volatile_copy_to_devmem` writes
            // byte-by-byte volatile.
            unsafe {
                volatile_copy_to_devmem(ring_ptr.add(start), &self.staging[start..end]);
            }
        } else {
            // Wrapped: write [prev_tail .. ring_size), then [0 .. new_tail).
            let start = prev_tail as usize;
            let end = new_tail as usize;
            unsafe {
                volatile_copy_to_devmem(ring_ptr.add(start), &self.staging[start..ring_size]);
                volatile_copy_to_devmem(ring_ptr, &self.staging[..end]);
            }
        }

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
}
