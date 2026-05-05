//! Error types for the device runtime.

use std::io;
use std::time::Duration;

use thiserror::Error;

use crate::bridge::BridgeError;
use crate::devmem::DevMemError;
use crate::ring::RingError;

/// Top-level runtime error. Every fallible call on the device runtime
/// returns this.
#[derive(Debug, Error)]
pub enum DeviceError {
    /// Failed to enable the LW_H2F bridge via sysfs.
    #[error("LW_H2F bridge enable failed: {0}")]
    Bridge(#[from] BridgeError),

    /// `/dev/mem` mmap failed (typically permission denied or the
    /// kernel refusing to map a strict-devmem-protected range).
    #[error("/dev/mem map failed at {phys_addr:#X} ({size} bytes): {detail}")]
    Mmap {
        phys_addr: u32,
        size: usize,
        detail: &'static str,
    },

    /// The `ID` register did not contain the expected
    /// `(MAGIC << 16) | VERSION` value — typically because the FPGA is
    /// not running the menu-core RBF.
    #[error("device ID mismatch: got {got:#010X}, expected {expected:#010X}")]
    IdMismatch { got: u32, expected: u32 },

    /// Ring writer failure — see [`RingError`].
    #[error("ring writer error: {0}")]
    Ring(#[from] RingError),

    /// FPGA reported an error via `STATUS.ER` / `ERROR_INFO` while
    /// processing a command. The fetcher has stopped and must be
    /// recovered (clear-error or soft-reset).
    #[error("hardware error {kind:?} (info={info:#010X})")]
    Hardware {
        kind: HardwareErrorKind,
        info: u32,
    },

    /// Polling loop exhausted without the awaited condition.
    #[error("timeout waiting for {what} after {waited:?}")]
    Timeout {
        what: &'static str,
        waited: Duration,
    },

    /// Texture pool bump allocator out of room.
    #[error("texture pool exhausted: needed {needed} bytes, {free} free")]
    TexturePoolExhausted { needed: u32, free: u32 },

    /// Caller-supplied texture data slice is shorter than `stride *
    /// height`.
    #[error("texture data truncated: expected {expected} bytes, got {got}")]
    TextureDataTruncated { expected: usize, got: usize },

    /// Caller-supplied texture stride is below the format's row width.
    #[error("texture stride {stride} is below width*bpp ({row_bytes})")]
    TextureStrideTooSmall { stride: u32, row_bytes: u32 },

    /// All texture descriptor slots used.
    #[error("texture descriptor table full ({capacity} entries)")]
    DescriptorTableFull { capacity: u32 },

    /// `VIDEO_INFO` reports zeros — HDMI mode hasn't latched. Usually
    /// means the user needs to boot with a valid `MiSTer.ini`
    /// `video_mode=` entry first.
    #[error("VIDEO_INFO reports {width}×{height} — HDMI mode is not active")]
    VideoNotActive { width: u16, height: u16 },

    /// The chosen video mode produces a framebuffer too large for an
    /// 8 MB FB slot.
    #[error(
        "framebuffer {width}×{height} ({bytes} bytes) exceeds slot size ({slot_bytes} bytes)"
    )]
    FramebufferTooLarge {
        width: u16,
        height: u16,
        bytes: usize,
        slot_bytes: usize,
    },

    /// Reserved DDR3 base address misaligned (must be a 32 MB-aligned
    /// address inside the kernel reservation).
    #[error("base address {0:#X} is not 32-MB-aligned")]
    BadBaseAlignment(u32),

    /// `std::io` error from the bridge enable path.
    #[error("io error: {0}")]
    Io(#[from] io::Error),
}

impl From<DevMemError> for DeviceError {
    fn from(e: DevMemError) -> Self {
        DeviceError::Mmap {
            phys_addr: e.phys_addr,
            size: e.size,
            detail: e.detail,
        }
    }
}

/// Kind of hardware error, decoded from the low byte of `ERROR_INFO`.
/// See PROTOCOL.md §8.1 for the canonical table.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum HardwareErrorKind {
    /// `0x01` — unknown opcode encountered.
    BadOpcode,
    /// `0x02` — `length_w` did not match the opcode's expected value
    /// (detail packs `(expected << 8) | got`).
    BadLength,
    /// `0x03` — texture id out of range or invalid.
    BadTexture,
    /// `0x04` — unrecognized texture format.
    BadFormat,
    /// `0x06` — AXI response error (SLVERR=2, DECERR=3).
    Axi,
    /// `0x07` — scanout line buffer underrun.
    ScanoutUnderrun,
    /// `0x08` — host advanced `RING_TAIL` backwards unexpectedly.
    RingOverrun,
    /// Any unrecognized code (forward compatibility).
    Other(u8),
}

/// Decoded hardware error suitable for diagnostic display. The `info`
/// payload follows §8.1's per-error format.
#[derive(Debug, Copy, Clone)]
pub struct HardwareError {
    pub kind: HardwareErrorKind,
    pub info: u32,
}

impl HardwareError {
    /// Decode an `ERROR_INFO` register read. Returns `None` if the low
    /// byte is `ERR_NONE` (`0x00`).
    pub fn from_register(error_info: u32) -> Option<Self> {
        let code = (error_info & 0xFF) as u8;
        if code == 0 {
            return None;
        }
        let kind = match code {
            0x01 => HardwareErrorKind::BadOpcode,
            0x02 => HardwareErrorKind::BadLength,
            0x03 => HardwareErrorKind::BadTexture,
            0x04 => HardwareErrorKind::BadFormat,
            0x06 => HardwareErrorKind::Axi,
            0x07 => HardwareErrorKind::ScanoutUnderrun,
            0x08 => HardwareErrorKind::RingOverrun,
            other => HardwareErrorKind::Other(other),
        };
        Some(Self {
            kind,
            info: error_info,
        })
    }

    /// Detail field — the high 24 bits of `ERROR_INFO`.
    #[inline]
    pub fn detail(&self) -> u32 {
        self.info >> 8
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_none_is_some_only_when_low_byte_nonzero() {
        assert!(HardwareError::from_register(0x0000_0000).is_none());
        assert!(HardwareError::from_register(0xFFFF_FF00).is_none());
        assert!(HardwareError::from_register(0x0000_0001).is_some());
    }

    #[test]
    fn decode_known_codes() {
        let cases = [
            (0x0000_0001u32, HardwareErrorKind::BadOpcode),
            (0x1234_0002, HardwareErrorKind::BadLength),
            (0x0000_0003, HardwareErrorKind::BadTexture),
            (0x0000_0004, HardwareErrorKind::BadFormat),
            (0x0000_0006, HardwareErrorKind::Axi),
            (0x0000_0007, HardwareErrorKind::ScanoutUnderrun),
            (0x0000_0008, HardwareErrorKind::RingOverrun),
        ];
        for (info, expected_kind) in cases {
            let e = HardwareError::from_register(info).expect("should decode");
            assert_eq!(e.kind, expected_kind);
            assert_eq!(e.info, info);
        }
    }

    #[test]
    fn decode_unknown_code_is_other() {
        let e = HardwareError::from_register(0x0000_00FE).unwrap();
        assert_eq!(e.kind, HardwareErrorKind::Other(0xFE));
    }

    #[test]
    fn detail_strips_low_byte() {
        let e = HardwareError::from_register(0xAABBCC_01).unwrap();
        assert_eq!(e.detail(), 0x00AA_BBCC);
    }
}
