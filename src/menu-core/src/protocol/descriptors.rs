//! Texture descriptor layout (PROTOCOL.md §6).

use core::mem::{align_of, size_of};

/// Texture pixel format codes (PROTOCOL.md §6.2).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum TextureFormat {
    /// 4 bytes per pixel, in-memory order B, G, R, A.
    Rgba8888 = 0,
    /// 1 byte per pixel; alpha only.
    A8 = 1,
}

impl TextureFormat {
    /// Bytes per pixel for this format.
    #[inline]
    pub const fn bytes_per_pixel(self) -> u32 {
        match self {
            TextureFormat::Rgba8888 => 4,
            TextureFormat::A8 => 1,
        }
    }
}

/// 32-byte texture descriptor stored in the descriptor table in DDR3.
///
/// Written directly by the host with ordinary memory stores, read by
/// the FPGA during `COPY_RECT` execution. Field layout must match
/// PROTOCOL.md §6.1 byte-for-byte.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct TextureDescriptor {
    /// Physical address of pixel data in DDR3.
    pub data_addr: u32,
    /// Bytes per row. Must be `>= width * bytes_per_pixel`; larger
    /// values allow sub-region copies within a larger atlas (§6.4).
    pub pitch_bytes: u32,
    /// Width in pixels.
    pub width: u16,
    /// Height in pixels.
    pub height: u16,
    /// Format code (see [`TextureFormat`]).
    pub format: u8,
    /// Reserved flags. Must be 0 in v0.
    pub flags: u8,
    /// Reserved bytes 0x0E..0x10. Must be 0.
    _reserved_0e: u16,
    /// Reserved bytes 0x10..0x20. Must be 0.
    _reserved_10: [u32; 4],
}

impl TextureDescriptor {
    /// Build a descriptor with the default reserved-zero fields.
    pub const fn new(
        data_addr: u32,
        pitch_bytes: u32,
        width: u16,
        height: u16,
        format: TextureFormat,
    ) -> Self {
        Self {
            data_addr,
            pitch_bytes,
            width,
            height,
            format: format as u8,
            flags: 0,
            _reserved_0e: 0,
            _reserved_10: [0; 4],
        }
    }

    /// Total byte count of the pixel data implied by pitch × height.
    /// Useful for sizing allocations; not used by the FPGA.
    #[inline]
    pub fn data_bytes(&self) -> u32 {
        self.pitch_bytes.saturating_mul(self.height as u32)
    }
}

// Compile-time assertions that the descriptor is exactly 32 bytes and
// 4-byte aligned (for DDR3 burst-friendly access).
const _: () = {
    assert!(size_of::<TextureDescriptor>() == 32);
    assert!(align_of::<TextureDescriptor>() == 4);
};

/// Size of a single texture descriptor in bytes.
pub const DESCRIPTOR_SIZE: usize = 32;

#[cfg(test)]
mod tests {
    use super::*;
    use core::mem::offset_of;

    #[test]
    fn descriptor_size_is_32_bytes() {
        assert_eq!(size_of::<TextureDescriptor>(), 32);
        assert_eq!(DESCRIPTOR_SIZE, 32);
    }

    #[test]
    fn descriptor_field_offsets_match_spec() {
        // PROTOCOL.md §6.1
        assert_eq!(offset_of!(TextureDescriptor, data_addr), 0x00);
        assert_eq!(offset_of!(TextureDescriptor, pitch_bytes), 0x04);
        assert_eq!(offset_of!(TextureDescriptor, width), 0x08);
        assert_eq!(offset_of!(TextureDescriptor, height), 0x0A);
        assert_eq!(offset_of!(TextureDescriptor, format), 0x0C);
        assert_eq!(offset_of!(TextureDescriptor, flags), 0x0D);
    }

    #[test]
    fn descriptor_builds_with_defaults_zero() {
        let d = TextureDescriptor::new(0x3200_0000, 512, 128, 64, TextureFormat::Rgba8888);
        assert_eq!(d.format, 0);
        assert_eq!(d.flags, 0);
        assert_eq!(d._reserved_0e, 0);
        assert_eq!(d._reserved_10, [0; 4]);
    }

    #[test]
    fn format_bytes_per_pixel() {
        assert_eq!(TextureFormat::Rgba8888.bytes_per_pixel(), 4);
        assert_eq!(TextureFormat::A8.bytes_per_pixel(), 1);
    }
}
