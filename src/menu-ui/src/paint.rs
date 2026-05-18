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
use crate::style::Transform;
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
    opacities: &HashMap<NodeId, f32>,
    transforms: &HashMap<NodeId, Transform>,
    mut frame: Frame<'a>,
) -> Result<Frame<'a>, DeviceError> {
    // Background clear. Use the clip-respecting variant so damage
    // painting (which sets a user clip before calling us) only writes
    // pixels inside that clip; with no clip active the effective
    // bounds are the full FB anyway, so the full-paint case is
    // unchanged. Root bg ignores opacity intentionally — root is the
    // bottom of the stack; "opacity 0" on root would expose whatever
    // sat in the FB from a previous frame, which is meaningless for
    // a menu canvas.
    let root_bg = tree
        .get(root)
        .and_then(|n| n.style.background_color)
        .unwrap_or(Rgba::BLACK);
    frame = frame.fill_rect(
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
        opacities,
        transforms,
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
    opacities: &HashMap<NodeId, f32>,
    transforms: &HashMap<NodeId, Transform>,
    mut frame: Frame<'a>,
    skip_root_bg: bool,
) -> Result<Frame<'a>, DeviceError> {
    let Some(node) = tree.get(id) else {
        return Ok(frame);
    };
    let Some(lay) = layouts.get(&id) else {
        return Ok(frame);
    };

    let opacity = opacities.get(&id).copied().unwrap_or(1.0);
    let opacity_u8 = opacity_to_u8(opacity);

    // Fully transparent: nothing to draw, nothing to recurse into
    // (children inherit multiplicatively so they're all 0 too).
    if opacity_u8 == 0 {
        return Ok(frame);
    }

    let xf = transforms.get(&id).copied().unwrap_or(Transform::IDENTITY);
    // Scaled = produce a different-sized dst rect than src. The FPGA
    // routes COPY_RECT with `src_w != dst_w || src_h != dst_h` into
    // its nearest-neighbour path; FILL_RECT only uses dst, so scale
    // just changes the rect size.
    let scaled = !xf.is_identity();

    match &node.kind {
        NodeKind::Div => {
            if !skip_root_bg
                && let Some(color) = node.style.background_color
                && lay.w > 0.5
                && lay.h > 0.5
            {
                let (tx, ty, tw, th) = xf.apply_to_rect(lay.x, lay.y, lay.w, lay.h);
                let dx = clamp_u16(tx);
                let dy = clamp_u16(ty);
                let dw = clamp_u16(tw);
                let dh = clamp_u16(th);
                if dw > 0 && dh > 0 {
                    // Clip-respecting: paint::paint may be called
                    // under a damage-rect clip; outside that clip the
                    // blit engine zeroes eff_w/eff_h and skips work.
                    let (effective_color, blend) = apply_opacity_to_color(color, opacity_u8);
                    frame = frame.fill_rect(
                        Rect::new(dx, dy, dw, dh),
                        effective_color,
                        blend,
                    )?;
                }
            }
        }
        NodeKind::Text { content } => {
            frame = paint_text(content, id, lay, text_styles, text_cache, opacity_u8, xf, scaled, frame)?;
        }
        NodeKind::Img { src } => {
            frame = paint_img(src, lay, images, opacity_u8, xf, frame)?;
        }
    }

    for &child in &node.children {
        frame = paint_subtree(tree, child, layouts, text_styles, text_cache, images, opacities, transforms, frame, false)?;
    }
    Ok(frame)
}

fn paint_img<'a>(
    src: &str,
    lay: &ComputedLayout,
    images: &ImageRegistry,
    opacity_u8: u8,
    xf: Transform,
    frame: Frame<'a>,
) -> Result<Frame<'a>, DeviceError> {
    let (texture, src_w, src_h, fully_opaque) = match images.get(src) {
        Some(CachedImage::Loaded { texture, width, height, fully_opaque }) => {
            (*texture, *width, *height, *fully_opaque)
        }
        _ => return Ok(frame),
    };
    if src_w == 0 || src_h == 0 || lay.w < 0.5 || lay.h < 0.5 {
        return Ok(frame);
    }
    let (tx, ty, tw, th) = xf.apply_to_rect(lay.x, lay.y, lay.w, lay.h);
    let dst = Rect::new(
        clamp_u16(tx),
        clamp_u16(ty),
        clamp_u16(tw),
        clamp_u16(th),
    );
    // Pick the cheapest correct blend. Opaque is write-only; SrcAlpha
    // needs a dst read + math + write. For a fully-opaque image
    // rendered at opacity 1.0 the two produce identical pixels, but
    // Opaque has half the DDR traffic — load-bearing for the
    // full-screen wallpaper (1920×1080 → ~14 ms vs ~28 ms per paint).
    let blend = if fully_opaque && opacity_u8 == 0xFF {
        BlendMode::Opaque
    } else {
        BlendMode::SrcAlpha
    };
    frame.copy_rect(
        &texture,
        Rect::new(0, 0, src_w, src_h),
        dst,
        CopyOpts {
            blend,
            filter: Filter::Nearest,
            tint: opacity_tint(opacity_u8),
        },
    )
}

fn paint_text<'a>(
    content: &str,
    id: NodeId,
    lay: &ComputedLayout,
    text_styles: &HashMap<NodeId, ResolvedTextStyle>,
    text_cache: &TextCache,
    opacity_u8: u8,
    xf: Transform,
    scaled: bool,
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
    // Transform the text's layout rect, then size dst to the
    // transformed dimensions. In identity (non-scaled) mode this is
    // a no-op and we still get the burst-friendly 16-px x alignment
    // (the FPGA's 1:1 path). In scaled mode we let dst differ from
    // src so the FPGA's nearest-neighbour scaled path engages — the
    // alignment optimisation doesn't apply there anyway (scaled
    // copy reads per pixel, no bursts).
    let (tx, ty, tw, th) = xf.apply_to_rect(
        lay.x,
        lay.y,
        cached.width as f32,
        cached.height as f32,
    );
    let dst_x = if scaled {
        clamp_u16(tx)
    } else {
        // Round dst.x down to a 16-pixel multiple so the framebuffer
        // write address is 64-byte aligned at cur_x=0 — engages the
        // engine's 16-pixel burst tier from the start of every row.
        // Visual shift is at most 15 px left of where Taffy placed
        // the text; imperceptible at our typical sizes.
        clamp_u16(tx) & !15
    };
    let dst = Rect::new(
        dst_x,
        clamp_u16(ty),
        clamp_u16(tw).max(1),
        clamp_u16(th).max(1),
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
            tint: opacity_tint(opacity_u8),
        },
    )
}

#[inline]
fn opacity_to_u8(opacity: f32) -> u8 {
    (opacity.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// Build a tint that the blit engine will multiply against an
/// already-tinted texture (text RT, image) to scale its contribution
/// by `opacity_u8 / 255`. `None` short-circuits the engine's tint
/// path entirely when opacity is fully opaque.
///
/// All four channels are set to `opacity_u8` because the engine's
/// SrcAlpha uses the premultiplied formula
///     `out = src.RGB + dst.RGB · (1 − src.A/255)`
/// and the A8 path bakes RGB = tint.RGB · glyph_alpha into the text
/// RT — so for premultiplied src we must dim RGB *and* alpha to get
/// the visual effect of "less src contribution". Tinting only the
/// alpha leaves the RGB at full strength and produces hollow text.
/// The same tint works for non-premultiplied images because dimming
/// both their RGB (linear factor) and alpha (linear factor in the
/// (1−src.A) dst term) gives the expected `src·op + dst·(1−op)`
/// blend at opaque pixels.
#[inline]
fn opacity_tint(opacity_u8: u8) -> Option<Rgba> {
    if opacity_u8 == 0xFF {
        None
    } else {
        Some(Rgba::new(opacity_u8, opacity_u8, opacity_u8, opacity_u8))
    }
}

/// Apply opacity to a solid fill colour, returning the colour to use
/// and the blend mode required to render it correctly. When fully
/// opaque we stay on the fast Opaque path; otherwise we switch to
/// SrcAlpha. The engine's SrcAlpha is premultiplied, so RGB must be
/// pre-scaled by the *effective* alpha (color.a · opacity / 255²):
///     out = src.RGB + dst.RGB · (1 − src.A/255)
/// With unpremultiplied src this gives `src + dst·(1−A)` which
/// over-brightens by `src·(1−A)` and produces a too-light fill.
#[inline]
fn apply_opacity_to_color(color: Rgba, opacity_u8: u8) -> (Rgba, BlendMode) {
    if opacity_u8 == 0xFF {
        (color, BlendMode::Opaque)
    } else {
        let eff_a = ((color.a as u16) * (opacity_u8 as u16) / 255) as u8;
        let pre = |c: u8| ((c as u16) * (eff_a as u16) / 255) as u8;
        (
            Rgba::new(pre(color.r), pre(color.g), pre(color.b), eff_a),
            BlendMode::SrcAlpha,
        )
    }
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
