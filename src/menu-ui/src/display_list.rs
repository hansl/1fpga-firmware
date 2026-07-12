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
use crate::style::{Overflow, Transform};
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
    /// Text line, resolved against the shared text cache AT REPLAY
    /// TIME (not build time). The engine ensures this frame's text
    /// RTs immediately before replaying the same packet, so brand-new
    /// text paints in the same engine frame — resolving at build time
    /// instead left a one-frame blank blink during animations (the
    /// damage system repainted the region with background while the
    /// RT was still a cache miss, and the glyphs arrived a tick
    /// later). `x`/`y` are the transformed origin — SIGNED, because a
    /// line sliding off the left/top edge keeps its true origin here
    /// and replay trims the copy to `clip` (the accumulated viewport
    /// ∩ overflow clip) with a matching src offset; dst dims are the
    /// cached RT dims scaled by `sx`/`sy` (identity = exact).
    Text {
        key: CacheKey,
        x: i32,
        y: i32,
        sx: f32,
        sy: f32,
        blend: BlendMode,
        tint: Option<Rgba>,
        clip: Rect,
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

/// One hardware-plane candidate's paint plan: a display list in
/// PLANE-LOCAL coordinates plus the plane's screen geometry. The
/// geometry lives OUTSIDE the display list so a pure move (translate
/// tween on the portal) changes x/y but not the content hash — the
/// engine turns that into a position-register write with zero blits.
#[derive(Clone, Debug)]
pub struct PlaneDL {
    /// z rank (the `layer` style value). Higher = closer to the viewer.
    pub z: u8,
    /// Content in plane-local coordinates. `background` is the
    /// transparent clear (the scanout blends the plane by per-pixel
    /// alpha over wallpaper+content).
    pub dl: DisplayList,
    /// Screen position of the plane's top-left, transform applied
    /// (signed — planes may hang off any edge).
    pub x: i32,
    pub y: i32,
    /// Surface size (untransformed layout box).
    pub w: u16,
    pub h: u16,
    /// Hardware plane alpha: the portal's OWN accumulated opacity
    /// (its style opacity × every ancestor's). Content inside the
    /// plane renders at its LOCAL opacity product only — a portal
    /// fade therefore never changes the plane's pixels, just this
    /// value, which the engine turns into an alpha-register write.
    pub alpha: u8,
}

/// [`build`]'s output: the content-layer display list plus any
/// partitioned hardware-plane lists (v1: at most one — the hardware
/// has a single overlay plane; the highest-z candidate wins and the
/// rest stay inline in the content list).
#[derive(Clone, Debug, Default)]
pub struct BuildOutput {
    pub content: DisplayList,
    pub planes: Vec<PlaneDL>,
}

/// Hardware caps for plane candidates (mirrors
/// `Device::PLANE_MAX_W/H` — the compositor's width register is 12
/// bits and its row index 9 bits).
const PLANE_MAX_W: f32 = 4095.0;
const PLANE_MAX_H: f32 = 512.0;

/// Find the winning plane subtree: the highest-z `layer`-marked node
/// whose layout box fits the hardware caps. Ties keep the first in
/// tree order. Nodes under a winning candidate are NOT re-considered
/// (nested layers flatten into their plane).
fn find_plane_root(
    tree: &Tree,
    id: NodeId,
    layouts: &HashMap<NodeId, ComputedLayout>,
    best: &mut Option<(NodeId, u8)>,
) {
    let Some(node) = tree.get(id) else { return };
    if let Some(z) = node.style.layer
        && z > 0
        && let Some(lay) = layouts.get(&id)
        && lay.w >= 1.0
        && lay.h >= 1.0
        && lay.w <= PLANE_MAX_W
        && lay.h <= PLANE_MAX_H
    {
        if best.map(|(_, bz)| z > bz).unwrap_or(true) {
            *best = Some((id, z));
        }
        // Don't scan below a candidate — nested layers flatten.
        return;
    }
    for &child in &node.children {
        find_plane_root(tree, child, layouts, best);
    }
}

/// Build the display list(s) for the current tree state. The single
/// walk that replaces `damage::compute_scene` + `paint::paint`'s
/// tree recursion. A `layer`-marked subtree (see [`PlaneDL`]) is
/// partitioned out of the content list and rebuilt in plane-local
/// coordinates.
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
) -> BuildOutput {
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

    // Plane selection: only meaningful when compositing (the plane is
    // blended by the scanout compositor); without it everything stays
    // in the single framebuffer list.
    let mut plane_pick: Option<(NodeId, u8)> = None;
    if compositing {
        find_plane_root(tree, root, layouts, &mut plane_pick);
    }

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
        // Nothing outside the viewport is painted OR damage-tracked;
        // overflow:hidden ancestors shrink this as the walk descends.
        ClipF::new(fb.width as f32, fb.height as f32),
        plane_pick.map(|(id, _)| id),
        (0.0, 0.0),
        &mut entries,
    );
    let content = DisplayList { background, entries };

    let mut planes = Vec::new();
    if let Some((pid, z)) = plane_pick
        && let Some(lay) = layouts.get(&pid)
    {
        let xf = transforms.get(&pid).copied().unwrap_or(Transform::IDENTITY);
        let (tx, ty, _, _) = xf.apply_to_rect(lay.x, lay.y, lay.w, lay.h);
        let w = lay.w.round() as u16;
        let h = lay.h.round() as u16;
        // The portal's accumulated opacity becomes the hardware
        // alpha; the plane CONTENT uses a locally-resolved opacity
        // map (product of style opacities strictly BELOW the portal),
        // so fading the portal is a pure register change — content
        // hashes stay put and nothing re-renders.
        let alpha = opacities
            .get(&pid)
            .copied()
            .unwrap_or(1.0)
            .clamp(0.0, 1.0)
            .mul_add(255.0, 0.5) as u8;
        let mut local_opacities: HashMap<NodeId, f32> = HashMap::new();
        fn resolve_local(
            tree: &Tree,
            id: NodeId,
            acc: f32,
            out: &mut HashMap<NodeId, f32>,
        ) {
            let Some(node) = tree.get(id) else { return };
            let acc = acc * node.style.opacity.unwrap_or(1.0).clamp(0.0, 1.0);
            out.insert(id, acc);
            for &child in &node.children {
                resolve_local(tree, child, acc, out);
            }
        }
        // Portal itself enters at 1.0 — ITS opacity is the plane
        // alpha, not a pixel factor. (Its own style.opacity must not
        // double-apply, so seed children directly.)
        local_opacities.insert(pid, 1.0);
        if let Some(pnode) = tree.get(pid) {
            for &child in &pnode.children {
                resolve_local(tree, child, 1.0, &mut local_opacities);
            }
        }
        let mut plane_entries = Vec::new();
        walk(
            tree,
            pid,
            layouts,
            text_styles,
            text_cache,
            images,
            &local_opacities,
            transforms,
            // The engine clears the plane surface transparent before
            // replaying (via the background fill below) — pristine dst.
            true,
            // The portal's own background paints INTO the plane.
            /* skip_root_bg */ false,
            // Plane-local clip: the full surface, INCLUDING parts
            // currently off-screen — sliding the plane must reveal
            // pre-rendered pixels, not blanks.
            ClipF::new(w as f32, h as f32),
            None,
            // Subtracting the portal's own transformed origin converts
            // the walk to plane-local coordinates AND cancels the
            // portal's translate (the slide lives in the position
            // registers, not in the pixels). Portal scale/rotate are
            // unsupported (assumed identity).
            (tx, ty),
            &mut plane_entries,
        );
        planes.push(PlaneDL {
            z,
            dl: DisplayList {
                background: Some(BackgroundFill {
                    rect: Rect::new(0, 0, w, h),
                    color: Rgba::TRANSPARENT,
                }),
                entries: plane_entries,
            },
            x: tx.round() as i32,
            y: ty.round() as i32,
            w,
            h,
            alpha,
        });
    }

    BuildOutput { content, planes }
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
    clip: ClipF,
    // Subtree partitioned into a hardware plane — invisible to THIS
    // walk (it renders via its own plane-local walk).
    skip: Option<NodeId>,
    // Coordinate origin subtracted from every produced rect. (0,0)
    // for the framebuffer walk; the portal's transformed top-left for
    // a plane-local walk.
    origin: (f32, f32),
    out: &mut Vec<Entry>,
) {
    if skip == Some(id) {
        return;
    }
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
                let (tx, ty) = (tx - origin.0, ty - origin.1);
                if let Some(bbox) = clip.visible(tx, ty, tw, th) {
                    // Fills clip EXACTLY: the painted rect is the
                    // visible rect (no src to adjust).
                    let rect = Rect::new(bbox.x, bbox.y, bbox.w, bbox.h);
                    let mut h = DefaultHasher::new();
                    0u8.hash(&mut h); // tag
                    color.to_u32().hash(&mut h);
                    bbox.hash(&mut h);
                    opacity_u8.hash(&mut h);
                    // Over the pristine transparent clear, SrcAlpha of
                    // a premultiplied source against dst=0 IS the
                    // source — write it Opaque and skip the per-pixel
                    // dst read. Same policy Text/Copy below already
                    // apply; the child_dst_clear chain guarantees
                    // correctness (anything painting over pixels a
                    // parent already filled has lost the flag). The
                    // stored premultiplied rgb+alpha is exactly what
                    // the scanout's plane/content blend consumes —
                    // note this premultiplies even at full opacity
                    // (apply_opacity_to_color's Opaque branch keeps
                    // straight rgb, which over-brightens sub-FF-alpha
                    // fills at composite). This is what keeps a full
                    // plane re-render (11 translucent card bodies) at
                    // burst-write speed instead of ~30 ms of RMW.
                    let (effective_color, blend) = if dst_clear {
                        let eff_a = ((color.a as u16) * (opacity_u8 as u16) / 255) as u8;
                        let pre = |c: u8| ((c as u16) * (eff_a as u16) / 255) as u8;
                        (
                            Rgba::new(pre(color.r), pre(color.g), pre(color.b), eff_a),
                            BlendMode::Opaque,
                        )
                    } else {
                        apply_opacity_to_color(color, opacity_u8)
                    };
                    // Skip the fill op (not the damage item) when it
                    // cannot change pixels: effective alpha 0 (CSS
                    // transparent paints NOTHING — an Opaque fill of
                    // zeros would punch a hole through the content
                    // layer to the wallpaper, and a SrcAlpha a=0 fill
                    // is a pure read+write of every pixel for no
                    // visual change), or an opaque child covering the
                    // whole div (the fill would be overwritten).
                    let op = if effective_color.a == 0
                        || has_opaque_covering_child(
                            tree, id, layouts, images, opacities, transforms,
                        ) {
                        None
                    } else {
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
                // Dst rect from the CACHED RT dims when available
                // (that is exactly what replay blits); the layout box
                // otherwise. The two agree in practice — Taffy
                // measures text through the same atlas — so a
                // first-frame miss doesn't misplace damage.
                let (tx, ty, tw, th) = match text_cache.lookup(&key) {
                    Some(cached) if cached.width > 0 && cached.height > 0 => xf.apply_to_rect(
                        lay.x,
                        lay.y,
                        cached.width as f32,
                        cached.height as f32,
                    ),
                    _ => xf.apply_to_aabb(lay.x, lay.y, lay.w, lay.h),
                };
                let (tx, ty) = (tx - origin.0, ty - origin.1);
                if let Some(bbox) = clip.visible(tx, ty, tw, th) {
                    let op = Some(DrawOp::Text {
                        key: key.clone(),
                        // True (possibly negative) origin; replay
                        // trims the blit to `clip` with a matching
                        // src offset, so a line sliding off an edge
                        // keeps its glyphs pinned in place instead of
                        // shifting.
                        x: tx.round() as i32,
                        y: ty.round() as i32,
                        sx: xf.scale_x,
                        sy: xf.scale_y,
                        // Color is baked into the RT. Over the pristine
                        // transparent clear an Opaque copy is pixel-
                        // identical to SrcAlpha (premultiplied src over
                        // transparent) and skips the per-pixel dst read
                        // — including when an opacity tint applies (the
                        // tint multiplies the premultiplied source; the
                        // dst contributes nothing against transparent).
                        blend: if dst_clear {
                            BlendMode::Opaque
                        } else {
                            BlendMode::SrcAlpha
                        },
                        tint: opacity_tint(opacity_u8),
                        clip: Rect::new(bbox.x, bbox.y, bbox.w, bbox.h),
                    });
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
            let (ax, ay, aw, ah) = xf.apply_to_aabb(lay.x, lay.y, lay.w, lay.h);
            let (ax, ay) = (ax - origin.0, ay - origin.1);
            if let Some(bbox) = clip.visible(ax, ay, aw, ah) {
                let mut h = DefaultHasher::new();
                2u8.hash(&mut h);
                src.hash(&mut h);
                bbox.hash(&mut h);
                opacity_u8.hash(&mut h);
                sx_q.hash(&mut h);
                sy_q.hash(&mut h);
                rot_q.hash(&mut h);
                let op = build_img_op(src, lay, images, opacity_u8, xf, dst_clear, origin, bbox);
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
    // overflow:hidden shrinks the clip for the subtree; an empty
    // result prunes the whole subtree (the carousel's off-window
    // cards never even get walked).
    let child_clip = if node.style.overflow == Some(Overflow::Hidden) {
        let (tx, ty, tw, th) = xf.apply_to_rect(lay.x, lay.y, lay.w, lay.h);
        clip.intersect(tx - origin.0, ty - origin.1, tw, th)
    } else {
        clip
    };
    if child_clip.is_empty() {
        return;
    }
    for &child in &node.children {
        walk(
            tree, child, layouts, text_styles, text_cache, images, opacities, transforms,
            child_dst_clear, false, child_clip, skip, origin, out,
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
    // Coordinate origin (see `walk`) — subtracted so plane-local
    // lists get plane-local dst rects.
    origin: (f32, f32),
    // `visible`: the transformed AABB's visible portion (already
    // viewport- and overflow-clipped by the caller). The Copy dst is
    // trimmed to it with a proportional src trim; the affine path
    // only culls (its dst is an engine-computed AABB).
    visible: PixelRect,
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
    let (tx, ty) = (tx - origin.0, ty - origin.1);

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

    if tw < 0.5 || th < 0.5 {
        return None;
    }
    // Trim dst to the visible rect and map the trim back into src
    // space. 1:1 copies (the common case — the registry pre-sizes
    // images to their layout box) map exactly; scaled copies land
    // within a pixel, invisible on a moving edge.
    let rx = src_w as f32 / tw;
    let ry = src_h as f32 / th;
    let sx0 = (((visible.x as f32 - tx) * rx).round().max(0.0) as u16).min(src_w - 1);
    let sy0 = (((visible.y as f32 - ty) * ry).round().max(0.0) as u16).min(src_h - 1);
    let sw = ((visible.w as f32 * rx).round().max(1.0) as u16).min(src_w - sx0);
    let sh = ((visible.h as f32 * ry).round().max(1.0) as u16).min(src_h - sy0);
    let dst = Rect::new(visible.x, visible.y, visible.w, visible.h);
    // Opaque when the pixels can't need blending (opaque src at full
    // opacity) or can't have anything to blend WITH (pristine dst —
    // the tint still applies on the Opaque path).
    let blend = if dst_clear || (fully_opaque && opacity_u8 == 0xFF) {
        BlendMode::Opaque
    } else {
        BlendMode::SrcAlpha
    };
    Some(DrawOp::Copy {
        texture,
        src: Rect::new(sx0, sy0, sw, sh),
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
    text_cache: &TextCache,
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
            DrawOp::Text { key, x, y, sx, sy, blend, tint, clip: tclip } => {
                match text_cache.lookup(key) {
                    Some(cached) if cached.width > 0 && cached.height > 0 => {
                        let dw = ((cached.width as f32) * sx).max(1.0);
                        let dh = ((cached.height as f32) * sy).max(1.0);
                        // Trim the dst rect (true, possibly negative
                        // origin) to the build-time clip, mapping the
                        // trim back into src space so partially
                        // visible lines keep their glyphs in place.
                        let dx0 = (*x).max(tclip.x as i32);
                        let dy0 = (*y).max(tclip.y as i32);
                        let dx1 = (*x + dw.round() as i32).min(tclip.x as i32 + tclip.w as i32);
                        let dy1 = (*y + dh.round() as i32).min(tclip.y as i32 + tclip.h as i32);
                        if dx1 <= dx0 || dy1 <= dy0 {
                            frame
                        } else {
                            let rx = cached.width as f32 / dw;
                            let ry = cached.height as f32 / dh;
                            let sx0 = ((((dx0 - x) as f32) * rx).round().max(0.0) as u16)
                                .min(cached.width - 1);
                            let sy0 = ((((dy0 - y) as f32) * ry).round().max(0.0) as u16)
                                .min(cached.height - 1);
                            let sw = ((((dx1 - dx0) as f32) * rx).round().max(1.0) as u16)
                                .min(cached.width - sx0);
                            let sh = ((((dy1 - dy0) as f32) * ry).round().max(1.0) as u16)
                                .min(cached.height - sy0);
                            frame.copy_rect(
                                &cached.texture,
                                Rect::new(sx0, sy0, sw, sh),
                                Rect::new(
                                    dx0 as u16,
                                    dy0 as u16,
                                    (dx1 - dx0) as u16,
                                    (dy1 - dy0) as u16,
                                ),
                                CopyOpts { blend: *blend, filter: Filter::Nearest, tint: *tint },
                            )?
                        }
                    }
                    // RT not cached (atlas missing / zero extent):
                    // skip silently, same as the old painter; the
                    // damage item keeps the region tracked.
                    _ => frame,
                }
            }
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

/// Accumulated paint clip in f32 screen space: the viewport
/// intersected with every `overflow: hidden` ancestor's transformed
/// rect.
///
/// All culling and clipping happens HERE, in f32, before any u16
/// clamp. The old path clamped raw transformed coordinates, which (a)
/// pinned off-left geometry to x = 0 — carousel cards sliding out the
/// left edge stacked at the screen corner, blending over each other
/// and churning damage every tween frame — (b) let off-right
/// geometry reach the blitter at x beyond the framebuffer width,
/// where `y*stride + x*bpp` addressing wraps it into the wrong rows,
/// and (c) painted and damage-tracked everything regardless of
/// visibility, so offscreen nodes cost real FPGA bandwidth.
#[derive(Clone, Copy, Debug)]
struct ClipF {
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
}

impl ClipF {
    fn new(w: f32, h: f32) -> Self {
        ClipF { x0: 0.0, y0: 0.0, x1: w, y1: h }
    }

    fn intersect(self, x: f32, y: f32, w: f32, h: f32) -> Self {
        ClipF {
            x0: self.x0.max(x),
            y0: self.y0.max(y),
            x1: self.x1.min(x + w),
            y1: self.y1.min(y + h),
        }
    }

    fn is_empty(self) -> bool {
        self.x1 - self.x0 < 0.5 || self.y1 - self.y0 < 0.5
    }

    /// The visible pixel rect of `(x, y, w, h)` under this clip;
    /// `None` when nothing survives. The result is always inside the
    /// framebuffer (the root clip is the viewport), so the u16
    /// conversions cannot distort.
    fn visible(self, x: f32, y: f32, w: f32, h: f32) -> Option<PixelRect> {
        if w < 0.5 || h < 0.5 {
            return None;
        }
        let c = self.intersect(x, y, w, h);
        if c.is_empty() {
            return None;
        }
        let x0 = c.x0.round().max(0.0);
        let y0 = c.y0.round().max(0.0);
        let pw = (c.x1.round() - x0).max(0.0) as u16;
        let ph = (c.y1.round() - y0).max(0.0) as u16;
        if pw == 0 || ph == 0 {
            return None;
        }
        Some(PixelRect { x: x0 as u16, y: y0 as u16, w: pw, h: ph })
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

/// Apply opacity to a solid fill colour: opaque results write
/// directly, anything translucent premultiplies and blends. Keyed on
/// the EFFECTIVE alpha (colour alpha × opacity) — the old version
/// keyed on opacity alone, so a full-opacity translucent colour
/// (`#131d29e6`) was written Opaque with straight rgb: it stomped
/// whatever it painted over instead of blending, and composited
/// over-bright at scanout. (Fills over the pristine clear don't reach
/// this — the dst_clear fast path in `walk` premultiplies + writes
/// Opaque, which against transparent IS the blend.)
#[inline]
fn apply_opacity_to_color(color: Rgba, opacity_u8: u8) -> (Rgba, BlendMode) {
    let eff_a = ((color.a as u16) * (opacity_u8 as u16) / 255) as u8;
    if eff_a == 0xFF {
        (color, BlendMode::Opaque)
    } else {
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

        let out = build(
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
        assert!(out.planes.is_empty());
        let dl = out.content;

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
        // Text ops always exist and resolve at REPLAY time — a cache
        // miss at build time must not drop the op (that produced a
        // one-frame blank blink for brand-new text during animations).
        match label_e.op.as_ref().expect("text op present") {
            DrawOp::Text { key, x, y, sx, sy, .. } => {
                assert_eq!(key.content, "hi");
                assert_eq!((*x, *y), (107, 55));
                assert_eq!((*sx, *sy), (1.0, 1.0));
            }
            other => panic!("expected Text, got {other:?}"),
        }
        // Exact layout box — no 16-px snap anchoring (the old
        // divergence this module exists to prevent).
        assert_eq!(label_e.item.bbox.x, 107);
        assert_eq!(label_e.item.bbox.w, 120);
    }

    /// Offscreen nodes vanish (no op, no damage); nodes straddling
    /// the left edge are trimmed to the visible part instead of being
    /// pinned to x = 0 (which used to stack off-left carousel cards
    /// at the screen corner); zero-alpha fills keep their damage item
    /// but paint nothing.
    #[test]
    fn viewport_culls_and_clips() {
        let mut tree = Tree::new();
        let root = tree.create(NodeKind::Div, Style::default());
        let solid = |r, g, b, a| Style {
            background_color: Some(Rgba::new(r, g, b, a)),
            ..Default::default()
        };
        let off_left = tree.create(NodeKind::Div, solid(1, 1, 1, 255));
        let straddle = tree.create(NodeKind::Div, solid(2, 2, 2, 255));
        let off_right = tree.create(NodeKind::Div, solid(3, 3, 3, 255));
        let ghost_bg = tree.create(NodeKind::Div, solid(9, 9, 9, 0)); // transparent bg
        tree.append_child(root, off_left);
        tree.append_child(root, straddle);
        tree.append_child(root, off_right);
        tree.append_child(root, ghost_bg);

        let mut layouts = HashMap::new();
        let lay = |x, y, w, h| ComputedLayout { x, y, w, h };
        layouts.insert(root, lay(0.0, 0.0, 1920.0, 1080.0));
        layouts.insert(off_left, lay(-500.0, 100.0, 300.0, 100.0)); // fully out
        layouts.insert(straddle, lay(-100.0, 100.0, 300.0, 100.0)); // 200 px visible
        layouts.insert(off_right, lay(30000.0, 100.0, 300.0, 100.0)); // fully out
        layouts.insert(ghost_bg, lay(10.0, 10.0, 50.0, 50.0));

        let out = build(
            &tree,
            root,
            &fb(),
            &layouts,
            &HashMap::new(),
            &TextCache::new(),
            &ImageRegistry::new(),
            &HashMap::new(),
            &HashMap::new(),
            true,
        );
        assert!(out.planes.is_empty());
        let dl = out.content;

        // off_left and off_right are gone entirely; straddle + ghost remain.
        assert_eq!(dl.entries.len(), 2);
        let s = &dl.entries[0];
        assert_eq!((s.item.bbox.x, s.item.bbox.w), (0, 200), "trimmed, not shifted");
        match s.op.as_ref().expect("straddle paints") {
            DrawOp::Fill { rect, .. } => assert_eq!((rect.x, rect.w), (0, 200)),
            other => panic!("expected Fill, got {other:?}"),
        }
        let g = &dl.entries[1];
        assert!(g.op.is_none(), "zero-alpha fill paints nothing");
        assert_eq!(g.item.bbox.w, 50, "…but still damages (visibility toggles)");
    }

    /// overflow:hidden clips children at paint time and prunes
    /// subtrees that fall fully outside the window.
    #[test]
    fn overflow_hidden_clips_children() {
        let mut tree = Tree::new();
        let root = tree.create(NodeKind::Div, Style::default());
        let frame_style = Style {
            overflow: Some(Overflow::Hidden),
            ..Default::default()
        };
        let window = tree.create(NodeKind::Div, frame_style);
        let solid = Style {
            background_color: Some(Rgba::new(5, 5, 5, 255)),
            ..Default::default()
        };
        let inside = tree.create(NodeKind::Div, solid.clone());
        let poking = tree.create(NodeKind::Div, solid.clone());
        let outside = tree.create(NodeKind::Div, solid);
        tree.append_child(root, window);
        tree.append_child(window, inside);
        tree.append_child(window, poking);
        tree.append_child(window, outside);

        let mut layouts = HashMap::new();
        let lay = |x, y, w, h| ComputedLayout { x, y, w, h };
        layouts.insert(root, lay(0.0, 0.0, 1920.0, 1080.0));
        layouts.insert(window, lay(100.0, 100.0, 400.0, 200.0));
        layouts.insert(inside, lay(150.0, 150.0, 100.0, 50.0));
        layouts.insert(poking, lay(450.0, 150.0, 100.0, 50.0)); // 50 px inside
        layouts.insert(outside, lay(600.0, 150.0, 100.0, 50.0)); // beyond window

        let out = build(
            &tree,
            root,
            &fb(),
            &layouts,
            &HashMap::new(),
            &TextCache::new(),
            &ImageRegistry::new(),
            &HashMap::new(),
            &HashMap::new(),
            true,
        );
        assert!(out.planes.is_empty());
        let dl = out.content;

        assert_eq!(dl.entries.len(), 2, "outside child culled");
        assert_eq!(dl.entries[0].item.bbox.w, 100);
        let p = &dl.entries[1];
        assert_eq!(
            (p.item.bbox.x, p.item.bbox.w),
            (450, 50),
            "poking child trimmed at the window edge"
        );
    }

    /// Translucent fills over the pristine transparent clear write
    /// PREMULTIPLIED pixels with an Opaque burst (no dst read);
    /// children of a filled parent lose the flag and blend.
    #[test]
    fn pristine_fill_is_premultiplied_opaque() {
        let mut tree = Tree::new();
        let root = tree.create(NodeKind::Div, Style::default());
        let panel = tree.create(
            NodeKind::Div,
            Style {
                // #40404080: straight rgb 64, alpha 128.
                background_color: Some(Rgba::new(64, 64, 64, 128)),
                ..Default::default()
            },
        );
        let inner = tree.create(
            NodeKind::Div,
            Style {
                background_color: Some(Rgba::new(200, 200, 200, 128)),
                ..Default::default()
            },
        );
        tree.append_child(root, panel);
        tree.append_child(panel, inner);

        let mut layouts = HashMap::new();
        let lay = |x, y, w, h| ComputedLayout { x, y, w, h };
        layouts.insert(root, lay(0.0, 0.0, 1920.0, 1080.0));
        layouts.insert(panel, lay(100.0, 100.0, 400.0, 200.0));
        layouts.insert(inner, lay(120.0, 120.0, 100.0, 50.0));

        let out = build(
            &tree,
            root,
            &fb(),
            &layouts,
            &HashMap::new(),
            &TextCache::new(),
            &ImageRegistry::new(),
            &HashMap::new(),
            &HashMap::new(),
            true, // compositing → pristine transparent clear
        );
        let dl = out.content;
        assert_eq!(dl.entries.len(), 2);
        match dl.entries[0].op.as_ref().expect("panel paints") {
            DrawOp::Fill { color, blend, .. } => {
                assert_eq!(*blend, BlendMode::Opaque, "pristine fill skips dst reads");
                // Premultiplied: 64 * 128/255 ≈ 32.
                assert_eq!(color.a, 128);
                assert!(color.r <= 33 && color.r >= 31, "rgb premultiplied, got {}", color.r);
            }
            other => panic!("expected Fill, got {other:?}"),
        }
        match dl.entries[1].op.as_ref().expect("inner paints") {
            DrawOp::Fill { blend, .. } => {
                assert_eq!(
                    *blend,
                    BlendMode::SrcAlpha,
                    "child over the parent's fill must blend"
                );
            }
            other => panic!("expected Fill, got {other:?}"),
        }
    }

    /// `layer`-marked subtrees partition out of the content list into
    /// a plane list with PLANE-LOCAL coordinates, the portal's
    /// translate lands in the plane GEOMETRY (not the pixels), and a
    /// pure move leaves the plane's content hash untouched — the
    /// register-write fast path's foundation.
    #[test]
    fn layer_subtree_partitions_into_plane() {
        let build_with_tx = |slide: f32| {
            let mut tree = Tree::new();
            let root = tree.create(NodeKind::Div, Style::default());
            let portal = tree.create(
                NodeKind::Div,
                Style { layer: Some(1), ..Default::default() },
            );
            let card = tree.create(
                NodeKind::Div,
                Style {
                    background_color: Some(Rgba::new(20, 30, 40, 255)),
                    ..Default::default()
                },
            );
            tree.append_child(root, portal);
            tree.append_child(portal, card);

            let mut layouts = HashMap::new();
            let lay = |x, y, w, h| ComputedLayout { x, y, w, h };
            layouts.insert(root, lay(0.0, 0.0, 1920.0, 1080.0));
            // Wider than the screen — only a plane can hold this.
            layouts.insert(portal, lay(100.0, 380.0, 3000.0, 400.0));
            layouts.insert(card, lay(300.0, 400.0, 280.0, 340.0));

            // The slide: portal + its subtree share the translate
            // (resolve_transforms accumulates it in the real flow).
            let slid = Transform { tx: slide, ..Transform::IDENTITY };
            let mut transforms = HashMap::new();
            transforms.insert(portal, slid);
            transforms.insert(card, slid);

            // Portal at half opacity: becomes HARDWARE alpha, must NOT
            // bake into the plane's pixels (resolve_opacity would give
            // the card 0.5 accumulated; the plane walk re-resolves
            // locally below the portal).
            let mut opacities = HashMap::new();
            opacities.insert(portal, 0.5);
            opacities.insert(card, 0.5);

            build(
                &tree,
                root,
                &fb(),
                &layouts,
                &HashMap::new(),
                &TextCache::new(),
                &ImageRegistry::new(),
                &opacities,
                &transforms,
                true,
            )
        };

        let out = build_with_tx(-50.0);
        // Content list: the portal subtree is gone entirely.
        assert!(out.content.entries.is_empty(), "portal content stays out");
        assert_eq!(out.planes.len(), 1);
        let p = &out.planes[0];
        assert_eq!((p.z, p.w, p.h), (1, 3000, 400));
        // Geometry carries the translate: 100 - 50.
        assert_eq!((p.x, p.y), (50, 380));
        // Card in plane-local coords: layout offset inside the portal,
        // translate cancelled.
        assert_eq!(p.dl.entries.len(), 1);
        let bbox = &p.dl.entries[0].item.bbox;
        assert_eq!((bbox.x, bbox.y, bbox.w, bbox.h), (200, 20, 280, 340));
        // The plane's background is the transparent clear.
        assert_eq!(p.dl.background.as_ref().unwrap().color, Rgba::TRANSPARENT);
        // Portal opacity became hardware alpha; the card's pixels
        // render at FULL local opacity (fill color alpha untouched).
        assert_eq!(p.alpha, 128);
        match p.dl.entries[0].op.as_ref().expect("card paints") {
            DrawOp::Fill { color, .. } => {
                assert_eq!(color.a, 255, "portal fade must not bake into plane pixels");
            }
            other => panic!("expected Fill, got {other:?}"),
        }

        // Pure move: same content hash, different geometry.
        let moved = build_with_tx(-150.0);
        assert_eq!(moved.planes[0].x, -50);
        assert_eq!(
            moved.planes[0].dl.scene_hash(),
            p.dl.scene_hash(),
            "a slide must not change the plane's content hash"
        );
    }
}
