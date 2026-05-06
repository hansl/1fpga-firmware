//! Command types and binary encoder (PROTOCOL.md §5).
//!
//! Every command is a 4-byte header followed by zero or more 32-bit
//! argument words, written little-endian into the command ring. See
//! the spec for the canonical opcode table, flag layouts, and the
//! optional-argument convention used by `COPY_RECT`.

/// Rectangle with 16-bit unsigned coordinates. Coordinates are in
/// framebuffer pixel space (origin top-left).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    pub x: u16,
    pub y: u16,
    pub w: u16,
    pub h: u16,
}

impl Rect {
    pub const fn new(x: u16, y: u16, w: u16, h: u16) -> Self {
        Self { x, y, w, h }
    }
}

/// RGBA color. Stored logically as four 8-bit channels; encoded as a
/// 32-bit little-endian word laid out `A:R:G:B` from MSB to LSB
/// (see PROTOCOL.md §7.1).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Rgba {
    pub const fn new(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    /// Convert to the 32-bit wire representation (`A:R:G:B`, MSB→LSB).
    ///
    /// When this u32 is stored little-endian into memory the in-memory
    /// byte order becomes `B, G, R, A` — matching §7.1.
    #[inline]
    pub const fn to_u32(self) -> u32 {
        ((self.a as u32) << 24) | ((self.r as u32) << 16) | ((self.g as u32) << 8) | (self.b as u32)
    }

    pub const WHITE: Rgba = Rgba::new(255, 255, 255, 255);
    pub const BLACK: Rgba = Rgba::new(0, 0, 0, 255);
    pub const TRANSPARENT: Rgba = Rgba::new(0, 0, 0, 0);
}

/// Blend mode for `FILL_RECT` / `COPY_RECT` (PROTOCOL.md §7.2).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum BlendMode {
    #[default]
    Opaque = 0,
    SrcAlpha = 1,
    Additive = 2,
}

/// Sampling filter for `COPY_RECT` scaling (PROTOCOL.md §7.4).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum Filter {
    #[default]
    Nearest = 0,
}

// --- Opcodes (PROTOCOL.md §5.2) ----------------------------------------

pub const OP_NOP: u8 = 0x00;
pub const OP_PRESENT: u8 = 0x01;
pub const OP_FENCE: u8 = 0x02;
pub const OP_SET_CLIP: u8 = 0x03;
pub const OP_CLEAR_CLIP: u8 = 0x04;
pub const OP_SET_RENDER_TARGET: u8 = 0x05;
pub const OP_FILL_RECT: u8 = 0x10;
pub const OP_COPY_RECT: u8 = 0x11;
pub const OP_EXTENDED: u8 = 0xFF;

// --- Flag bits ---------------------------------------------------------

// FILL_RECT flags: bits [1:0] blend, bit 2 ignore_clip.
const FILL_FLAG_IGNORE_CLIP: u16 = 1 << 2;

// COPY_RECT flags: bits [1:0] blend, bits [3:2] filter, bit 4 tint_en.
const COPY_FLAG_TINT_EN: u16 = 1 << 4;

/// Sentinel `tex_id` for [`Command::SetRenderTarget`] that selects the
/// framebuffer (the default render target).
pub const TARGET_FRAMEBUFFER: u16 = 0xFFFF;

/// High-level command representation. Each variant corresponds 1:1 to
/// an opcode; the encoder (`encode`) handles flag packing, optional
/// argument words, and `length_w` calculation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    /// Padding, primarily used at ring-end wrap. `padding_words` is the
    /// number of 32-bit filler words after the header (`length_w`).
    Nop { padding_words: u8 },

    /// Commit the current render framebuffer; swap at next vsync.
    Present,

    /// Write `fence_value` to `FENCE_VALUE` once all prior commands
    /// have retired.
    Fence { value: u32 },

    /// Set the user clip rectangle. FPGA intersects with framebuffer
    /// bounds; out-of-bounds values are silently clamped (§5.5).
    SetClip(Rect),

    /// Disable the user clip rectangle.
    ClearClip,

    /// Redirect subsequent draws to a texture, or back to the
    /// framebuffer. `tex_id == TARGET_FRAMEBUFFER` selects the
    /// framebuffer (the default at frame start). See PROTOCOL.md §5.6.
    SetRenderTarget { tex_id: u16 },

    /// Solid-color rectangle. `ignore_clip = true` bypasses the user
    /// clip rect (framebuffer bounds are still enforced).
    FillRect {
        dst: Rect,
        color: Rgba,
        blend: BlendMode,
        ignore_clip: bool,
    },

    /// Textured rectangle. `tint` is the optional tint argument word;
    /// when `Some`, `length_w = 6` and the tint is multiplied into the
    /// source before blending. When `None`, `length_w = 5`.
    CopyRect {
        tex_id: u32,
        src: Rect,
        dst: Rect,
        blend: BlendMode,
        filter: Filter,
        tint: Option<Rgba>,
    },
}

/// Error type for [`Command::encode`].
#[derive(Copy, Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum EncodeError {
    #[error("output buffer too small: needed {needed} bytes, got {have}")]
    BufferTooSmall { needed: usize, have: usize },
}

impl Command {
    /// Number of 32-bit argument words after the header (the header's
    /// `length_w` field).
    #[inline]
    pub fn length_words(&self) -> u8 {
        match *self {
            Command::Nop { padding_words } => padding_words,
            Command::Present => 0,
            Command::Fence { .. } => 1,
            Command::SetClip(_) => 2,
            Command::ClearClip => 0,
            Command::SetRenderTarget { .. } => 1,
            Command::FillRect { .. } => 3,
            Command::CopyRect { tint, .. } => {
                if tint.is_some() {
                    6
                } else {
                    5
                }
            }
        }
    }

    /// Total encoded size in bytes (header + args).
    #[inline]
    pub fn encoded_len(&self) -> usize {
        4 + (self.length_words() as usize) * 4
    }

    /// Encode the command into `out` at offset 0, returning the number
    /// of bytes written.
    pub fn encode(&self, out: &mut [u8]) -> Result<usize, EncodeError> {
        let need = self.encoded_len();
        if out.len() < need {
            return Err(EncodeError::BufferTooSmall {
                needed: need,
                have: out.len(),
            });
        }

        // Header in canonical integer form: `opcode:8 | length_w:8 | flags:16`.
        let (opcode, length_w, flags) = self.header_fields();
        let header: u32 = ((opcode as u32) << 24) | ((length_w as u32) << 16) | (flags as u32);
        write_u32_le(&mut out[0..4], header);

        // Argument words in canonical order.
        let args = &mut out[4..need];
        match *self {
            Command::Nop { padding_words } => {
                // NOP argument words are ignored but we must zero them so
                // the ring doesn't contain uninitialized data (helpful for
                // debugging and for the FPGA's forward-scan logic).
                for i in 0..padding_words as usize {
                    write_u32_le(&mut args[i * 4..i * 4 + 4], 0);
                }
            }
            Command::Present | Command::ClearClip => {
                // No argument words.
            }
            Command::Fence { value } => {
                write_u32_le(&mut args[0..4], value);
            }
            Command::SetClip(r) => {
                write_u32_le(&mut args[0..4], pack_xy(r.x, r.y));
                write_u32_le(&mut args[4..8], pack_xy(r.w, r.h));
            }
            Command::SetRenderTarget { tex_id } => {
                // Word 0: tex_id in low 16, reserved (must be 0) in high 16.
                write_u32_le(&mut args[0..4], tex_id as u32);
            }
            Command::FillRect { dst, color, .. } => {
                write_u32_le(&mut args[0..4], pack_xy(dst.x, dst.y));
                write_u32_le(&mut args[4..8], pack_xy(dst.w, dst.h));
                write_u32_le(&mut args[8..12], color.to_u32());
            }
            Command::CopyRect {
                tex_id,
                src,
                dst,
                tint,
                ..
            } => {
                write_u32_le(&mut args[0..4], tex_id);
                write_u32_le(&mut args[4..8], pack_xy(src.x, src.y));
                write_u32_le(&mut args[8..12], pack_xy(src.w, src.h));
                write_u32_le(&mut args[12..16], pack_xy(dst.x, dst.y));
                write_u32_le(&mut args[16..20], pack_xy(dst.w, dst.h));
                if let Some(t) = tint {
                    write_u32_le(&mut args[20..24], t.to_u32());
                }
            }
        }

        Ok(need)
    }

    /// Return `(opcode, length_w, flags)` — the fields that pack into
    /// the 32-bit header.
    fn header_fields(&self) -> (u8, u8, u16) {
        let length_w = self.length_words();
        match *self {
            Command::Nop { .. } => (OP_NOP, length_w, 0),
            Command::Present => (OP_PRESENT, length_w, 0),
            Command::Fence { .. } => (OP_FENCE, length_w, 0),
            Command::SetClip(_) => (OP_SET_CLIP, length_w, 0),
            Command::ClearClip => (OP_CLEAR_CLIP, length_w, 0),
            Command::SetRenderTarget { .. } => (OP_SET_RENDER_TARGET, length_w, 0),
            Command::FillRect {
                blend, ignore_clip, ..
            } => {
                let mut flags: u16 = blend as u16;
                if ignore_clip {
                    flags |= FILL_FLAG_IGNORE_CLIP;
                }
                (OP_FILL_RECT, length_w, flags)
            }
            Command::CopyRect {
                blend,
                filter,
                tint,
                ..
            } => {
                let mut flags: u16 = blend as u16;
                flags |= (filter as u16) << 2;
                if tint.is_some() {
                    flags |= COPY_FLAG_TINT_EN;
                }
                (OP_COPY_RECT, length_w, flags)
            }
        }
    }
}

// --- Low-level helpers -------------------------------------------------

/// Pack two 16-bit values into a 32-bit word as `(high << 16) | low`
/// (PROTOCOL.md §5.3 preamble).
#[inline]
const fn pack_xy(high: u16, low: u16) -> u32 {
    ((high as u32) << 16) | (low as u32)
}

#[inline]
fn write_u32_le(out: &mut [u8], value: u32) {
    out[0] = value as u8;
    out[1] = (value >> 8) as u8;
    out[2] = (value >> 16) as u8;
    out[3] = (value >> 24) as u8;
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    /// Encode a command and return the bytes as a Vec for easy comparison.
    fn enc(cmd: &Command) -> Vec<u8> {
        let mut buf = vec![0u8; cmd.encoded_len()];
        let n = cmd.encode(&mut buf).unwrap();
        assert_eq!(n, cmd.encoded_len());
        buf
    }

    // Helper: build a u32-per-word byte string the way Appendix B
    // writes them (big-endian digits in the header, LE in memory).
    // We verify against raw LE byte sequences.

    #[test]
    fn nop_length_zero() {
        // Header `00 00 0000` big-endian → LE bytes [00, 00, 00, 00]
        let bytes = enc(&Command::Nop { padding_words: 0 });
        assert_eq!(bytes, vec![0x00, 0x00, 0x00, 0x00]);
    }

    #[test]
    fn nop_with_padding_emits_zero_args() {
        let bytes = enc(&Command::Nop { padding_words: 3 });
        // Header: opcode=0x00, length_w=0x03, flags=0x0000 → 00 03 00 00
        // LE:     [00, 00, 03, 00]
        // Padding: three zero words → 12 zero bytes
        assert_eq!(
            bytes,
            vec![
                0x00, 0x00, 0x03, 0x00, // header
                0x00, 0x00, 0x00, 0x00, // pad 0
                0x00, 0x00, 0x00, 0x00, // pad 1
                0x00, 0x00, 0x00, 0x00, // pad 2
            ]
        );
        assert_eq!(bytes.len(), 16);
    }

    #[test]
    fn present_is_4_bytes() {
        // Header `01 00 0000` → LE [00, 00, 00, 01]
        let bytes = enc(&Command::Present);
        assert_eq!(bytes, vec![0x00, 0x00, 0x00, 0x01]);
    }

    #[test]
    fn fence_writes_value() {
        // Header `02 01 0000` → LE [00, 00, 01, 02]
        // Value 0xDEADBEEF → LE [EF, BE, AD, DE]
        let bytes = enc(&Command::Fence { value: 0xDEAD_BEEF });
        assert_eq!(bytes, vec![0x00, 0x00, 0x01, 0x02, 0xEF, 0xBE, 0xAD, 0xDE]);
    }

    #[test]
    fn set_clip_packs_xy_wh() {
        // SET_CLIP (0x03), length_w=2, flags=0 → LE [00,00,02,03]
        // Word 0: x=0x0010, y=0x0020  packed as (0x0010 << 16) | 0x0020 = 0x0010_0020
        // Word 1: w=0x0040, h=0x0050  packed as (0x0040 << 16) | 0x0050 = 0x0040_0050
        let bytes = enc(&Command::SetClip(Rect::new(0x10, 0x20, 0x40, 0x50)));
        assert_eq!(
            bytes,
            vec![
                0x00, 0x00, 0x02, 0x03, // header
                0x20, 0x00, 0x10, 0x00, // x|y
                0x50, 0x00, 0x40, 0x00, // w|h
            ]
        );
    }

    #[test]
    fn clear_clip_is_4_bytes() {
        let bytes = enc(&Command::ClearClip);
        assert_eq!(bytes, vec![0x00, 0x00, 0x00, 0x04]);
    }

    #[test]
    fn set_render_target_packs_tex_id() {
        // SET_RENDER_TARGET (0x05), length_w=1, flags=0 → header LE [00,00,01,05]
        // tex_id 0x1234 → arg word LE [34, 12, 00, 00]
        let bytes = enc(&Command::SetRenderTarget { tex_id: 0x1234 });
        assert_eq!(
            bytes,
            vec![
                0x00, 0x00, 0x01, 0x05, // header
                0x34, 0x12, 0x00, 0x00, // tex_id
            ]
        );
    }

    #[test]
    fn set_render_target_framebuffer_sentinel() {
        let bytes = enc(&Command::SetRenderTarget {
            tex_id: TARGET_FRAMEBUFFER,
        });
        assert_eq!(&bytes[4..8], &[0xFF, 0xFF, 0x00, 0x00]);
    }

    #[test]
    fn fill_rect_opaque_no_ignore_clip() {
        // FILL_RECT (0x10), length_w=3, flags = {blend=0 = opaque, ignore_clip=0} = 0x0000
        // Header LE: [00,00,03,10]
        // Color RGBA(0xFF, 0x00, 0x00, 0xFF) (red, full alpha) → u32 0xFF_FF_00_00 (A:R:G:B)
        //   → LE [00, 00, FF, FF]
        let bytes = enc(&Command::FillRect {
            dst: Rect::new(0, 0, 0xFFFF, 0xFFFF),
            color: Rgba::new(0xFF, 0x00, 0x00, 0xFF),
            blend: BlendMode::Opaque,
            ignore_clip: false,
        });
        assert_eq!(
            bytes,
            vec![
                0x00, 0x00, 0x03, 0x10, // header
                0x00, 0x00, 0x00, 0x00, // dx=0 | dy=0
                0xFF, 0xFF, 0xFF, 0xFF, // dw=FFFF | dh=FFFF
                0x00, 0x00, 0xFF, 0xFF, // color A=FF, R=FF, G=00, B=00
            ]
        );
    }

    #[test]
    fn fill_rect_ignore_clip_sets_bit_2() {
        // ignore_clip=1, blend=opaque → flags=0x0004
        // Header LE: [04, 00, 03, 10]
        let bytes = enc(&Command::FillRect {
            dst: Rect::new(0, 0, 0xFFFF, 0xFFFF),
            color: Rgba::BLACK,
            blend: BlendMode::Opaque,
            ignore_clip: true,
        });
        assert_eq!(&bytes[0..4], &[0x04, 0x00, 0x03, 0x10]);
    }

    #[test]
    fn fill_rect_blend_additive() {
        let bytes = enc(&Command::FillRect {
            dst: Rect::new(0, 0, 1, 1),
            color: Rgba::BLACK,
            blend: BlendMode::Additive,
            ignore_clip: false,
        });
        // flags = 2 (additive), ignore_clip=0 → 0x0002
        assert_eq!(&bytes[0..4], &[0x02, 0x00, 0x03, 0x10]);
    }

    #[test]
    fn copy_rect_no_tint_is_5_words() {
        // COPY_RECT (0x11), length_w=5, flags = {blend=opaque=0, filter=nearest=0, tint_en=0} = 0
        // Header LE: [00,00,05,11]
        let bytes = enc(&Command::CopyRect {
            tex_id: 0x0000_0042,
            src: Rect::new(0, 0, 32, 32),
            dst: Rect::new(100, 200, 32, 32),
            blend: BlendMode::Opaque,
            filter: Filter::Nearest,
            tint: None,
        });
        assert_eq!(bytes.len(), 4 + 5 * 4);
        assert_eq!(&bytes[0..4], &[0x00, 0x00, 0x05, 0x11]);
        // tex_id
        assert_eq!(&bytes[4..8], &[0x42, 0x00, 0x00, 0x00]);
    }

    #[test]
    fn copy_rect_with_tint_is_6_words() {
        // tint_en=1 → flags bit 4 set → flags = 0x0010
        // Header LE: [10, 00, 06, 11]
        let bytes = enc(&Command::CopyRect {
            tex_id: 0,
            src: Rect::new(0, 0, 8, 8),
            dst: Rect::new(0, 0, 8, 8),
            blend: BlendMode::SrcAlpha,
            filter: Filter::Nearest,
            tint: Some(Rgba::WHITE),
        });
        assert_eq!(bytes.len(), 4 + 6 * 4);
        // flags = blend(1) | tint_en(0x10) = 0x11
        assert_eq!(&bytes[0..4], &[0x11, 0x00, 0x06, 0x11]);
        // Last word is the tint RGBA of WHITE (0xFFFF_FFFF)
        assert_eq!(&bytes[24..28], &[0xFF, 0xFF, 0xFF, 0xFF]);
    }

    #[test]
    fn length_w_matches_tint_presence() {
        let with_tint = Command::CopyRect {
            tex_id: 0,
            src: Rect::default(),
            dst: Rect::default(),
            blend: BlendMode::Opaque,
            filter: Filter::Nearest,
            tint: Some(Rgba::WHITE),
        };
        let without_tint = Command::CopyRect {
            tex_id: 0,
            src: Rect::default(),
            dst: Rect::default(),
            blend: BlendMode::Opaque,
            filter: Filter::Nearest,
            tint: None,
        };
        assert_eq!(with_tint.length_words(), 6);
        assert_eq!(without_tint.length_words(), 5);
    }

    #[test]
    fn rgba_to_u32_byte_order() {
        // PROTOCOL.md §7.1: u32 layout is A:R:G:B MSB→LSB.
        let c = Rgba::new(0x11, 0x22, 0x33, 0x44);
        assert_eq!(c.to_u32(), 0x44_11_22_33);

        // When stored LE in memory, bytes should be B, G, R, A.
        let mut buf = [0u8; 4];
        write_u32_le(&mut buf, c.to_u32());
        assert_eq!(buf, [0x33, 0x22, 0x11, 0x44]);
    }

    #[test]
    fn encoded_len_matches_actual_bytes_written() {
        let cmds = [
            Command::Nop { padding_words: 7 },
            Command::Present,
            Command::Fence { value: 1 },
            Command::SetClip(Rect::new(0, 0, 100, 100)),
            Command::ClearClip,
            Command::FillRect {
                dst: Rect::new(0, 0, 10, 10),
                color: Rgba::WHITE,
                blend: BlendMode::Opaque,
                ignore_clip: false,
            },
            Command::CopyRect {
                tex_id: 1,
                src: Rect::default(),
                dst: Rect::default(),
                blend: BlendMode::Opaque,
                filter: Filter::Nearest,
                tint: None,
            },
            Command::CopyRect {
                tex_id: 1,
                src: Rect::default(),
                dst: Rect::default(),
                blend: BlendMode::Opaque,
                filter: Filter::Nearest,
                tint: Some(Rgba::BLACK),
            },
        ];
        for cmd in &cmds {
            let bytes = enc(cmd);
            assert_eq!(bytes.len(), cmd.encoded_len(), "mismatch for {:?}", cmd);
            assert_eq!(bytes.len() % 4, 0, "not 4-byte aligned: {:?}", cmd);
        }
    }

    #[test]
    fn encode_returns_err_on_short_buffer() {
        let cmd = Command::Present;
        let mut small = [0u8; 2];
        let r = cmd.encode(&mut small);
        assert_eq!(r, Err(EncodeError::BufferTooSmall { needed: 4, have: 2 }));
    }
}
