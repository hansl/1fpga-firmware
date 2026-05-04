//! TTF rasterization and font atlas building.
//!
//! Glyphs are rasterized once (at startup or on first use) into a
//! single A8 atlas texture that lives in the texture pool. To render a
//! string, the host emits one [`COPY_RECT`] per character with `src` =
//! the glyph's rect within the atlas, `dst` = the pen position offset
//! by the glyph's bearing, A8 format, the desired colour as the tint,
//! and `SrcAlpha` blend.
//!
//! The atlas uses a simple shelf packing layout: glyphs flow
//! left-to-right; when a row fills, we move down by the tallest glyph
//! in that row plus 1 px padding.

use std::collections::HashMap;

use fontdue::{Font, FontSettings};

#[derive(Debug, thiserror::Error)]
pub enum TextError {
    #[error("font parse failed: {0}")]
    FontParse(&'static str),
    #[error("atlas would exceed {atlas_w}×{atlas_h} cap with the requested charset")]
    AtlasOverflow { atlas_w: u32, atlas_h: u32 },
}

/// Per-glyph information stored alongside the atlas.
#[derive(Debug, Clone, Copy)]
pub struct GlyphInfo {
    /// Position of the glyph's top-left within the atlas.
    pub atlas_x: u16,
    pub atlas_y: u16,
    /// Glyph dimensions in pixels.
    pub width: u16,
    pub height: u16,
    /// Pen-relative offset for the glyph's top-left when rendered.
    /// Positive `bearing_x` shifts right of the pen; positive
    /// `bearing_y` is downward (we use a top-left coordinate system,
    /// so a glyph's top is `pen.y + (line_height - bearing_y)`).
    pub bearing_x: i16,
    pub ymin: i16,
    /// Horizontal pen advance to the next glyph.
    pub advance: i16,
}

/// A rasterised glyph atlas plus per-character metrics.
pub struct FontAtlas {
    /// A8 pixel data, row-major. Length = `width * height`.
    pub bytes: Vec<u8>,
    pub width: u16,
    pub height: u16,
    /// Recommended line height (font's full vertical advance, in
    /// pixels) for laying out multiline text.
    pub line_height: u16,
    /// Maximum ascender height in the rasterised set, used to position
    /// the pen on a baseline-anchored coordinate system.
    pub ascent: u16,
    glyphs: HashMap<char, GlyphInfo>,
}

impl FontAtlas {
    /// Look up the glyph info for `ch`, or fall back to the entry for
    /// `'?'` if the codepoint isn't in the atlas.
    pub fn glyph(&self, ch: char) -> Option<&GlyphInfo> {
        self.glyphs.get(&ch).or_else(|| self.glyphs.get(&'?'))
    }

    /// Total advance width of `text` if rendered with this atlas. Useful
    /// for centering / right-aligning.
    pub fn measure(&self, text: &str) -> u32 {
        let mut total: i32 = 0;
        for ch in text.chars() {
            if let Some(g) = self.glyph(ch) {
                total += g.advance as i32;
            }
        }
        total.max(0) as u32
    }
}

/// Rasterise `font_bytes` at `px_size` for every char in `charset`,
/// pack the glyphs into a power-of-two-width A8 atlas, and return it.
///
/// `atlas_w` is the target atlas width in pixels; the height grows as
/// needed up to `atlas_h_cap`.
pub fn build_atlas(
    font_bytes: &[u8],
    px_size: f32,
    charset: &str,
    atlas_w: u32,
    atlas_h_cap: u32,
) -> Result<FontAtlas, TextError> {
    let font = Font::from_bytes(font_bytes, FontSettings::default()).map_err(TextError::FontParse)?;

    let line_metrics = font.horizontal_line_metrics(px_size).unwrap_or_else(|| {
        // Fallback for fonts without horizontal line metrics.
        fontdue::LineMetrics {
            ascent: px_size,
            descent: 0.0,
            line_gap: 0.0,
            new_line_size: px_size,
        }
    });
    let ascent = line_metrics.ascent.ceil().max(0.0) as u16;
    let line_height = line_metrics.new_line_size.ceil().max(px_size as f64 as f32) as u16;

    let mut bytes: Vec<u8> = Vec::new();
    let mut height: u32 = 0;
    let mut row_y: u32 = 0;
    let mut row_x: u32 = 0;
    let mut row_h: u32 = 0;

    let mut glyphs = HashMap::new();

    for ch in charset.chars() {
        let (metrics, bitmap) = font.rasterize(ch, px_size);
        let gw = metrics.width as u32;
        let gh = metrics.height as u32;

        // Place into atlas with 1px padding so neighbouring glyphs
        // don't bleed into each other under nearest-neighbour sampling.
        if row_x + gw + 1 > atlas_w {
            row_y += row_h + 1;
            row_x = 0;
            row_h = 0;
        }

        let need_h = row_y + gh + 1;
        if need_h > atlas_h_cap {
            return Err(TextError::AtlasOverflow {
                atlas_w,
                atlas_h: atlas_h_cap,
            });
        }
        if need_h > height {
            height = need_h;
            bytes.resize((atlas_w * height) as usize, 0);
        }

        // Blit the glyph bitmap into the atlas at (row_x, row_y).
        for gy in 0..gh {
            let dst_off = ((row_y + gy) * atlas_w + row_x) as usize;
            let src_off = (gy * gw) as usize;
            bytes[dst_off..dst_off + gw as usize]
                .copy_from_slice(&bitmap[src_off..src_off + gw as usize]);
        }

        glyphs.insert(
            ch,
            GlyphInfo {
                atlas_x: row_x as u16,
                atlas_y: row_y as u16,
                width: gw as u16,
                height: gh as u16,
                bearing_x: metrics.xmin as i16,
                ymin: metrics.ymin as i16,
                advance: metrics.advance_width.round() as i16,
            },
        );

        row_x += gw + 1;
        if gh > row_h {
            row_h = gh;
        }
    }

    Ok(FontAtlas {
        bytes,
        width: atlas_w as u16,
        height: height as u16,
        line_height,
        ascent,
        glyphs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_FONT: &[u8] = include_bytes!("../fonts/NotoSans-Regular.ttf");

    #[test]
    fn builds_an_ascii_atlas() {
        let charset: String = (b' '..=b'~').map(|b| b as char).collect();
        let atlas = build_atlas(TEST_FONT, 24.0, &charset, 512, 512).unwrap();
        assert!(atlas.width == 512);
        assert!(atlas.height > 0 && atlas.height <= 512);
        // 'H', 'e', 'l', 'l', 'o', '!', '?' should all be present.
        for ch in "Hello!?".chars() {
            assert!(atlas.glyph(ch).is_some(), "missing glyph for {ch:?}");
        }
    }

    #[test]
    fn measure_matches_advances() {
        let atlas = build_atlas(TEST_FONT, 24.0, " HelloWorld", 512, 512).unwrap();
        let total: u32 = "Hello"
            .chars()
            .map(|c| atlas.glyph(c).unwrap().advance as u32)
            .sum();
        assert_eq!(atlas.measure("Hello"), total);
    }

    #[test]
    fn glyph_falls_back_to_question_mark() {
        let charset: String = (b' '..=b'~').map(|b| b as char).collect();
        let atlas = build_atlas(TEST_FONT, 16.0, &charset, 256, 256).unwrap();
        // Unicode codepoint not in the Latin-only font.
        let unknown = '日';
        assert!(!atlas.glyphs.contains_key(&unknown));
        let q = atlas.glyph('?').unwrap().atlas_x;
        assert_eq!(atlas.glyph(unknown).unwrap().atlas_x, q);
    }
}
