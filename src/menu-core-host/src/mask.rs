//! Content coverage mask (scanout compositor, task #15).
//!
//! The compositor can skip reading + blending the content layer on 64x64
//! tiles that have no non-transparent pixels (the menu UI is ~94%
//! transparent), roughly halving the scanout DDR read and relieving the
//! transition-frame contention. This builds the 1-bit-per-tile bitmap the
//! FPGA consumes: one packed `u32` per tile row, the low [`TILES_X`] bits
//! marking covered tile columns.
//!
//! The host must keep the mask **conservative** — a set bit that turns out
//! transparent only wastes a little bandwidth, but an unset bit over real
//! content would drop it. Callers therefore mark every drawn element's
//! bounding box and union recent frames (the displayed framebuffer slot can
//! lag the rendered one by up to two frames under triple-buffering).

/// Tile edge in pixels. Must match the compositor's `TILE`.
pub const TILE: u32 = 64;
/// Tiles across a 1920-wide screen.
pub const TILES_X: usize = 30;
/// Tile rows down a 1080-tall screen (ceil(1080/64)).
pub const TILES_Y: usize = 17;

const SCREEN_W: u32 = 1920;
const SCREEN_H: u32 = 1080;

/// A 30x17 tile coverage bitmap, one `u32` per row (bit `tx` = column `tx`).
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct CoverageMask {
    rows: [u32; TILES_Y],
}

impl Default for CoverageMask {
    fn default() -> Self {
        Self::empty()
    }
}

impl CoverageMask {
    /// Empty (nothing covered).
    pub const fn empty() -> Self {
        Self {
            rows: [0; TILES_Y],
        }
    }

    /// Fully covered (every tile) — the safe fallback (= no skipping).
    pub const fn full() -> Self {
        let row = (1u32 << TILES_X) - 1;
        Self {
            rows: [row; TILES_Y],
        }
    }

    /// Mark every tile overlapping the pixel rectangle `(x, y, w, h)`,
    /// clamped to the screen. Negative / off-screen parts are clipped.
    pub fn mark_rect(&mut self, x: i32, y: i32, w: u32, h: u32) {
        if w == 0 || h == 0 {
            return;
        }
        let x1 = x as i64 + w as i64 - 1;
        let y1 = y as i64 + h as i64 - 1;
        if x1 < 0 || y1 < 0 {
            return;
        }
        let x0 = x.max(0) as u32;
        let y0 = y.max(0) as u32;
        let x1 = (x1 as u32).min(SCREEN_W - 1);
        let y1 = (y1 as u32).min(SCREEN_H - 1);
        if x0 > x1 || y0 > y1 {
            return;
        }
        let tx0 = (x0 / TILE) as usize;
        let tx1 = ((x1 / TILE) as usize).min(TILES_X - 1);
        let ty0 = (y0 / TILE) as usize;
        let ty1 = ((y1 / TILE) as usize).min(TILES_Y - 1);
        // contiguous run of set bits [tx0, tx1]
        let span = (tx1 - tx0 + 1) as u32;
        let bits: u32 = if span >= 32 {
            u32::MAX
        } else {
            ((1u32 << span) - 1) << tx0
        };
        for ty in ty0..=ty1 {
            self.rows[ty] |= bits;
        }
    }

    /// OR another mask into this one (for unioning recent frames).
    pub fn union(&mut self, other: &CoverageMask) {
        for i in 0..TILES_Y {
            self.rows[i] |= other.rows[i];
        }
    }

    /// Packed rows for [`crate::device::Device::upload_content_mask`].
    pub fn words(&self) -> &[u32; TILES_Y] {
        &self.rows
    }

    /// Count of set tiles (diagnostics).
    pub fn covered_tiles(&self) -> u32 {
        self.rows.iter().map(|r| r.count_ones()).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_is_zero() {
        assert_eq!(CoverageMask::empty().covered_tiles(), 0);
    }

    #[test]
    fn full_covers_all() {
        assert_eq!(
            CoverageMask::full().covered_tiles(),
            (TILES_X * TILES_Y) as u32
        );
    }

    #[test]
    fn single_pixel_marks_one_tile() {
        let mut m = CoverageMask::empty();
        m.mark_rect(70, 70, 1, 1); // tile (1,1)
        assert_eq!(m.covered_tiles(), 1);
        assert_eq!(m.words()[1], 1 << 1);
    }

    #[test]
    fn rect_spans_tiles() {
        let mut m = CoverageMask::empty();
        m.mark_rect(0, 0, 128, 64); // tiles x0,x1 at row 0
        assert_eq!(m.words()[0], 0b11);
        assert_eq!(m.words()[1], 0);
    }

    #[test]
    fn offscreen_clipped() {
        let mut m = CoverageMask::empty();
        m.mark_rect(-100, -100, 50, 50); // fully off top-left
        assert_eq!(m.covered_tiles(), 0);
    }

    #[test]
    fn union_ors() {
        let mut a = CoverageMask::empty();
        a.mark_rect(0, 0, 1, 1);
        let mut b = CoverageMask::empty();
        b.mark_rect(64, 0, 1, 1);
        a.union(&b);
        assert_eq!(a.words()[0], 0b11);
    }
}
