//! Paint a [`Tree`] into a `menu_core_host::Frame`.
//!
//! N4.5: text nodes blit a cached pre-rendered RT instead of issuing
//! one COPY_RECT per glyph. The caching is driven by
//! [`crate::text::TextCache`] and renders happen once per
//! `(content, font, size, color)` tuple via [`render_pending_text`].

use std::collections::HashMap;

use menu_core_host::device::FramebufferConfig;
use menu_core_host::error::DeviceError;
use menu_core_host::frame::{CopyOpts, Frame};
use menu_core_host::protocol::{BlendMode, Filter, Rect, Rgba};

use crate::font::FontRegistry;
use crate::image::{CachedImage, ImageRegistry};
use crate::layout::ComputedLayout;
use crate::text::{CacheKey, PendingRender, ResolvedTextStyle, TextCache};
use crate::vdom::{NodeId, NodeKind, Tree};

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

/// Walk the tree and paint each node into `frame`.
pub fn paint<'a>(
    tree: &Tree,
    root: NodeId,
    fb: &FramebufferConfig,
    layouts: &HashMap<NodeId, ComputedLayout>,
    text_styles: &HashMap<NodeId, ResolvedTextStyle>,
    text_cache: &TextCache,
    images: &ImageRegistry,
    mut frame: Frame<'a>,
) -> Result<Frame<'a>, DeviceError> {
    // Background clear.
    let root_bg = tree
        .get(root)
        .and_then(|n| n.style.background_color)
        .unwrap_or(Rgba::BLACK);
    frame = frame.fill_rect_unclipped(
        Rect::new(0, 0, fb.width, fb.height),
        root_bg,
        BlendMode::Opaque,
    )?;

    frame = paint_subtree(
        tree,
        root,
        layouts,
        text_styles,
        text_cache,
        images,
        frame,
        /* skip_root_bg */ true,
    )?;
    Ok(frame)
}

fn paint_subtree<'a>(
    tree: &Tree,
    id: NodeId,
    layouts: &HashMap<NodeId, ComputedLayout>,
    text_styles: &HashMap<NodeId, ResolvedTextStyle>,
    text_cache: &TextCache,
    images: &ImageRegistry,
    mut frame: Frame<'a>,
    skip_root_bg: bool,
) -> Result<Frame<'a>, DeviceError> {
    let Some(node) = tree.get(id) else {
        return Ok(frame);
    };
    let Some(lay) = layouts.get(&id) else {
        return Ok(frame);
    };

    match &node.kind {
        NodeKind::Div => {
            if !skip_root_bg
                && let Some(color) = node.style.background_color
                && lay.w > 0.5
                && lay.h > 0.5
            {
                let dx = clamp_u16(lay.x);
                let dy = clamp_u16(lay.y);
                let dw = clamp_u16(lay.w);
                let dh = clamp_u16(lay.h);
                if dw > 0 && dh > 0 {
                    frame = frame.fill_rect_unclipped(
                        Rect::new(dx, dy, dw, dh),
                        color,
                        BlendMode::Opaque,
                    )?;
                }
            }
        }
        NodeKind::Text { content } => {
            frame = paint_text(content, id, lay, text_styles, text_cache, frame)?;
        }
        NodeKind::Img { src } => {
            frame = paint_img(src, lay, images, frame)?;
        }
    }

    for &child in &node.children {
        frame = paint_subtree(tree, child, layouts, text_styles, text_cache, images, frame, false)?;
    }
    Ok(frame)
}

fn paint_img<'a>(
    src: &str,
    lay: &ComputedLayout,
    images: &ImageRegistry,
    frame: Frame<'a>,
) -> Result<Frame<'a>, DeviceError> {
    let cached = match images.get(src) {
        Some(CachedImage::Loaded { texture, width, height }) => (*texture, *width, *height),
        _ => return Ok(frame),
    };
    let (texture, src_w, src_h) = cached;
    if src_w == 0 || src_h == 0 || lay.w < 0.5 || lay.h < 0.5 {
        return Ok(frame);
    }
    let dst = Rect::new(
        clamp_u16(lay.x),
        clamp_u16(lay.y),
        clamp_u16(lay.w),
        clamp_u16(lay.h),
    );
    frame.copy_rect(
        &texture,
        Rect::new(0, 0, src_w, src_h),
        dst,
        CopyOpts {
            // Use SrcAlpha so PNGs with transparency composite over
            // whatever's behind them. Fully-opaque images degrade
            // gracefully (alpha=255 → out = src).
            blend: BlendMode::SrcAlpha,
            filter: Filter::Nearest,
            tint: None,
        },
    )
}

fn paint_text<'a>(
    content: &str,
    id: NodeId,
    lay: &ComputedLayout,
    text_styles: &HashMap<NodeId, ResolvedTextStyle>,
    text_cache: &TextCache,
    frame: Frame<'a>,
) -> Result<Frame<'a>, DeviceError> {
    let Some(rs) = text_styles.get(&id) else {
        return Ok(frame);
    };
    let key = CacheKey {
        content: content.to_string(),
        font_name: rs.font_name.clone(),
        px_size: rs.px_size.round() as u16,
        color: rs.color.to_u32(),
    };
    let Some(cached) = text_cache.lookup(&key) else {
        // Should have been populated before paint. Skip silently.
        return Ok(frame);
    };
    if cached.width == 0 || cached.height == 0 {
        return Ok(frame);
    }
    // Round dst.x down to a 16-pixel multiple so the framebuffer write
    // address is 64-byte aligned at cur_x=0. This lets the FPGA blit
    // engine engage its 16-pixel burst tier from the start of every
    // text row instead of bottoming out at smaller bursts. Visual
    // shift is at most 15 px left of where Taffy placed the text;
    // imperceptible at our typical sizes.
    let raw_x = clamp_u16(lay.x);
    let aligned_x = raw_x & !15;
    let dst = Rect::new(
        aligned_x,
        clamp_u16(lay.y),
        cached.width,
        cached.height,
    );
    frame.copy_rect(
        &cached.texture,
        Rect::new(0, 0, cached.width, cached.height),
        dst,
        CopyOpts {
            // Color baked into the RT; SrcAlpha so transparent areas
            // outside the glyphs don't overwrite the framebuffer.
            blend: BlendMode::SrcAlpha,
            filter: Filter::Nearest,
            tint: None,
        },
    )
}

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
