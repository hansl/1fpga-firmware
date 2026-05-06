//! TTF rasterisation and A8 atlas building.
//!
//! Identical algorithm to the one in `menu-core/src/text.rs` (the
//! existing `text-test` / `text-anim` host-side helper) — relocated
//! here so the `menu-ui` runtime owns its own font pipeline. To
//! render a string, paint emits one `COPY_RECT` per character with
//! `src` = the glyph's atlas rect, A8 format, the desired colour as
//! the tint, and `SrcAlpha` blend.
//!
//! Layout: simple shelf packing — glyphs flow left-to-right; when a
//! row fills, we move down by the tallest glyph in that row + 1 px
//! padding.

use std::collections::{HashMap, HashSet};

use fontdue::{Font, FontSettings};

#[derive(Debug, thiserror::Error)]
pub enum AtlasError {
    #[error("font parse failed: {0}")]
    FontParse(&'static str),
    #[error("atlas would exceed {atlas_w}×{atlas_h} cap with the requested charset")]
    AtlasOverflow { atlas_w: u32, atlas_h: u32 },
}

/// Per-glyph information stored alongside the atlas.
#[derive(Debug, Clone, Copy)]
pub struct GlyphInfo {
    pub atlas_x: u16,
    pub atlas_y: u16,
    pub width: u16,
    pub height: u16,
    pub bearing_x: i16,
    /// Distance from baseline to the glyph's bottom (positive = above
    /// baseline, negative = descender).
    pub ymin: i16,
    pub advance: i16,
}

/// A rasterised glyph atlas plus per-character metrics.
pub struct FontAtlas {
    pub bytes: Vec<u8>,
    pub width: u16,
    pub height: u16,
    pub line_height: u16,
    pub ascent: u16,
    /// Map from character to its rasterised glyph entry. Codepoints
    /// for which the font has no real glyph are NOT inserted here —
    /// `glyph()` falls back to `'?'` for them.
    glyphs: HashMap<char, GlyphInfo>,
    /// Every character that was *requested* during the build,
    /// regardless of whether the font produced a real glyph for it.
    /// Distinct from `glyphs.keys()` so the registry's rebuild check
    /// (`has_glyph`) doesn't treat font-missing chars as "still
    /// needed" and rebuild every frame.
    requested: HashSet<char>,
}

impl FontAtlas {
    /// Look up the glyph info for `ch`, or fall back to `'?'` if the
    /// codepoint isn't in the atlas.
    pub fn glyph(&self, ch: char) -> Option<&GlyphInfo> {
        self.glyphs.get(&ch).or_else(|| self.glyphs.get(&'?'))
    }

    /// Returns `true` if `ch` was considered when this atlas was
    /// built — whether or not the font produced a real glyph. The
    /// registry uses this to decide if a rebuild is needed when new
    /// text appears with a character not yet in the atlas.
    pub fn has_glyph(&self, ch: char) -> bool {
        self.requested.contains(&ch)
    }

    /// Iterate every char this atlas was built with. The registry
    /// uses this on a rebuild to preserve everything the prior atlas
    /// covered.
    pub fn requested_chars(&self) -> impl Iterator<Item = char> + '_ {
        self.requested.iter().copied()
    }

    /// Total advance width of `text` if rendered with this atlas.
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
/// pack the glyphs into a fixed-width A8 atlas (height grows up to
/// `atlas_h_cap`).
pub fn build_atlas(
    font_bytes: &[u8],
    px_size: f32,
    charset: &str,
    atlas_w: u32,
    atlas_h_cap: u32,
) -> Result<FontAtlas, AtlasError> {
    let font =
        Font::from_bytes(font_bytes, FontSettings::default()).map_err(AtlasError::FontParse)?;

    let line_metrics = font.horizontal_line_metrics(px_size).unwrap_or_else(|| {
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
    let mut requested = HashSet::new();

    for ch in charset.chars() {
        // Track every requested char so the registry's rebuild check
        // doesn't treat font-missing chars as "still needed".
        requested.insert(ch);

        // Skip codepoints the font doesn't have a real glyph for.
        // fontdue returns the `.notdef` (empty box) glyph for missing
        // chars, which we'd otherwise insert into the atlas as if it
        // were a real entry — `glyph()` would then return that box
        // instead of falling back to `?`. `lookup_glyph_index` returns
        // 0 for missing chars (per OpenType convention).
        if ch != '\u{0}' && font.lookup_glyph_index(ch) == 0 {
            continue;
        }
        let (metrics, bitmap) = font.rasterize(ch, px_size);
        let gw = metrics.width as u32;
        let gh = metrics.height as u32;

        if row_x + gw + 1 > atlas_w {
            row_y += row_h + 1;
            row_x = 0;
            row_h = 0;
        }

        let need_h = row_y + gh + 1;
        if need_h > atlas_h_cap {
            return Err(AtlasError::AtlasOverflow {
                atlas_w,
                atlas_h: atlas_h_cap,
            });
        }
        if need_h > height {
            height = need_h;
            bytes.resize((atlas_w * height) as usize, 0);
        }

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
        requested,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_FONT: &[u8] = include_bytes!("../../../menu-core/fonts/NotoSans-Regular.ttf");

    #[test]
    fn builds_an_ascii_atlas() {
        let charset: String = (b' '..=b'~').map(|b| b as char).collect();
        let atlas = build_atlas(TEST_FONT, 24.0, &charset, 512, 512).unwrap();
        assert!(atlas.width == 512);
        assert!(atlas.height > 0 && atlas.height <= 512);
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
}
