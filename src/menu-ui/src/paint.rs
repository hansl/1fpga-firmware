//! Paint a [`Tree`] into a `menu_core_host::Frame`.
//!
//! N4.5: text nodes blit a cached pre-rendered RT instead of issuing
//! one COPY_RECT per glyph. The caching is driven by
//! [`crate::text::TextCache`] and renders happen once per
//! `(content, font, size, color)` tuple via [`render_pending_text`].

use menu_core_host::error::DeviceError;
use menu_core_host::frame::{CopyOpts, Frame};
use menu_core_host::protocol::{BlendMode, Filter, Rect, Rgba};

use crate::font::FontRegistry;
use crate::text::PendingRender;

/// Render every [`PendingRender`] into its allocated render-target
/// texture. Called inside a [`Frame`] after `begin_frame` and before
/// `paint`. Restores the active target to the framebuffer on exit.
pub fn render_pending_text<'a>(
    mut frame: Frame<'a>,
    pendings: &[PendingRender],
    fonts: &FontRegistry,
) -> Result<Frame<'a>, DeviceError> {
    if pendings.is_empty() {
        return Ok(frame);
    }
    for p in pendings {
        let Some(cached) = fonts.get(&p.font_name, p.px_size) else {
            continue;
        };
        let atlas = &cached.atlas;
        let glyph_tex = &cached.texture;

        // Switch destination to this RT and clear it transparent.
        frame = frame.set_target(&p.texture)?;
        frame = frame.fill_rect_unclipped(
            Rect::new(0, 0, p.width, p.height),
            Rgba::TRANSPARENT,
            BlendMode::Opaque,
        )?;

        // Walk glyphs; the RT is exactly the text's bounding box, so
        // the pen starts at (0, baseline=ascent).
        let baseline_y = atlas.ascent as i32;
        let mut pen_x: i32 = 0;
        for ch in p.content.chars() {
            let g = match atlas.glyph(ch) {
                Some(g) => *g,
                None => continue,
            };
            if g.width > 0 && g.height > 0 {
                let dst_x = pen_x + g.bearing_x as i32;
                let dst_y = baseline_y - g.ymin as i32 - g.height as i32;
                if dst_x >= 0 && dst_y >= 0 {
                    frame = frame.copy_rect(
                        glyph_tex,
                        Rect::new(g.atlas_x, g.atlas_y, g.width, g.height),
                        Rect::new(
                            clamp_u16(dst_x as f32),
                            clamp_u16(dst_y as f32),
                            g.width,
                            g.height,
                        ),
                        CopyOpts {
                            blend: BlendMode::SrcAlpha,
                            filter: Filter::Nearest,
                            tint: Some(p.color),
                        },
                    )?;
                }
            }
            pen_x += g.advance as i32;
        }
    }
    // Back to the framebuffer for the main paint pass.
    frame.set_target_framebuffer()
}

/// (Tree-walk painting moved to `crate::display_list`: `build()` makes
/// one pass resolving draw ops + damage items together, `replay()`
/// emits them. This module keeps only the glyph-RT renderer, which is
/// device-coupled (allocates/fills render targets) and will live on
/// the engine thread after the dual-core split.)

#[inline]
fn clamp_u16(v: f32) -> u16 {
    if v <= 0.0 {
        0
    } else if v >= u16::MAX as f32 {
        u16::MAX
    } else {
        v as u16
    }
}
