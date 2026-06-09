//! Control register offsets and access helpers for the LW_H2F window.
//!
//! Mirrors PROTOCOL.md §3.1. All offsets are byte offsets from the
//! register block base (`0xFF210000` physical on the DE10-Nano).

/// Byte size of the register window actually used.
pub const REGISTER_WINDOW_SIZE: usize = 0x100;

// --- Register offsets (§3.1) -------------------------------------------

pub const ID: usize = 0x00;
pub const STATUS: usize = 0x04;
pub const CONTROL: usize = 0x08;
pub const ERROR_INFO: usize = 0x0C;
pub const VSYNC_COUNT: usize = 0x10;
pub const FRAME_COUNT: usize = 0x14;
pub const VIDEO_MODE: usize = 0x18;
pub const VIDEO_INFO: usize = 0x1C;
pub const FB_STATE: usize = 0x20;
pub const FB_WIDTH: usize = 0x24;
pub const FB_HEIGHT: usize = 0x28;
pub const FB_STRIDE: usize = 0x2C;
pub const RING_BASE: usize = 0x30;
pub const RING_SIZE: usize = 0x34;
pub const RING_HEAD: usize = 0x38;
pub const RING_TAIL: usize = 0x3C;
pub const RING_KICK: usize = 0x40;
pub const FENCE_VALUE: usize = 0x48;
pub const FB0_ADDR: usize = 0x50;
pub const FB1_ADDR: usize = 0x54;
pub const FB2_ADDR: usize = 0x58;
pub const TEX_TABLE_ADDR: usize = 0x60;
pub const TEX_TABLE_COUNT: usize = 0x64;
/// Physical base of the 16 KB layer-table region (two back-to-back
/// 8 KB tables; PROTOCOL.md §11.1). Programmed once at init.
pub const LAYER_TABLE_BASE: usize = 0x68;
/// Atomic frame swap (PROTOCOL.md §3.1, §11.2).
/// Bit 31 = active table (0 = A, 1 = B). Bits 8..0 = valid layer
/// count (0..256). All other bits reserved (write 0).
pub const LAYER_COMMIT: usize = 0x6C;
/// Read-only diagnostic. Free-running 32-bit count of layer
/// descriptors the on-FPGA DMA has written into the layer cache —
/// should advance by `count` every frame once the compositor is live.
/// Pure observability; the renderer does not depend on it.
pub const LAYER_DEBUG: usize = 0x70;
/// Base address of the opaque wallpaper layer (scanout compositor,
/// Phase B). The compositor blends the content framebuffer over this
/// layer when `CONTROL_COMPOSITE` is set. Full-screen BGRA8888, same
/// stride as the content FB (`FB_STRIDE`).
pub const WALLPAPER_ADDR: usize = 0x74;
pub const PERF_CYCLES_BUSY: usize = 0x80;
pub const PERF_CMDS_EXEC: usize = 0x84;
pub const PERF_BYTES_READ: usize = 0x88;
pub const PERF_BYTES_WRITTEN: usize = 0x8C;

// --- STATUS bits (§3.2) ------------------------------------------------

pub const STATUS_ERROR: u32 = 1 << 0;
pub const STATUS_BUSY: u32 = 1 << 1;
pub const STATUS_UNDERRUN: u32 = 1 << 2;
pub const STATUS_VSYNC: u32 = 1 << 3;

// --- CONTROL bits (§3.2) -----------------------------------------------

pub const CONTROL_ENABLE: u32 = 1 << 0;
pub const CONTROL_SOFT_RESET: u32 = 1 << 1;
pub const CONTROL_CLEAR_ERROR: u32 = 1 << 2;
/// Enable the scanout compositor's content-over-wallpaper blend
/// (Phase B). When clear, the compositor scans out the content FB
/// directly (no wallpaper read/blend). Latched into `CONTROL[8]`.
pub const CONTROL_COMPOSITE: u32 = 1 << 8;

// --- FB_STATE field extraction (§3.2) ----------------------------------

/// Extract `FB_DISPLAY` (bits 1:0) from the packed `FB_STATE` value.
#[inline]
pub const fn fb_state_display(s: u32) -> u8 {
    (s & 0x3) as u8
}

/// Extract `FB_RENDER` (bits 3:2) from the packed `FB_STATE` value.
#[inline]
pub const fn fb_state_render(s: u32) -> u8 {
    ((s >> 2) & 0x3) as u8
}

/// Extract `FB_READY` (bits 5:4) from the packed `FB_STATE` value.
/// Value 3 means "no frame pending".
#[inline]
pub const fn fb_state_ready(s: u32) -> u8 {
    ((s >> 4) & 0x3) as u8
}

/// Assemble an `FB_STATE` value from its three 2-bit fields. Intended
/// primarily for unit tests; the FPGA is the actual writer.
#[inline]
pub const fn pack_fb_state(display: u8, render: u8, ready: u8) -> u32 {
    ((display & 0x3) as u32) | (((render & 0x3) as u32) << 2) | (((ready & 0x3) as u32) << 4)
}

/// The post-reset value of `FB_STATE`: `FB_DISPLAY=0, FB_RENDER=0, FB_READY=3`.
pub const FB_STATE_RESET: u32 = pack_fb_state(0, 0, 3);

// --- Typed register access over a raw pointer -------------------------

/// Typed, volatile access wrapper over an LW_H2F mmap pointer.
///
/// Holds a raw pointer, not a reference, because the FPGA can modify
/// memory concurrently. All reads and writes are volatile to prevent
/// the compiler from reordering or eliding them.
///
/// # Safety
///
/// The caller must guarantee `base` is a valid, mapped pointer to at
/// least `REGISTER_WINDOW_SIZE` bytes and remains valid for the
/// lifetime of the `RegisterBlock`.
#[derive(Clone, Copy)]
pub struct RegisterBlock {
    base: *mut u8,
}

// `RegisterBlock` is just a raw-pointer wrapper; access is always
// explicit and volatile. It is not inherently `Send`/`Sync`.
impl RegisterBlock {
    /// # Safety
    ///
    /// `base` must be a valid mmap pointer to at least
    /// [`REGISTER_WINDOW_SIZE`] bytes.
    pub unsafe fn new(base: *mut u8) -> Self {
        Self { base }
    }

    /// Volatile 32-bit read at `offset` (must be 4-byte aligned).
    #[inline]
    pub fn read32(&self, offset: usize) -> u32 {
        debug_assert_eq!(offset & 0x3, 0, "register offset must be 4-byte aligned");
        debug_assert!(
            offset + 4 <= REGISTER_WINDOW_SIZE,
            "register offset out of range"
        );
        unsafe { core::ptr::read_volatile(self.base.add(offset) as *const u32) }
    }

    /// Volatile 32-bit write at `offset` (must be 4-byte aligned).
    #[inline]
    pub fn write32(&self, offset: usize, value: u32) {
        debug_assert_eq!(offset & 0x3, 0, "register offset must be 4-byte aligned");
        debug_assert!(
            offset + 4 <= REGISTER_WINDOW_SIZE,
            "register offset out of range"
        );
        unsafe { core::ptr::write_volatile(self.base.add(offset) as *mut u32, value) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fb_state_fields() {
        let s = pack_fb_state(0, 1, 2);
        assert_eq!(fb_state_display(s), 0);
        assert_eq!(fb_state_render(s), 1);
        assert_eq!(fb_state_ready(s), 2);
    }

    #[test]
    fn fb_state_reset_matches_spec() {
        // PROTOCOL.md §3.3: reset state is FB_STATE = 0x30
        assert_eq!(FB_STATE_RESET, 0x30);
    }

    #[test]
    fn offsets_match_spec() {
        assert_eq!(ID, 0x00);
        assert_eq!(STATUS, 0x04);
        assert_eq!(CONTROL, 0x08);
        assert_eq!(FB_STATE, 0x20);
        assert_eq!(RING_BASE, 0x30);
        assert_eq!(RING_TAIL, 0x3C);
        assert_eq!(RING_KICK, 0x40);
        assert_eq!(FENCE_VALUE, 0x48);
        assert_eq!(FB0_ADDR, 0x50);
        assert_eq!(TEX_TABLE_ADDR, 0x60);
        assert_eq!(LAYER_TABLE_BASE, 0x68);
        assert_eq!(LAYER_COMMIT, 0x6C);
        assert_eq!(LAYER_DEBUG, 0x70);
        assert_eq!(WALLPAPER_ADDR, 0x74);
        assert_eq!(CONTROL_COMPOSITE, 1 << 8);
    }
}
