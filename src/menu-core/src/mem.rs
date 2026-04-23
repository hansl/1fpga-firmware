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
pub const TEX_POOL_OFFSET: usize = 0x0200_0000;

// --- Sizes of each sub-region ----------------------------------------

/// Size of each framebuffer slot (8 MB; 1080p32 fits with 94 KB padding).
pub const FB_SLOT_SIZE: usize = 8 * 1024 * 1024;

/// Size of the command ring buffer (must be a power of two).
pub const RING_SIZE: usize = 1024 * 1024;

/// Size of the texture descriptor table (128 KB = 4096 × 32 B default).
pub const TEX_TABLE_SIZE: usize = 128 * 1024;

/// Default number of texture descriptor entries. See PROTOCOL.md §2.3
/// — this is an allocation size, not a protocol limit.
pub const DEFAULT_TEX_TABLE_COUNT: u32 = 4096;

/// Size of the texture data pool (224 MB).
pub const TEX_POOL_SIZE: usize = 224 * 1024 * 1024;

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
    fn phys_addr_adds_correctly() {
        assert_eq!(phys_addr(0x3000_0000, FB0_OFFSET), 0x3000_0000);
        assert_eq!(phys_addr(0x3000_0000, TEX_POOL_OFFSET), 0x3200_0000);
    }
}
