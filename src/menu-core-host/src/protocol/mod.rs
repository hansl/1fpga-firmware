//! Protocol constants and types shared across the host/FPGA interface.
//!
//! The canonical source of truth is `cores/menu-core/PROTOCOL.md`. Any
//! divergence between this module and that document is a bug.

pub mod commands;
pub mod descriptors;
pub mod layer;
pub mod registers;

pub use commands::{BlendMode, Command, EncodeError, Filter, Rect, Rgba};
pub use descriptors::{TextureDescriptor, TextureFormat};
pub use layer::{LAYER_TEX_SOLID, LayerBlend, LayerDescriptor};

/// Magic value (high 16 bits of the `ID` register).
///
/// See PROTOCOL.md §3.2 `ID`.
pub const ID_MAGIC: u16 = 0x1FFA;

/// Protocol version (low 16 bits of the `ID` register).
pub const PROTOCOL_VERSION: u16 = 1;

/// Combined `ID` register value: `(MAGIC << 16) | VERSION`.
pub const ID_VALUE: u32 = ((ID_MAGIC as u32) << 16) | (PROTOCOL_VERSION as u32);

/// Fixed pixel dimensions for the v0 target mode (1920×1080).
pub const FB_WIDTH: u16 = 1920;
pub const FB_HEIGHT: u16 = 1080;

/// Bytes per pixel (BGRA8888).
pub const FB_BYTES_PER_PIXEL: usize = 4;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_value_matches_spec() {
        // PROTOCOL.md §3.2: ID register value is 0x1FFA0001 for version 1
        assert_eq!(ID_VALUE, 0x1FFA_0001);
    }

    #[test]
    fn fb_dimensions_are_1080p() {
        assert_eq!(FB_WIDTH, 1920);
        assert_eq!(FB_HEIGHT, 1080);
    }
}
