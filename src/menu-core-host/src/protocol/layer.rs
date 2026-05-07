//! Layer descriptor layout for compositor scanout (PROTOCOL.md §7).
//!
//! Each layer is a single 32-byte descriptor in DDR3 (the layer table,
//! see [`crate::mem`]). The compositor scanout module — once the FPGA
//! side lands — walks the active layer table per HDMI scanline,
//! finds layers whose `dst` rect intersects the current Y, samples
//! pixels from the referenced texture (or applies a solid colour),
//! and composites them in slot order over a transparent background.
//!
//! **Slot index is z-order**: slot 0 draws first (back), slot 255
//! draws last (front). The host is responsible for keeping the table
//! ordered when z-positions change. Disabling a slot (clearing its
//! `enabled` flag) leaves a hole that the compositor skips.

use core::mem::{align_of, size_of};

/// Sentinel `tex_id` meaning "this layer is a solid colour" — the
/// compositor uses the [`LayerDescriptor::color`] field as the
/// per-pixel value and never touches a texture for this layer.
pub const LAYER_TEX_SOLID: u16 = 0xFFFF;

/// Blend mode encoding shared with the blit engine. Values match the
/// `BLEND_*` constants the FPGA already recognises.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum LayerBlend {
    /// Replace the underlying pixel with the layer's pixel.
    Opaque = 0,
    /// `out = src + dst * (1 - src.a)` (premultiplied alpha math).
    SrcAlpha = 1,
    /// `out = saturating_add(src, dst)` per-channel.
    Additive = 2,
}

impl LayerBlend {
    /// Inverse of `as u8`: parse a wire value back into a [`LayerBlend`].
    /// Returns `None` for unrecognised codes; descriptors with such
    /// values will be treated as disabled by the compositor.
    pub const fn from_bits(b: u8) -> Option<Self> {
        match b {
            0 => Some(Self::Opaque),
            1 => Some(Self::SrcAlpha),
            2 => Some(Self::Additive),
            _ => None,
        }
    }
}

/// Bit positions within [`LayerDescriptor::flags`] (PROTOCOL.md §7.1.b).
pub mod flag {
    /// Slot is active. The compositor only honours layers with
    /// `flags & ENABLED != 0`.
    pub const ENABLED: u16 = 1 << 0;
    /// `tex_id` references an A8 (alpha-only) atlas; sampled alpha is
    /// modulated against [`LayerDescriptor::color`] for the RGB
    /// component (matches the blit engine's tinted-A8 path).
    pub const TINT_FROM_A8: u16 = 1 << 1;
    /// Bits 2-3: blend-mode field. Use [`set_blend`] / [`get_blend`].
    pub const BLEND_MASK: u16 = 0b11 << 2;
    pub const BLEND_SHIFT: u32 = 2;

    /// Encode a [`super::LayerBlend`] into the flag word.
    #[inline]
    pub const fn with_blend(flags: u16, blend: super::LayerBlend) -> u16 {
        (flags & !BLEND_MASK) | (((blend as u16) << BLEND_SHIFT) & BLEND_MASK)
    }
}

/// 32-byte layer descriptor, written by the host with ordinary stores
/// and read by the compositor each scanline. Field layout must match
/// PROTOCOL.md §7.1 byte-for-byte.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct LayerDescriptor {
    /// Bit 0: enabled. Bit 1: tint-from-A8. Bits 2-3: blend mode.
    /// Higher bits reserved (must be 0).
    pub flags: u16,
    /// Texture id to sample from. `LAYER_TEX_SOLID` (0xFFFF) means
    /// this is a solid-colour layer — the compositor uses
    /// [`Self::color`] as every pixel and never reads a texture.
    pub tex_id: u16,
    /// Top-left of the layer in framebuffer-pixel coordinates. Signed
    /// so layers can extend past the screen on the top/left edges.
    pub dst_x: i16,
    pub dst_y: i16,
    /// Layer dimensions on screen.
    pub dst_w: u16,
    pub dst_h: u16,
    /// Source rect within the referenced texture. Ignored for solid-
    /// colour layers. (`src_w` / `src_h` ≠ `dst_w` / `dst_h` is
    /// reserved for a future scaling extension; for now keep them
    /// equal.)
    pub src_x: u16,
    pub src_y: u16,
    pub src_w: u16,
    pub src_h: u16,
    /// Solid colour (`tex_id == LAYER_TEX_SOLID`) OR tint colour
    /// (when [`flag::TINT_FROM_A8`] or RGBA tint is in use). BGRA in
    /// memory order, matching the blit engine.
    pub color: u32,
    /// Per-layer alpha multiplier (0..255). Applied on top of the
    /// blend mode — useful for fade in/out without rebuilding the
    /// referenced texture.
    pub opacity: u8,
    /// Reserved bytes 0x19..0x20. Must be 0.
    pub _reserved: [u8; 7],
}

impl LayerDescriptor {
    /// Build a textured layer covering `dst` with full-texture
    /// sampling, opaque blend, and full opacity.
    pub const fn textured(tex_id: u16, dst_x: i16, dst_y: i16, w: u16, h: u16) -> Self {
        Self {
            flags: flag::ENABLED,
            tex_id,
            dst_x,
            dst_y,
            dst_w: w,
            dst_h: h,
            src_x: 0,
            src_y: 0,
            src_w: w,
            src_h: h,
            color: 0,
            opacity: 0xFF,
            _reserved: [0; 7],
        }
    }

    /// Build a solid-colour layer covering `dst`.
    pub const fn solid(color: u32, dst_x: i16, dst_y: i16, w: u16, h: u16) -> Self {
        Self {
            flags: flag::ENABLED,
            tex_id: LAYER_TEX_SOLID,
            dst_x,
            dst_y,
            dst_w: w,
            dst_h: h,
            src_x: 0,
            src_y: 0,
            src_w: w,
            src_h: h,
            color,
            opacity: 0xFF,
            _reserved: [0; 7],
        }
    }

    /// Apply a blend-mode setting to the descriptor's flags word.
    #[inline]
    pub const fn with_blend(mut self, blend: LayerBlend) -> Self {
        self.flags = flag::with_blend(self.flags, blend);
        self
    }

    /// Set per-layer opacity (0 = fully transparent, 0xFF = full).
    #[inline]
    pub const fn with_opacity(mut self, opacity: u8) -> Self {
        self.opacity = opacity;
        self
    }

    /// Returns `true` if the slot is active.
    #[inline]
    pub const fn is_enabled(&self) -> bool {
        (self.flags & flag::ENABLED) != 0
    }
}

// Compile-time assertions matching the spec.
const _: () = {
    assert!(size_of::<LayerDescriptor>() == 32);
    assert!(align_of::<LayerDescriptor>() == 4);
};

#[cfg(test)]
mod tests {
    use super::*;
    use core::mem::offset_of;

    #[test]
    fn descriptor_size_is_32_bytes() {
        assert_eq!(size_of::<LayerDescriptor>(), 32);
    }

    #[test]
    fn descriptor_field_offsets() {
        // PROTOCOL.md §7.1 wire layout. Lock these — the FPGA
        // compositor will read by offset, not by Rust struct field.
        assert_eq!(offset_of!(LayerDescriptor, flags), 0x00);
        assert_eq!(offset_of!(LayerDescriptor, tex_id), 0x02);
        assert_eq!(offset_of!(LayerDescriptor, dst_x), 0x04);
        assert_eq!(offset_of!(LayerDescriptor, dst_y), 0x06);
        assert_eq!(offset_of!(LayerDescriptor, dst_w), 0x08);
        assert_eq!(offset_of!(LayerDescriptor, dst_h), 0x0A);
        assert_eq!(offset_of!(LayerDescriptor, src_x), 0x0C);
        assert_eq!(offset_of!(LayerDescriptor, src_y), 0x0E);
        assert_eq!(offset_of!(LayerDescriptor, src_w), 0x10);
        assert_eq!(offset_of!(LayerDescriptor, src_h), 0x12);
        assert_eq!(offset_of!(LayerDescriptor, color), 0x14);
        assert_eq!(offset_of!(LayerDescriptor, opacity), 0x18);
    }

    #[test]
    fn textured_constructor_sets_enabled_full_opacity() {
        let d = LayerDescriptor::textured(7, 100, 200, 240, 240);
        assert!(d.is_enabled());
        assert_eq!(d.tex_id, 7);
        assert_eq!(d.dst_x, 100);
        assert_eq!(d.dst_y, 200);
        assert_eq!(d.dst_w, 240);
        assert_eq!(d.dst_h, 240);
        assert_eq!(d.src_w, 240);
        assert_eq!(d.src_h, 240);
        assert_eq!(d.opacity, 0xFF);
    }

    #[test]
    fn solid_constructor_uses_sentinel_tex() {
        let d = LayerDescriptor::solid(0xFF80_4060, 0, 0, 100, 100);
        assert_eq!(d.tex_id, LAYER_TEX_SOLID);
        assert_eq!(d.color, 0xFF80_4060);
    }

    #[test]
    fn default_is_disabled() {
        let d = LayerDescriptor::default();
        assert!(!d.is_enabled());
    }

    #[test]
    fn blend_round_trips_through_flags() {
        let d = LayerDescriptor::textured(1, 0, 0, 10, 10).with_blend(LayerBlend::SrcAlpha);
        let bits = (d.flags & flag::BLEND_MASK) >> flag::BLEND_SHIFT;
        assert_eq!(LayerBlend::from_bits(bits as u8), Some(LayerBlend::SrcAlpha));

        let d = d.with_blend(LayerBlend::Additive);
        let bits = (d.flags & flag::BLEND_MASK) >> flag::BLEND_SHIFT;
        assert_eq!(LayerBlend::from_bits(bits as u8), Some(LayerBlend::Additive));

        // Re-applying a blend doesn't disturb other flag bits.
        assert!(d.is_enabled());
    }

    #[test]
    fn from_bits_rejects_unknown_codes() {
        assert!(LayerBlend::from_bits(3).is_none());
        assert!(LayerBlend::from_bits(255).is_none());
    }
}
