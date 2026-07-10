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
use menu_core_host::protocol::{BlendMode, Filter, Rect, Rgba, affine::MAX_SRC_DIM};

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

/// Walk the tree and paint each node into `frame`. `clip` is the
/// damage-paint clip rect currently active on the frame (i.e., the
/// argument the caller passed to `Frame::set_clip`). When provided,
/// the walker skips issuing blit ops for any node whose transformed
/// bbox doesn't intersect it — saving the per-op FPGA dispatch +
/// DDR3 arbitration cost for nodes that would have produced zero
/// pixels anyway. This is a host-side cull; the engine's `dst ∩ clip`
/// check would also zero them out, but only after the op has been
/// fetched and decoded by the ring fetcher.
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
    clip: Option<Rect>,
    compositing: bool,
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
    // Skip the root background fill when the bottom layer (root's first
    // child) is an opaque, full-FB image — i.e. a wallpaper that will
    // overwrite these exact pixels anyway. Otherwise the fill is a
    // wasted opaque write over every damage rect, on top of which the
    // wallpaper copy then writes again. Falls back to filling when
    // there's no such guaranteed cover (transparent/partial bg).
    if compositing {
        // The content FB is the upper compositor layer over the hardware
        // wallpaper. Clear to transparent (premultiplied alpha 0) so the
        // wallpaper shows wherever there's no UI; the FPGA blends content
        // over it at scanout. This replaces the per-frame full-FB wallpaper
        // copy that was the menu-fps bottleneck.
        frame = frame.fill_rect(
            Rect::new(0, 0, fb.width, fb.height),
            Rgba::TRANSPARENT,
            BlendMode::Opaque,
        )?;
    } else if !has_opaque_covering_child(tree, root, layouts, images, opacities, transforms) {
        let root_bg = tree
            .get(root)
            .and_then(|n| n.style.background_color)
            .unwrap_or(Rgba::BLACK);
        frame = frame.fill_rect(
            Rect::new(0, 0, fb.width, fb.height),
            root_bg,
            BlendMode::Opaque,
        )?;
    }

    frame = paint_subtree(
        tree,
        root,
        layouts,
        text_styles,
        text_cache,
        images,
        opacities,
        transforms,
        clip,
        compositing,
        frame,
        /* skip_root_bg */ true,
    )?;
    Ok(frame)
}

/// True if `node` has a child that, when painted, opaquely covers the
/// node's entire layout box — an opaque-texture full-cover `<img>` or an
/// opaque-background full-cover `<div>`, at full opacity and
/// untransformed. Such a child is painted on top of the node's own
/// background, so that background fill is dead pixels and can be
/// skipped. Applied recursively per node, this peels off the stack of
/// redundant full-FB fills below an opaque wallpaper (container BLACK →
/// app `#0a0a14` → wallpaper) — each is covered by the next.
fn has_opaque_covering_child(
    tree: &Tree,
    node_id: NodeId,
    layouts: &HashMap<NodeId, ComputedLayout>,
    images: &ImageRegistry,
    opacities: &HashMap<NodeId, f32>,
    transforms: &HashMap<NodeId, Transform>,
) -> bool {
    let Some(node) = tree.get(node_id) else {
        return false;
    };
    let Some(nlay) = layouts.get(&node_id) else {
        return false;
    };
    for &cid in &node.children {
        let Some(child) = tree.get(cid) else {
            continue;
        };
        // Full opacity + untransformed, so the child's layout box is
        // exactly what it paints.
        if opacities.get(&cid).copied().unwrap_or(1.0) < 0.999 {
            continue;
        }
        let xf = transforms.get(&cid).copied().unwrap_or(Transform::IDENTITY);
        if !xf.is_identity() || xf.has_rotation() {
            continue;
        }
        let Some(clay) = layouts.get(&cid) else {
            continue;
        };
        // Child layout must contain the node's box.
        let covers = clay.x <= nlay.x + 0.5
            && clay.y <= nlay.y + 0.5
            && clay.x + clay.w >= nlay.x + nlay.w - 0.5
            && clay.y + clay.h >= nlay.y + nlay.h - 0.5;
        if !covers {
            continue;
        }
        let opaque = match &child.kind {
            NodeKind::Img { src } => matches!(
                images.get(src),
                Some(CachedImage::Loaded { fully_opaque: true, .. })
            ),
            NodeKind::Div => child
                .style
                .background_color
                .map(|c| c.a == 0xFF)
                .unwrap_or(false),
            NodeKind::Text { .. } => false,
        };
        if opaque {
            return true;
        }
    }
    false
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
    clip: Option<Rect>,
    // True when this subtree is painted directly over the pristine
    // transparent clear (compositing on, nothing opaque underneath yet).
    // Content over the clear can use Opaque (write-only copy) instead of
    // SrcAlpha — identical pixels for premultiplied src over a transparent
    // dst, but no per-pixel dst read (~half the DDR traffic). It flips to
    // false under any node that paints a background.
    dst_clear: bool,
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

    // Transformed bbox for this node, for clip culling. Computed
    // once; the leaf branches reuse the same rect for their actual
    // blit so the math doesn't repeat.
    let (tx, ty, tw, th) = xf.apply_to_rect(lay.x, lay.y, lay.w, lay.h);
    let dx = clamp_u16(tx);
    let dy = clamp_u16(ty);
    let dw = clamp_u16(tw);
    let dh = clamp_u16(th);

    match &node.kind {
        NodeKind::Div => {
            if !skip_root_bg
                && let Some(color) = node.style.background_color
                && lay.w > 0.5
                && lay.h > 0.5
                && dw > 0 && dh > 0
                && rect_intersects_clip(dx, dy, dw, dh, clip)
                // Skip the fill when an opaque child (the wallpaper, or
                // an opaque panel) covers this div — its fill would be
                // overwritten, so it's a wasted opaque write per rect.
                && !has_opaque_covering_child(tree, id, layouts, images, opacities, transforms)
            {
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
        NodeKind::Text { content } => {
            // Skip culling for scaled text — paint_text uses a
            // different dst sizing path; the no-cull case still
            // benefits from the engine's own clip rejection.
            if scaled || rect_intersects_clip(dx, dy, dw, dh, clip) {
                frame = paint_text(content, id, lay, text_styles, text_cache, opacity_u8, xf, scaled, dst_clear, frame)?;
            }
        }
        NodeKind::Img { src } => {
            // Cull against the rotated AABB (collapses to the scale box
            // when unrotated) so a rotated icon's corners aren't culled.
            let (ax, ay, aw, ah) = xf.apply_to_aabb(lay.x, lay.y, lay.w, lay.h);
            if rect_intersects_clip(clamp_u16(ax), clamp_u16(ay), clamp_u16(aw), clamp_u16(ah), clip) {
                frame = paint_img(src, lay, images, opacity_u8, xf, dst_clear, frame)?;
            }
        }
    }

    // Children are no longer over the pristine clear once this node has
    // painted a background of its own (e.g. the ActionBar's faint bar):
    // their content must SrcAlpha-blend over it, not copy over it.
    let child_dst_clear = dst_clear && node.style.background_color.is_none();
    for &child in &node.children {
        frame = paint_subtree(tree, child, layouts, text_styles, text_cache, images, opacities, transforms, clip, child_dst_clear, frame, false)?;
    }
    Ok(frame)
}

/// True when the rect `(x, y, w, h)` overlaps `clip`, or `clip` is
/// `None` (no clip → always paint). Pure host-side check used to
/// avoid issuing blit ops the engine would clip to zero anyway.
#[inline]
fn rect_intersects_clip(x: u16, y: u16, w: u16, h: u16, clip: Option<Rect>) -> bool {
    let Some(c) = clip else { return true; };
    if w == 0 || h == 0 || c.w == 0 || c.h == 0 {
        return false;
    }
    let ax2 = x as u32 + w as u32;
    let ay2 = y as u32 + h as u32;
    let bx2 = c.x as u32 + c.w as u32;
    let by2 = c.y as u32 + c.h as u32;
    ax2 > c.x as u32
        && bx2 > x as u32
        && ay2 > c.y as u32
        && by2 > y as u32
}

fn paint_img<'a>(
    src: &str,
    lay: &ComputedLayout,
    images: &ImageRegistry,
    opacity_u8: u8,
    xf: Transform,
    dst_clear: bool,
    frame: Frame<'a>,
) -> Result<Frame<'a>, DeviceError> {
    // Prefer a texture pre-resized to this node's base layout box
    // (built in prepare_images for explicitly-sized imgs): when the
    // node isn't transformed, dst == that box, so the blit is 1:1 and
    // skips the FPGA's nearest-neighbour scale path. Fall back to the
    // intrinsic texture for auto-sized imgs (and the variant won't help
    // mid-transform anyway, where dst differs from the base box).
    let base_w = clamp_u16(lay.w);
    let base_h = clamp_u16(lay.h);
    let cached = images
        .get_sized(src, base_w, base_h)
        .or_else(|| images.get(src));
    let (texture, src_w, src_h, fully_opaque) = match cached {
        Some(CachedImage::Loaded { texture, width, height, fully_opaque }) => {
            (*texture, *width, *height, *fully_opaque)
        }
        _ => return Ok(frame),
    };
    if src_w == 0 || src_h == 0 || lay.w < 0.5 || lay.h < 0.5 {
        return Ok(frame);
    }
    let (tx, ty, tw, th) = xf.apply_to_rect(lay.x, lay.y, lay.w, lay.h);

    // Rotated images render through the affine blit (rotation + scale,
    // bilinear). The staged source is capped at 128×128 (§5.7); larger
    // sources fall through to the unrotated nearest path below so the
    // UI degrades gracefully instead of erroring.
    if xf.has_rotation() && src_w <= MAX_SRC_DIM && src_h <= MAX_SRC_DIM {
        let cx = (tx + tw * 0.5).round() as i32;
        let cy = (ty + th * 0.5).round() as i32;
        // Uniform scale from the source to the (already scale-
        // transformed) display box. Menu icons are square; width drives
        // both axes.
        let scale = (tw / src_w as f32).max(0.0) as f64;
        // Affine v1 has no tint (RGBA-only), so opacity can't be applied
        // here. Over the transparent clear, Opaque copy is correct (the
        // rotated-out corners sample as transparent and copy over the
        // already-transparent FB) and avoids the per-pixel dst read;
        // otherwise SrcAlpha so the corners/edges composite cleanly.
        return frame.blit_affine_rotate(
            &texture,
            Rect::new(0, 0, src_w, src_h),
            xf.rotation as f64,
            scale,
            cx,
            cy,
            if dst_clear { BlendMode::Opaque } else { BlendMode::SrcAlpha },
        );
    }

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
    // Over the transparent clear, any premultiplied src can be copied
    // (Opaque, write-only) — same pixels as SrcAlpha but no dst read.
    let blend = if (dst_clear || fully_opaque) && opacity_u8 == 0xFF {
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
    // Retained for call-site symmetry; scaling is conveyed through
    // `xf`-derived tw/th (dst != src engages the FPGA's scaled path).
    _scaled: bool,
    dst_clear: bool,
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
    // No grid snapping: the blit engine's src realignment skid lets
    // bursts engage at any src/dst offset (dst self-aligns within one
    // pixel), so text can sit exactly where Taffy placed it and slide
    // animations move at 1-px granularity. (This used to round down
    // to a 16-px multiple to hit the old aligned-only burst tier.)
    let dst_x = clamp_u16(tx);
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
            // Color baked into the RT. Over the transparent clear the RT's
            // transparent padding copies transparent over an already-
            // transparent FB, so Opaque (write-only) is correct and skips
            // the per-pixel dst read; otherwise SrcAlpha so the padding
            // doesn't clobber what's underneath.
            blend: if dst_clear && opacity_u8 == 0xFF {
                BlendMode::Opaque
            } else {
                BlendMode::SrcAlpha
            },
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
