//! Memory layout of the reserved DDR3 region (PROTOCOL.md §2).
//!
//! The region sits within the MiSTer kernel's existing `memmap=513M$511M`
//! reservation (`[0x1FF00000, 0x40000000)`); our 256 MB window at
//! `0x30000000` requires no kernel changes.

/// Default physical base address of the carve-out. This is what we
/// program into FPGA control registers at startup. Any 32-MB-aligned
/// address in the reserved DDR3 region is valid; the spec only fixes
/// the *relative* offsets below.
pub const DEFAULT_BASE: u32 = 0x3000_0000;

/// Total size of the carve-out.
pub const REGION_SIZE: usize = 256 * 1024 * 1024;

// --- Relative offsets within the region (PROTOCOL.md §2) --------------

pub const FB0_OFFSET: usize = 0x0000_0000;
pub const FB1_OFFSET: usize = 0x0080_0000;
pub const FB2_OFFSET: usize = 0x0100_0000;
pub const RING_OFFSET: usize = 0x0180_0000;
pub const TEX_TABLE_OFFSET: usize = 0x0190_0000;
/// Layer descriptor table — the compositor scanout's source of truth.
/// Two back-to-back tables of 8 KB each (256 layers × 32 B); the
/// active table is selected via the `LAYER_ACTIVE` register, letting
/// the host build a new frame's layout in the inactive table and
/// swap atomically so scanout never observes a partial update.
/// Sits in the gap between TEX_TABLE_OFFSET and TEX_POOL_OFFSET.
pub const LAYER_TABLE_OFFSET: usize = 0x01B0_0000;
pub const TEX_POOL_OFFSET: usize = 0x0200_0000;

// --- Sizes of each sub-region ----------------------------------------

/// Size of each framebuffer slot (8 MB; 1080p32 fits with 94 KB padding).
pub const FB_SLOT_SIZE: usize = 8 * 1024 * 1024;

/// Size of the command ring buffer (must be a power of two).
pub const RING_SIZE: usize = 1024 * 1024;

/// Size of the texture descriptor table (2 MB = 65 536 × 32 B max).
/// `tex_id` is 16-bit on the wire, so the absolute ceiling is
/// `0x10000` slots; we reserve `0xFFFF` as the framebuffer sentinel
/// (PROTOCOL.md §5.6) so `DEFAULT_TEX_TABLE_COUNT = 65 535`.
pub const TEX_TABLE_SIZE: usize = 2 * 1024 * 1024;

/// Default number of texture descriptor entries. Capped at the
/// 16-bit `tex_id` ceiling minus the framebuffer sentinel
/// (`0xFFFF`). See PROTOCOL.md §2.3.
pub const DEFAULT_TEX_TABLE_COUNT: u32 = 65_535;

/// Size of the texture data pool (224 MB).
pub const TEX_POOL_SIZE: usize = 224 * 1024 * 1024;

/// Content coverage mask (task #15). One bit per 64x64 tile; the
/// compositor reads 5x128-bit beats = 80 bytes, with one packed u32 per
/// tile row in the low 30 bits. The two ping-pong mask buffers are
/// allocated from the texture pool.
pub const MASK_ROWS: usize = 17; // ceil(1080 / 64)
pub const MASK_BYTES: usize = 80; // 5 beats x 16 bytes (>= MASK_ROWS*4)

/// Number of layer descriptor slots per layer table. 256 is enough
/// for a complex menu UI (background + cards + text + transition
/// overlays + reserve) and fits comfortably in BRAM on the FPGA
/// side once the compositor reads them.
pub const LAYERS_PER_TABLE: u32 = 256;

/// Bytes per layer descriptor (matches PROTOCOL.md §7.1).
pub const LAYER_DESCRIPTOR_SIZE: usize = 32;

/// Size of one layer table in bytes (256 × 32 = 8 KB).
pub const LAYER_TABLE_SIZE: usize =
    (LAYERS_PER_TABLE as usize) * LAYER_DESCRIPTOR_SIZE;

/// Size of the layer-table region: two back-to-back tables for
/// double-buffered atomic commit (16 KB total).
pub const LAYER_REGION_SIZE: usize = 2 * LAYER_TABLE_SIZE;

/// Offset of the second (back) layer table within the region.
/// Tables A/B alternate as the active descriptor source.
pub const LAYER_TABLE_B_OFFSET: usize = LAYER_TABLE_OFFSET + LAYER_TABLE_SIZE;

/// Convenience: physical address of a sub-region given the carve-out
/// base and offset.
#[inline]
pub const fn phys_addr(base: u32, offset: usize) -> u32 {
    base + offset as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offsets_are_32mb_aligned_for_pool() {
        // Texture pool starts at 32 MB for clean power-of-2 alignment.
        assert_eq!(TEX_POOL_OFFSET % (32 * 1024 * 1024), 0);
    }

    #[test]
    fn ring_size_is_power_of_two() {
        assert!(RING_SIZE.is_power_of_two());
    }

    #[test]
    fn total_fits_region_size() {
        let end = TEX_POOL_OFFSET + TEX_POOL_SIZE;
        assert_eq!(end, REGION_SIZE);
    }

    #[test]
    fn fb_slot_fits_1080p_bgra() {
        let used = 1920 * 1080 * 4;
        assert!(FB_SLOT_SIZE >= used);
    }

    #[test]
    fn regions_do_not_overlap() {
        let regions = [
            (FB0_OFFSET, FB_SLOT_SIZE),
            (FB1_OFFSET, FB_SLOT_SIZE),
            (FB2_OFFSET, FB_SLOT_SIZE),
            (RING_OFFSET, RING_SIZE),
            (TEX_TABLE_OFFSET, TEX_TABLE_SIZE),
            (LAYER_TABLE_OFFSET, LAYER_REGION_SIZE),
            (TEX_POOL_OFFSET, TEX_POOL_SIZE),
        ];
        for (i, (a_start, a_size)) in regions.iter().enumerate() {
            for (j, (b_start, b_size)) in regions.iter().enumerate() {
                if i == j {
                    continue;
                }
                let a_end = a_start + a_size;
                let b_end = b_start + b_size;
                assert!(
                    a_end <= *b_start || b_end <= *a_start,
                    "regions {} and {} overlap",
                    i,
                    j
                );
            }
        }
    }

    #[test]
    fn tex_table_accommodates_default_count() {
        assert!(TEX_TABLE_SIZE >= (DEFAULT_TEX_TABLE_COUNT as usize) * 32);
    }

    #[test]
    fn layer_table_dimensions() {
        assert_eq!(
            LAYER_TABLE_SIZE,
            LAYERS_PER_TABLE as usize * LAYER_DESCRIPTOR_SIZE
        );
        // Both tables must fit in the 5 MB gap between TEX_TABLE end
        // and TEX_POOL start.
        let gap = TEX_POOL_OFFSET - (TEX_TABLE_OFFSET + TEX_TABLE_SIZE);
        assert!(LAYER_REGION_SIZE <= gap);
        // B table immediately follows A.
        assert_eq!(LAYER_TABLE_B_OFFSET, LAYER_TABLE_OFFSET + LAYER_TABLE_SIZE);
    }

    #[test]
    fn phys_addr_adds_correctly() {
        assert_eq!(phys_addr(0x3000_0000, FB0_OFFSET), 0x3000_0000);
        assert_eq!(phys_addr(0x3000_0000, TEX_POOL_OFFSET), 0x3200_0000);
    }
}
