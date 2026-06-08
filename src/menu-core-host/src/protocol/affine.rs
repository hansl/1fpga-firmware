//! Affine-transform helpers for [`Command::BlitAffine`] (PROTOCOL.md
//! §5.3 BLIT_AFFINE).
//!
//! The FPGA samples the source by an **inverse** affine map (it walks
//! the destination and, per pixel, looks up where that pixel came from
//! in the source). The host precomputes that inverse matrix in Q16.16
//! fixed point so the fabric needs no trig.
//!
//! For a destination pixel at offset `(ox, oy)` from the destination
//! AABB origin, the sampled source coordinate is:
//!
//! ```text
//! srcx = m00·ox + m01·oy + tx
//! srcy = m10·ox + m11·oy + ty
//! ```
//!
//! `srcx`/`srcy` are in **texel space**: integer `n` is the center of
//! texel `n`, so the FPGA takes `floor(srcx)`/`frac(srcx)` directly as
//! the bilinear base index/weight. The source origin `(sx, sy)` and the
//! pixel-center / texel-center half-pixel biases are folded into
//! `tx, ty` here.
//!
//! [`Command::BlitAffine`]: crate::protocol::commands::Command::BlitAffine

use std::f64::consts::PI;

use super::commands::Rect;

/// `1.0` in Q16.16.
pub const Q16_ONE: i32 = 1 << 16;

/// Maximum source sub-rect dimension for `BLIT_AFFINE` — the on-chip
/// staging buffer is sized for this (PROTOCOL.md §5.7).
pub const MAX_SRC_DIM: u16 = 128;

/// Whether a source sub-rect fits within the affine staging limit
/// (PROTOCOL.md §5.7). Used to reject oversized affine blits host-side
/// before emission.
#[inline]
pub fn src_within_limit(src: Rect) -> bool {
    src.w <= MAX_SRC_DIM && src.h <= MAX_SRC_DIM
}

/// Convert a real value to Q16.16, round-to-nearest. Saturates on
/// overflow (coordinates and coefficients are tiny relative to i32
/// range in practice, but a degenerate `scale → 0` could blow up `k`).
#[inline]
fn to_q16(v: f64) -> i32 {
    let scaled = (v * 65536.0).round();
    if scaled >= i32::MAX as f64 {
        i32::MAX
    } else if scaled <= i32::MIN as f64 {
        i32::MIN
    } else {
        scaled as i32
    }
}

/// A built affine blit: the destination AABB and the inverse 2×3 matrix
/// `[m00, m01, m10, m11, tx, ty]` (Q16.16) for [`Command::BlitAffine`].
///
/// [`Command::BlitAffine`]: crate::protocol::commands::Command::BlitAffine
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct AffineBlit {
    pub dst: Rect,
    pub m: [i32; 6],
}

/// Build the inverse sampling matrix for rotating a source sub-rect
/// `src` clockwise by `angle_deg` and scaling it uniformly by `scale`,
/// rendered into a destination AABB of size `dw × dh`.
///
/// The matrix is independent of where the AABB is *placed* (offsets are
/// measured from the AABB origin), so callers position the result by
/// choosing `dst.x / dst.y` freely. `dw / dh` should be the AABB that
/// tightly contains the transformed source — see [`rotate_scale`],
/// which computes it.
///
/// `scale` must be > 0.
pub fn inverse_matrix(src: Rect, dw: u16, dh: u16, angle_deg: f64, scale: f64) -> [i32; 6] {
    let th = angle_deg * PI / 180.0;
    let c = th.cos();
    let s = th.sin();
    // Inverse uniform scale. The 2×2 inverse-rotation block is the
    // transpose of R(θ): [[c, s], [-s, c]], pre-multiplied by 1/scale.
    let k = 1.0 / scale;
    let m00 = k * c;
    let m01 = k * s;
    let m10 = -k * s;
    let m11 = k * c;

    let sw = src.w as f64;
    let sh = src.h as f64;
    let dwf = dw as f64;
    let dhf = dh as f64;

    // Evaluate the inverse map at the destination pixel *center*
    // (ox+0.5) and convert the resulting source corner coordinate to
    // texel-center space (subtract 0.5). The source rect center maps to
    // the AABB center; everything else falls out of the affine form.
    let tx = src.x as f64 + sw / 2.0 - 0.5 + m00 * (0.5 - dwf / 2.0) + m01 * (0.5 - dhf / 2.0);
    let ty = src.y as f64 + sh / 2.0 - 0.5 + m10 * (0.5 - dwf / 2.0) + m11 * (0.5 - dhf / 2.0);

    [
        to_q16(m00),
        to_q16(m01),
        to_q16(m10),
        to_q16(m11),
        to_q16(tx),
        to_q16(ty),
    ]
}

/// Compute the axis-aligned bounding box (in pixels) that tightly
/// contains `src` rotated by `angle_deg` and scaled by `scale`.
pub fn aabb_size(src: Rect, angle_deg: f64, scale: f64) -> (u16, u16) {
    let th = angle_deg * PI / 180.0;
    let c = th.cos().abs();
    let s = th.sin().abs();
    let sw = src.w as f64;
    let sh = src.h as f64;
    // Subtract a tiny epsilon before ceil so floating-point dust (e.g.
    // cos(90°) = 6.12e-17 making 4.0 read as 4.0000000000000005) does
    // not bump an exact integer AABB up by a whole pixel.
    let ceil_dim = |v: f64| (v - 1e-6).ceil().clamp(0.0, u16::MAX as f64) as u16;
    let dw = ceil_dim(scale * (sw * c + sh * s));
    let dh = ceil_dim(scale * (sw * s + sh * c));
    (dw, dh)
}

/// Convenience: build a rotate + uniform-scale affine blit of `src`,
/// with the transformed image centered at framebuffer point
/// `(cx, cy)`. Returns the destination AABB (sized to fit the rotated
/// image, positioned so its center is at `(cx, cy)`) and the inverse
/// matrix.
///
/// Negative placement (the AABB extending past the top/left edge) is
/// saturated to origin `0`; keep `(cx, cy)` at least `dw/2`, `dh/2`
/// inside the framebuffer for exact positioning. Right/bottom overflow
/// is handled correctly by the FPGA's clip.
pub fn rotate_scale(src: Rect, angle_deg: f64, scale: f64, cx: i32, cy: i32) -> AffineBlit {
    let (dw, dh) = aabb_size(src, angle_deg, scale);
    let dx = (cx - dw as i32 / 2).clamp(0, u16::MAX as i32) as u16;
    let dy = (cy - dh as i32 / 2).clamp(0, u16::MAX as i32) as u16;
    AffineBlit {
        dst: Rect::new(dx, dy, dw, dh),
        m: inverse_matrix(src, dw, dh, angle_deg, scale),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    /// Apply the inverse matrix to a destination offset, returning the
    /// source texel coordinate in floating point (mirrors what the FPGA
    /// computes from the Q16.16 coefficients).
    fn sample(m: &[i32; 6], ox: f64, oy: f64) -> (f64, f64) {
        let f = |q: i32| q as f64 / 65536.0;
        (
            f(m[0]) * ox + f(m[1]) * oy + f(m[4]),
            f(m[2]) * ox + f(m[3]) * oy + f(m[5]),
        )
    }

    #[test]
    fn src_limit_predicate() {
        assert!(src_within_limit(Rect::new(0, 0, 128, 128)));
        assert!(src_within_limit(Rect::new(10, 20, 1, 1)));
        assert!(!src_within_limit(Rect::new(0, 0, 129, 128)));
        assert!(!src_within_limit(Rect::new(0, 0, 128, 129)));
    }

    #[test]
    fn identity_is_unit_matrix_zero_translation() {
        let src = Rect::new(0, 0, 64, 48);
        let m = inverse_matrix(src, 64, 48, 0.0, 1.0);
        assert_eq!(m, [Q16_ONE, 0, 0, Q16_ONE, 0, 0]);
        // Every destination pixel maps to the same source pixel.
        let (sx, sy) = sample(&m, 10.0, 7.0);
        assert!((sx - 10.0).abs() < 1e-9);
        assert!((sy - 7.0).abs() < 1e-9);
    }

    #[test]
    fn identity_with_source_origin_offsets_translation() {
        // A sub-rect at (5, 9) of a larger atlas: dst offset 0 must map
        // to source texel (5, 9).
        let src = Rect::new(5, 9, 32, 32);
        let m = inverse_matrix(src, 32, 32, 0.0, 1.0);
        let (sx, sy) = sample(&m, 0.0, 0.0);
        assert!((sx - 5.0).abs() < 1e-9, "sx={sx}");
        assert!((sy - 9.0).abs() < 1e-9, "sy={sy}");
    }

    #[test]
    fn rotate_180_flips_both_axes() {
        let (w, h) = (10u16, 6u16);
        let src = Rect::new(0, 0, w, h);
        let m = inverse_matrix(src, w, h, 180.0, 1.0);
        // dst (0,0) ← src (w-1, h-1); dst (w-1,h-1) ← src (0,0).
        let (sx, sy) = sample(&m, 0.0, 0.0);
        assert!((sx - (w as f64 - 1.0)).abs() < 1e-6, "sx={sx}");
        assert!((sy - (h as f64 - 1.0)).abs() < 1e-6, "sy={sy}");
        let (sx, sy) = sample(&m, (w - 1) as f64, (h - 1) as f64);
        assert!(sx.abs() < 1e-6, "sx={sx}");
        assert!(sy.abs() < 1e-6, "sy={sy}");
    }

    #[test]
    fn rotate_90_cw_maps_corners() {
        // 90° CW: source W×H → AABB H×W. dst (0,0) ← source bottom-left.
        let (w, h) = (8u16, 4u16);
        let src = Rect::new(0, 0, w, h);
        let (dw, dh) = aabb_size(src, 90.0, 1.0);
        assert_eq!((dw, dh), (h, w));
        let m = inverse_matrix(src, dw, dh, 90.0, 1.0);
        // output (ox,oy) ← source (oy, h-1-ox)
        // top-left output ← source bottom-left (0, h-1)
        let (sx, sy) = sample(&m, 0.0, 0.0);
        assert!(sx.abs() < 1e-6, "sx={sx}");
        assert!((sy - (h as f64 - 1.0)).abs() < 1e-6, "sy={sy}");
        // bottom-left output (ox=0, oy=dh-1=w-1) ← source bottom-right (w-1, h-1)
        let (sx, sy) = sample(&m, 0.0, (dh - 1) as f64);
        assert!((sx - (w as f64 - 1.0)).abs() < 1e-6, "sx={sx}");
        assert!((sy - (h as f64 - 1.0)).abs() < 1e-6, "sy={sy}");
    }

    #[test]
    fn scale_2x_halves_the_source_step() {
        // Upscale 2×: AABB doubles, and stepping one dst pixel advances
        // the source by half a texel.
        let src = Rect::new(0, 0, 32, 32);
        let (dw, dh) = aabb_size(src, 0.0, 2.0);
        assert_eq!((dw, dh), (64, 64));
        let m = inverse_matrix(src, dw, dh, 0.0, 2.0);
        // m00 = 1/scale = 0.5
        assert_eq!(m[0], Q16_ONE / 2);
        let (sx0, _) = sample(&m, 0.0, 0.0);
        let (sx1, _) = sample(&m, 1.0, 0.0);
        assert!((sx1 - sx0 - 0.5).abs() < 1e-9);
    }

    #[test]
    fn rotate_scale_centers_aabb_on_point() {
        let src = Rect::new(0, 0, 40, 40);
        let blit = rotate_scale(src, 45.0, 1.0, 500, 300);
        // 45° AABB of a 40×40 square: ceil(40·√2) = 57 each side.
        let expected = (40.0_f64 * 2.0_f64.sqrt()).ceil() as u16;
        assert_eq!(blit.dst.w, expected);
        assert_eq!(blit.dst.h, expected);
        assert_eq!(blit.dst.x, 500 - expected as u16 / 2);
        assert_eq!(blit.dst.y, 300 - expected as u16 / 2);
        // The center *pixel* (index (dw-1)/2, since the +0.5 pixel-center
        // bias is folded into the matrix) samples the source center.
        let (sx, sy) = sample(
            &blit.m,
            (blit.dst.w as f64 - 1.0) / 2.0,
            (blit.dst.h as f64 - 1.0) / 2.0,
        );
        assert!((sx - 19.5).abs() < 0.01, "sx={sx}");
        assert!((sy - 19.5).abs() < 0.01, "sy={sy}");
    }
}
