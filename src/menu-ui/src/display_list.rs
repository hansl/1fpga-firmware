//! Display list: a frame's paint plan as resolved, self-contained data.
//!
//! Built in ONE tree walk ([`build`]), consumed by [`replay`]. Each
//! entry carries both the [`PaintedItem`] the damage differ compares
//! and the resolved [`DrawOp`] the painter emits — derived from the
//! same math in the same place, so "what damage thinks was painted"
//! and "what paint writes" cannot diverge. (The previous split
//! walkers — `damage::compute_scene` + `paint::paint_subtree` — had
//! exactly that failure mode twice: the damage bbox for text kept
//! mirroring a 16-px dst snap after the painter dropped it, and it
//! sized text from the layout box while the painter sized it from
//! the cached RT.)
//!
//! Everything in a [`DisplayList`] is plain owned data: no tree
//! references, no `Rc`, no device handles except [`TextureHandle`]s
//! (which are POD ids). That makes it `Send` by construction — the
//! prerequisite for the planned dual-core split, where the Boa/UI
//! thread builds the list on one core and the engine thread replays
//! it into the command ring on the other.

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use menu_core_host::device::FramebufferConfig;
use menu_core_host::error::DeviceError;
use menu_core_host::frame::{CopyOpts, Frame};
use menu_core_host::protocol::{BlendMode, Filter, Rect, Rgba, affine::MAX_SRC_DIM};
use menu_core_host::texture::TextureHandle;

use crate::image::{CachedImage, ImageRegistry};
use crate::layout::ComputedLayout;
use crate::runtime::damage::{PaintedItem, PaintedScene, PixelRect};
use crate::style::Transform;
use crate::text::{CacheKey, ResolvedTextStyle, TextCache};
use crate::vdom::{NodeId, NodeKind, Tree};

/// One resolved draw command. All style/layout/opacity/transform
/// decisions (including blend selection and opacity tint) are baked
/// in at build time; replay is a dumb translator to `Frame` calls.
#[derive(Clone, Debug)]
pub enum DrawOp {
    /// Solid fill (Div background).
    Fill {
        rect: Rect,
        color: Rgba,
        blend: BlendMode,
    },
    /// 1:1 or nearest-scaled texture copy (images, text RTs).
    Copy {
        texture: TextureHandle,
        src: Rect,
        dst: Rect,
        blend: BlendMode,
        tint: Option<Rgba>,
    },
    /// Rotated + scaled image via the affine engine.
    AffineRotate {
        texture: TextureHandle,
        src: Rect,
        rotation_deg: f64,
        scale: f64,
        cx: i32,
        cy: i32,
        blend: BlendMode,
    },
}

/// A damage-tracked drawable. `op` is `None` when the node
/// contributes to damage but has nothing to draw this frame (e.g.
/// text whose RT wasn't in the cache yet — same "skip silently"
/// behaviour the old painter had, but the damage rect still exists
/// so the pixels repaint once the RT lands).
#[derive(Clone, Debug)]
pub struct Entry {
    pub item: PaintedItem,
    pub op: Option<DrawOp>,
}

/// The whole-frame background fill emitted before any entry:
/// the compositing transparent clear, or the root background color
/// (skipped when an opaque full-cover child would overwrite it).
#[derive(Clone, Copy, Debug)]
pub struct BackgroundFill {
    pub rect: Rect,
    pub color: Rgba,
}

#[derive(Clone, Debug, Default)]
pub struct DisplayList {
    pub background: Option<BackgroundFill>,
    pub entries: Vec<Entry>,
}

impl DisplayList {
    /// Damage-diff view of this list. Cheap (items are ~24 B each);
    /// stored per FB slot by the runtime.
    pub fn to_scene(&self) -> PaintedScene {
        PaintedScene {
            items: self.entries.iter().map(|e| e.item.clone()).collect(),
        }
    }

    /// Scene hash for the per-slot skip check, identical semantics to
    /// [`PaintedScene::hash`] without materialising the scene.
    pub fn scene_hash(&self) -> u64 {
        let mut h = DefaultHasher::new();
        for e in &self.entries {
            e.item.node_id.0.hash(&mut h);
            e.item.bbox.hash(&mut h);
            e.item.content_hash.hash(&mut h);
        }
        h.finish()
    }
}

/// Build the display list for the current tree state. The single
/// walk that replaces `damage::compute_scene` + `paint::paint`'s
/// tree recursion.
#[allow(clippy::too_many_arguments)]
pub fn build(
    tree: &Tree,
    root: NodeId,
    fb: &FramebufferConfig,
    layouts: &HashMap<NodeId, ComputedLayout>,
    text_styles: &HashMap<NodeId, ResolvedTextStyle>,
    text_cache: &TextCache,
    images: &ImageRegistry,
    opacities: &HashMap<NodeId, f32>,
    transforms: &HashMap<NodeId, Transform>,
    compositing: bool,
) -> DisplayList {
    // Whole-frame background, same policy as the old paint() prologue:
    // compositing clears the content layer transparent (the hardware
    // wallpaper shows through and the FPGA blends at scanout);
    // otherwise fill the root bg unless an opaque full-cover child
    // (the wallpaper image) would overwrite every pixel anyway.
    let background = if compositing {
        Some(BackgroundFill {
            rect: Rect::new(0, 0, fb.width, fb.height),
            color: Rgba::TRANSPARENT,
        })
    } else if !has_opaque_covering_child(tree, root, layouts, images, opacities, transforms) {
        let root_bg = tree
            .get(root)
            .and_then(|n| n.style.background_color)
            .unwrap_or(Rgba::BLACK);
        Some(BackgroundFill {
            rect: Rect::new(0, 0, fb.width, fb.height),
            color: root_bg,
        })
    } else {
        None
    };

    let mut entries = Vec::new();
    walk(
        tree,
        root,
        layouts,
        text_styles,
        text_cache,
        images,
        opacities,
        transforms,
        // The compositing clear leaves a pristine transparent dst;
        // content over it can use Opaque copies (no per-pixel dst
        // read). A root bg fill is NOT transparent-pristine.
        compositing,
        /* skip_root_bg */ true,
        &mut entries,
    );

    DisplayList { background, entries }
}

#[allow(clippy::too_many_arguments)]
fn walk(
    tree: &Tree,
    id: NodeId,
    layouts: &HashMap<NodeId, ComputedLayout>,
    text_styles: &HashMap<NodeId, ResolvedTextStyle>,
    text_cache: &TextCache,
    images: &ImageRegistry,
    opacities: &HashMap<NodeId, f32>,
    transforms: &HashMap<NodeId, Transform>,
    dst_clear: bool,
    skip_root_bg: bool,
    out: &mut Vec<Entry>,
) {
    let Some(node) = tree.get(id) else {
        return;
    };
    let Some(lay) = layouts.get(&id) else {
        return;
    };

    let opacity_u8 = opacities
        .get(&id)
        .copied()
        .unwrap_or(1.0)
        .clamp(0.0, 1.0)
        .mul_add(255.0, 0.5) as u8;
    // Fully transparent: paints nothing, children inherit 0 — and it
    // contributes no damage item either (matches the old walkers).
    if opacity_u8 == 0 {
        return;
    }

    let xf = transforms.get(&id).copied().unwrap_or(Transform::IDENTITY);
    // Quantised transform components for the content hash — coarse
    // enough to ignore float jitter in tween tails, fine enough to
    // catch real animation steps. (Same policy as before.)
    let sx_q = (xf.scale_x.clamp(0.0, 64.0) * 1024.0).round() as i32;
    let sy_q = (xf.scale_y.clamp(0.0, 64.0) * 1024.0).round() as i32;
    let rot_q = (xf.rotation * 64.0).round() as i32;

    match &node.kind {
        NodeKind::Div => {
            if !skip_root_bg
                && let Some(color) = node.style.background_color
                && lay.w > 0.5
                && lay.h > 0.5
            {
                let (tx, ty, tw, th) = xf.apply_to_rect(lay.x, lay.y, lay.w, lay.h);
                let rect = Rect::new(clamp_u16(tx), clamp_u16(ty), clamp_u16(tw), clamp_u16(th));
                if rect.w > 0 && rect.h > 0 {
                    let bbox = PixelRect { x: rect.x, y: rect.y, w: rect.w, h: rect.h };
                    let mut h = DefaultHasher::new();
                    0u8.hash(&mut h); // tag
                    color.to_u32().hash(&mut h);
                    bbox.hash(&mut h);
                    opacity_u8.hash(&mut h);
                    // Skip the fill op (not the damage item) when an
                    // opaque child covers this div — the fill would
                    // be overwritten, so it's a wasted write.
                    let op = if has_opaque_covering_child(
                        tree, id, layouts, images, opacities, transforms,
                    ) {
                        None
                    } else {
                        let (effective_color, blend) =
                            apply_opacity_to_color(color, opacity_u8);
                        Some(DrawOp::Fill { rect, color: effective_color, blend })
                    };
                    out.push(Entry {
                        item: PaintedItem { node_id: id, bbox, content_hash: h.finish() },
                        op,
                    });
                }
            }
        }
        NodeKind::Text { content } => {
            if let Some(rs) = text_styles.get(&id) {
                let key = CacheKey {
                    content: content.to_string(),
                    font_name: rs.font_name.clone(),
                    px_size: rs.px_size.round() as u16,
                    color: rs.color.to_u32(),
                };
                // Size dst from the CACHED RT dimensions (that is what
                // gets blitted); fall back to the layout box for the
                // damage rect when the RT isn't rendered yet.
                let (op, bbox) = match text_cache.lookup(&key) {
                    Some(cached) if cached.width > 0 && cached.height > 0 => {
                        let (tx, ty, tw, th) = xf.apply_to_rect(
                            lay.x,
                            lay.y,
                            cached.width as f32,
                            cached.height as f32,
                        );
                        let dst = Rect::new(
                            clamp_u16(tx),
                            clamp_u16(ty),
                            clamp_u16(tw).max(1),
                            clamp_u16(th).max(1),
                        );
                        let bbox = PixelRect { x: dst.x, y: dst.y, w: dst.w, h: dst.h };
                        let op = DrawOp::Copy {
                            texture: cached.texture,
                            src: Rect::new(0, 0, cached.width, cached.height),
                            dst,
                            // Color is baked into the RT. Over the
                            // pristine transparent clear an Opaque
                            // copy is pixel-identical to SrcAlpha
                            // (premultiplied src over transparent)
                            // and skips the per-pixel dst read.
                            blend: if dst_clear && opacity_u8 == 0xFF {
                                BlendMode::Opaque
                            } else {
                                BlendMode::SrcAlpha
                            },
                            tint: opacity_tint(opacity_u8),
                        };
                        (Some(op), bbox)
                    }
                    _ => (None, transformed_bbox(lay, xf)),
                };
                if bbox.w > 0 && bbox.h > 0 {
                    let mut h = DefaultHasher::new();
                    1u8.hash(&mut h);
                    content.hash(&mut h);
                    rs.font_name.hash(&mut h);
                    (rs.px_size as u32).hash(&mut h);
                    rs.color.to_u32().hash(&mut h);
                    bbox.hash(&mut h);
                    opacity_u8.hash(&mut h);
                    sx_q.hash(&mut h);
                    sy_q.hash(&mut h);
                    out.push(Entry {
                        item: PaintedItem { node_id: id, bbox, content_hash: h.finish() },
                        op,
                    });
                }
            }
        }
        NodeKind::Img { src } => {
            let bbox = transformed_bbox(lay, xf);
            if bbox.w > 0 && bbox.h > 0 {
                let mut h = DefaultHasher::new();
                2u8.hash(&mut h);
                src.hash(&mut h);
                bbox.hash(&mut h);
                opacity_u8.hash(&mut h);
                sx_q.hash(&mut h);
                sy_q.hash(&mut h);
                rot_q.hash(&mut h);
                let op = build_img_op(src, lay, images, opacity_u8, xf, dst_clear);
                out.push(Entry {
                    item: PaintedItem { node_id: id, bbox, content_hash: h.finish() },
                    op,
                });
            }
        }
    }

    // Children painted over this node's own background must blend,
    // not copy — the pristine-clear guarantee ends here.
    let child_dst_clear = dst_clear && node.style.background_color.is_none();
    for &child in &node.children {
        walk(
            tree, child, layouts, text_styles, text_cache, images, opacities, transforms,
            child_dst_clear, false, out,
        );
    }
}

/// Resolve an `<img>` node to its draw op — same policy chain as the
/// old `paint_img`: pre-sized texture variant preferred, affine path
/// for rotation within the staging cap, Opaque fast path when the
/// pixels can't need blending.
fn build_img_op(
    src: &str,
    lay: &ComputedLayout,
    images: &ImageRegistry,
    opacity_u8: u8,
    xf: Transform,
    dst_clear: bool,
) -> Option<DrawOp> {
    let base_w = clamp_u16(lay.w);
    let base_h = clamp_u16(lay.h);
    let cached = images
        .get_sized(src, base_w, base_h)
        .or_else(|| images.get(src));
    let (texture, src_w, src_h, fully_opaque) = match cached {
        Some(CachedImage::Loaded { texture, width, height, fully_opaque }) => {
            (*texture, *width, *height, *fully_opaque)
        }
        _ => return None,
    };
    if src_w == 0 || src_h == 0 || lay.w < 0.5 || lay.h < 0.5 {
        return None;
    }
    let (tx, ty, tw, th) = xf.apply_to_rect(lay.x, lay.y, lay.w, lay.h);

    if xf.has_rotation() && src_w <= MAX_SRC_DIM && src_h <= MAX_SRC_DIM {
        let cx = (tx + tw * 0.5).round() as i32;
        let cy = (ty + th * 0.5).round() as i32;
        let scale = (tw / src_w as f32).max(0.0) as f64;
        return Some(DrawOp::AffineRotate {
            texture,
            src: Rect::new(0, 0, src_w, src_h),
            rotation_deg: xf.rotation as f64,
            scale,
            cx,
            cy,
            // Affine v1 has no tint, so opacity can't apply here; see
            // the old paint_img for the full rationale.
            blend: if dst_clear { BlendMode::Opaque } else { BlendMode::SrcAlpha },
        });
    }

    let dst = Rect::new(clamp_u16(tx), clamp_u16(ty), clamp_u16(tw), clamp_u16(th));
    let blend = if (dst_clear || fully_opaque) && opacity_u8 == 0xFF {
        BlendMode::Opaque
    } else {
        BlendMode::SrcAlpha
    };
    Some(DrawOp::Copy {
        texture,
        src: Rect::new(0, 0, src_w, src_h),
        dst,
        blend,
        tint: opacity_tint(opacity_u8),
    })
}

/// Replay the list into a frame, optionally restricted to a damage
/// clip rect. The caller is responsible for `set_clip`/`clear_clip`
/// around this call (as with the old `paint::paint`); `clip` here is
/// the same rect, used to cull ops host-side before they cost a ring
/// op + fetch + dispatch.
pub fn replay<'a>(
    dl: &DisplayList,
    clip: Option<Rect>,
    mut frame: Frame<'a>,
) -> Result<Frame<'a>, DeviceError> {
    if let Some(bg) = &dl.background {
        // fill_rect is clip-respecting on the engine side, so under a
        // damage clip this only writes the clipped area — same
        // behaviour as the old prologue.
        frame = frame.fill_rect(bg.rect, bg.color, BlendMode::Opaque)?;
    }
    for e in &dl.entries {
        let Some(op) = &e.op else { continue };
        if !bbox_intersects_clip(&e.item.bbox, clip) {
            continue;
        }
        frame = match op {
            DrawOp::Fill { rect, color, blend } => frame.fill_rect(*rect, *color, *blend)?,
            DrawOp::Copy { texture, src, dst, blend, tint } => frame.copy_rect(
                texture,
                *src,
                *dst,
                CopyOpts { blend: *blend, filter: Filter::Nearest, tint: *tint },
            )?,
            DrawOp::AffineRotate { texture, src, rotation_deg, scale, cx, cy, blend } => {
                frame.blit_affine_rotate(texture, *src, *rotation_deg, *scale, *cx, *cy, *blend)?
            }
        };
    }
    Ok(frame)
}

#[inline]
fn bbox_intersects_clip(b: &PixelRect, clip: Option<Rect>) -> bool {
    let Some(c) = clip else { return true };
    if b.w == 0 || b.h == 0 || c.w == 0 || c.h == 0 {
        return false;
    }
    (b.x as u32 + b.w as u32) > c.x as u32
        && (c.x as u32 + c.w as u32) > b.x as u32
        && (b.y as u32 + b.h as u32) > c.y as u32
        && (c.y as u32 + c.h as u32) > b.y as u32
}

/// Layout rect under `xf`, as a rotation-aware AABB (collapses to the
/// scale box when unrotated, to the layout rect when identity).
fn transformed_bbox(lay: &ComputedLayout, xf: Transform) -> PixelRect {
    let (tx, ty, tw, th) = xf.apply_to_aabb(lay.x, lay.y, lay.w, lay.h);
    PixelRect {
        x: clamp_u16(tx),
        y: clamp_u16(ty),
        w: clamp_u16(tw),
        h: clamp_u16(th),
    }
}

/// True if `node` has a child that opaquely covers the node's entire
/// layout box (opaque full-cover `<img>` or opaque-bg full-cover
/// `<div>`, full opacity, untransformed) — the node's own background
/// fill would be dead pixels. Moved verbatim from `paint.rs`.
pub fn has_opaque_covering_child(
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
        if opacities.get(&cid).copied().unwrap_or(1.0) < 0.999 {
            continue;
        }
        if !transforms
            .get(&cid)
            .copied()
            .unwrap_or(Transform::IDENTITY)
            .is_identity()
        {
            continue;
        }
        let Some(clay) = layouts.get(&cid) else {
            continue;
        };
        let covers = clay.x <= nlay.x
            && clay.y <= nlay.y
            && clay.x + clay.w >= nlay.x + nlay.w
            && clay.y + clay.h >= nlay.y + nlay.h;
        if !covers {
            continue;
        }
        match &child.kind {
            NodeKind::Img { src } => {
                let base_w = clamp_u16(clay.w);
                let base_h = clamp_u16(clay.h);
                let cached = images
                    .get_sized(src, base_w, base_h)
                    .or_else(|| images.get(src));
                if let Some(CachedImage::Loaded { fully_opaque: true, .. }) = cached {
                    return true;
                }
            }
            NodeKind::Div => {
                if let Some(bg) = child.style.background_color
                    && bg.a == 0xFF
                {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

/// Apply opacity to a solid fill colour (premultiplied SrcAlpha when
/// translucent). Moved verbatim from `paint.rs`.
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

/// Opacity tint for textures (see the old `paint.rs` for the full
/// premultiplied-blend rationale). `None` = skip the tint path.
#[inline]
fn opacity_tint(opacity_u8: u8) -> Option<Rgba> {
    if opacity_u8 == 0xFF {
        None
    } else {
        Some(Rgba::new(opacity_u8, opacity_u8, opacity_u8, opacity_u8))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style::Style;

    fn fb() -> FramebufferConfig {
        FramebufferConfig {
            width: 1920,
            height: 1080,
            stride: 1920 * 4,
            fb0_phys: 0,
            fb1_phys: 0,
            fb2_phys: 0,
        }
    }

    /// One walk, one truth: a div's damage bbox IS its fill rect, a
    /// cache-miss text still damages (op None) using the layout box,
    /// and fully-transparent nodes vanish entirely.
    #[test]
    fn build_binds_bbox_to_op() {
        let mut tree = Tree::new();
        let root = tree.create(NodeKind::Div, Style::default());
        let panel = tree.create(
            NodeKind::Div,
            Style {
                background_color: Some(Rgba::new(10, 20, 30, 255)),
                ..Default::default()
            },
        );
        let label = tree.create(NodeKind::Text { content: "hi".into() }, Style::default());
        let ghost = tree.create(
            NodeKind::Div,
            Style {
                background_color: Some(Rgba::new(1, 2, 3, 255)),
                ..Default::default()
            },
        );
        tree.append_child(root, panel);
        tree.append_child(root, label);
        tree.append_child(root, ghost);

        let mut layouts = HashMap::new();
        let lay = |x, y, w, h| ComputedLayout { x, y, w, h };
        layouts.insert(root, lay(0.0, 0.0, 1920.0, 1080.0));
        layouts.insert(panel, lay(100.0, 50.0, 300.0, 40.0));
        layouts.insert(label, lay(107.0, 55.0, 120.0, 20.0));
        layouts.insert(ghost, lay(500.0, 500.0, 50.0, 50.0));

        let mut text_styles = HashMap::new();
        text_styles.insert(
            label,
            ResolvedTextStyle {
                font_name: "test".into(),
                px_size: 16.0,
                color: Rgba::new(255, 255, 255, 255),
            },
        );

        let mut opacities = HashMap::new();
        opacities.insert(ghost, 0.0); // fully transparent -> no entry

        let dl = build(
            &tree,
            root,
            &fb(),
            &layouts,
            &text_styles,
            &TextCache::new(),
            &ImageRegistry::new(),
            &opacities,
            &HashMap::new(),
            /* compositing */ true,
        );

        // Compositing => transparent full-frame clear.
        let bg = dl.background.expect("compositing clear");
        assert_eq!((bg.rect.w, bg.rect.h), (1920, 1080));

        // panel fill + label (cache-miss) = 2 entries; ghost gone.
        assert_eq!(dl.entries.len(), 2);

        let panel_e = &dl.entries[0];
        match panel_e.op.as_ref().expect("panel paints") {
            DrawOp::Fill { rect, .. } => {
                assert_eq!(
                    (rect.x, rect.y, rect.w, rect.h),
                    (
                        panel_e.item.bbox.x,
                        panel_e.item.bbox.y,
                        panel_e.item.bbox.w,
                        panel_e.item.bbox.h
                    ),
                    "damage bbox must equal the painted rect"
                );
            }
            other => panic!("expected Fill, got {other:?}"),
        }

        let label_e = &dl.entries[1];
        assert!(label_e.op.is_none(), "cache miss paints nothing");
        // Exact layout box — no 16-px snap anchoring (the old
        // divergence this module exists to prevent).
        assert_eq!(label_e.item.bbox.x, 107);
        assert_eq!(label_e.item.bbox.w, 120);
    }
}
