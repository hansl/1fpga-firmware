//! Per-frame damage tracking.
//!
//! Rather than re-painting the full 1080p framebuffer every iteration,
//! we keep a snapshot of "what was last painted to the framebuffer" as
//! a flat list of [`PaintedItem`]s keyed by [`NodeId`]. Each frame the
//! runtime computes the new desired scene (same shape, fresh data),
//! diffs against the snapshot, and produces a single bounding rect
//! covering everything that changed. Only that rect is repainted.
//!
//! For menu-style UIs where a single nav event changes a focus
//! highlight + a small text label, damage is typically <10% of the
//! screen — fence drops from ~50ms to ~5ms on those frames, and to
//! literally zero on idle frames where the scene is identical.
//!
//! The triple-buffer dance is sidestepped by configuring the
//! framebuffer with all three slot pointers aliased to the same
//! physical address. PRESENT still rotates the FPGA's internal
//! display/render/ready indices, but every index points to the same
//! memory, so damage tracking sees one consistent buffer. Trade-off:
//! when our damage paint runs concurrently with HDMI scanout, a
//! single-frame tear is possible. With damage typically painting in
//! under one vsync interval, this is hard to notice in practice; if
//! it ever shows up we can switch to per-FB scene tracking.

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use menu_core_host::protocol::Rect;

use crate::layout::ComputedLayout;
use crate::style::Transform;
use crate::text::ResolvedTextStyle;
use crate::vdom::{NodeId, NodeKind, Tree};

/// One drawable item recorded in a [`PaintedScene`]. The pair (bbox,
/// content_hash) is what damage detection compares — when either
/// changes we count the node as damaged and add its bbox to the
/// damage rect.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaintedItem {
    pub node_id: NodeId,
    pub bbox: PixelRect,
    pub content_hash: u64,
}

/// Pixel-aligned rect derived from [`ComputedLayout`]. Stored
/// separately from the host crate's [`Rect`] so we can `Hash` it for
/// content-hash composition.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Default)]
pub struct PixelRect {
    pub x: u16,
    pub y: u16,
    pub w: u16,
    pub h: u16,
}

impl PixelRect {
    fn intersects(&self, other: &PixelRect) -> bool {
        let ax2 = self.x as u32 + self.w as u32;
        let ay2 = self.y as u32 + self.h as u32;
        let bx2 = other.x as u32 + other.w as u32;
        let by2 = other.y as u32 + other.h as u32;
        ax2 > other.x as u32
            && bx2 > self.x as u32
            && ay2 > other.y as u32
            && by2 > self.y as u32
    }

    /// True if `self` is fully inside (or equal to) `other`.
    fn contained_in(&self, other: &PixelRect) -> bool {
        let sx2 = self.x as u32 + self.w as u32;
        let sy2 = self.y as u32 + self.h as u32;
        let ox2 = other.x as u32 + other.w as u32;
        let oy2 = other.y as u32 + other.h as u32;
        (self.x as u32) >= (other.x as u32)
            && (self.y as u32) >= (other.y as u32)
            && sx2 <= ox2
            && sy2 <= oy2
    }
}

impl From<PixelRect> for Rect {
    fn from(r: PixelRect) -> Self {
        Rect::new(r.x, r.y, r.w, r.h)
    }
}

/// Snapshot of all drawable items in the tree at a point in time.
#[derive(Default, Clone, Debug)]
pub struct PaintedScene {
    pub items: Vec<PaintedItem>,
}

impl PaintedScene {
    pub fn empty() -> Self {
        Self::default()
    }

    /// Same hash that [`scene_hash`] returns, derived from this
    /// snapshot's items. Lets the runtime skip the second tree walk
    /// when it already has the scene built (the hash check is the
    /// fast path — the snapshot itself drives damage diffs).
    pub fn hash(&self) -> u64 {
        let mut h = DefaultHasher::new();
        for item in &self.items {
            item.node_id.0.hash(&mut h);
            item.bbox.hash(&mut h);
            item.content_hash.hash(&mut h);
        }
        h.finish()
    }
}

/// Pixel area summed across `rects` (no overlap dedup — overlapping
/// damage rects double-count, which is the conservative direction
/// for the "should we fall back to full paint?" decision).
pub fn total_area(rects: &[PixelRect]) -> u64 {
    rects
        .iter()
        .map(|r| (r.w as u64) * (r.h as u64))
        .sum()
}

/// One-shot hash of everything that determines whether the framebuffer
/// would render identical pixels for the current tree+layout vs. a
/// prior state. Used by the runtime for the "scene unchanged → skip
/// submit" fast path; comparing two `u64`s is much cheaper than
/// diffing two scene Vecs and avoids the rendering-correctness pitfalls
/// of partial / damage-rect repaints.
pub fn scene_hash(
    tree: &Tree,
    root: NodeId,
    layouts: &HashMap<NodeId, ComputedLayout>,
    text_styles: &HashMap<NodeId, ResolvedTextStyle>,
    opacities: &HashMap<NodeId, f32>,
    transforms: &HashMap<NodeId, Transform>,
) -> u64 {
    let scene = compute_scene(tree, root, layouts, text_styles, opacities, transforms);
    scene.hash()
}

/// Walk the tree and build a [`PaintedScene`] reflecting what would
/// be drawn for the current state.
pub fn compute_scene(
    tree: &Tree,
    root: NodeId,
    layouts: &HashMap<NodeId, ComputedLayout>,
    text_styles: &HashMap<NodeId, ResolvedTextStyle>,
    opacities: &HashMap<NodeId, f32>,
    transforms: &HashMap<NodeId, Transform>,
) -> PaintedScene {
    let mut items = Vec::new();
    walk(tree, root, layouts, text_styles, opacities, transforms, &mut items);
    PaintedScene { items }
}

fn walk(
    tree: &Tree,
    id: NodeId,
    layouts: &HashMap<NodeId, ComputedLayout>,
    text_styles: &HashMap<NodeId, ResolvedTextStyle>,
    opacities: &HashMap<NodeId, f32>,
    transforms: &HashMap<NodeId, Transform>,
    out: &mut Vec<PaintedItem>,
) {
    let Some(node) = tree.get(id) else {
        return;
    };
    let Some(lay) = layouts.get(&id) else {
        return;
    };
    let xf = transforms.get(&id).copied().unwrap_or(Transform::IDENTITY);
    let scaled = !xf.is_identity();
    // Damage bbox follows the *transformed* footprint so per-rect
    // clip + copy cover the area actually painted. For non-scaled
    // nodes this collapses to the layout rect (with the identity
    // transform leaving x/y/w/h unchanged), so existing damage
    // behaviour is preserved.
    let bbox = transformed_bbox(lay, xf);

    // Effective opacity drives painted pixels — include it in the
    // content hash so a tween that animates opacity (without moving
    // the bbox) damages the right rect. Quantise to 8 bits so
    // microscopic float jitter doesn't invalidate every frame.
    let opacity_u8 = opacities
        .get(&id)
        .copied()
        .unwrap_or(1.0)
        .clamp(0.0, 1.0)
        .mul_add(255.0, 0.5) as u8;
    // Quantise the effective transform similarly so float jitter in
    // a tween's penultimate frame doesn't invalidate damage every
    // tick. 1.0 unit = 1024 (i.e., milli-scale × ~1) — fine enough to
    // catch small UI changes (1.10 vs 1.05), coarse enough to ignore
    // single-bit float noise.
    let sx_q = (xf.scale_x.clamp(0.0, 64.0) * 1024.0).round() as i32;
    let sy_q = (xf.scale_y.clamp(0.0, 64.0) * 1024.0).round() as i32;
    let _ = scaled; // currently only the bbox + hash uses xf
    // Fully-transparent nodes don't paint and therefore don't
    // contribute to the scene; treat them as if they weren't there.
    let skip = opacity_u8 == 0;

    if !skip {
        match &node.kind {
            NodeKind::Div => {
                if let Some(bg) = node.style.background_color {
                    if bbox.w > 0 && bbox.h > 0 {
                        let mut h = DefaultHasher::new();
                        0u8.hash(&mut h); // tag
                        bg.to_u32().hash(&mut h);
                        bbox.hash(&mut h);
                        opacity_u8.hash(&mut h);
                        out.push(PaintedItem {
                            node_id: id,
                            bbox,
                            content_hash: h.finish(),
                        });
                    }
                }
            }
            NodeKind::Text { content } => {
                if bbox.w > 0 && bbox.h > 0 {
                    // paint_text aligns dst.x down to a 16-pixel
                    // boundary for burst-friendly writes. The actual
                    // painted area therefore extends up to 15 px
                    // *left* of the layout's x. Mirror that here so
                    // damage rects cover the full painted footprint —
                    // otherwise an OLD-text damage rect leaves a
                    // sliver of stale pixels at its left edge when
                    // the new text shifts position.
                    let painted_bbox = PixelRect {
                        x: bbox.x & !15,
                        y: bbox.y,
                        w: bbox.w,
                        h: bbox.h,
                    };
                    let mut h = DefaultHasher::new();
                    1u8.hash(&mut h);
                    content.hash(&mut h);
                    if let Some(s) = text_styles.get(&id) {
                        s.font_name.hash(&mut h);
                        (s.px_size as u32).hash(&mut h);
                        s.color.to_u32().hash(&mut h);
                    }
                    painted_bbox.hash(&mut h);
                    opacity_u8.hash(&mut h);
                    sx_q.hash(&mut h);
                    sy_q.hash(&mut h);
                    out.push(PaintedItem {
                        node_id: id,
                        bbox: painted_bbox,
                        content_hash: h.finish(),
                    });
                }
            }
            NodeKind::Img { src } => {
                if bbox.w > 0 && bbox.h > 0 {
                    let mut h = DefaultHasher::new();
                    2u8.hash(&mut h);
                    src.hash(&mut h);
                    bbox.hash(&mut h);
                    opacity_u8.hash(&mut h);
                    sx_q.hash(&mut h);
                    sy_q.hash(&mut h);
                    out.push(PaintedItem {
                        node_id: id,
                        bbox,
                        content_hash: h.finish(),
                    });
                }
            }
        }
    }
    for &child in &node.children {
        walk(tree, child, layouts, text_styles, opacities, transforms, out);
    }
}

fn layout_to_bbox(lay: &ComputedLayout) -> PixelRect {
    let x = lay.x.max(0.0).min(u16::MAX as f32) as u16;
    let y = lay.y.max(0.0).min(u16::MAX as f32) as u16;
    let w = lay.w.max(0.0).min(u16::MAX as f32) as u16;
    let h = lay.h.max(0.0).min(u16::MAX as f32) as u16;
    PixelRect { x, y, w, h }
}

/// Layout rect scaled around its centre per `xf`. Used by the
/// damage walker so each item's bbox matches the area `paint::paint`
/// actually writes after applying the same transform.
fn transformed_bbox(lay: &ComputedLayout, xf: Transform) -> PixelRect {
    let (tx, ty, tw, th) = xf.apply_to_rect(lay.x, lay.y, lay.w, lay.h);
    let to_u16 = |v: f32| v.max(0.0).min(u16::MAX as f32) as u16;
    PixelRect {
        x: to_u16(tx),
        y: to_u16(ty),
        w: to_u16(tw),
        h: to_u16(th),
    }
}

/// Diff `prev` and `cur`; return per-rect damage covering every node
/// that was added, removed, or changed. Output rects are
/// containment-deduplicated — when one rect is fully inside another,
/// the smaller one is dropped, since painting the larger one already
/// covers it. This collapses the common parent-with-changed-children
/// pattern (e.g., a card animating its scale also "changes" every
/// descendant's transform-derived content_hash) into a single
/// outermost rect, saving redundant tree walks + duplicate blit work.
pub fn compute_damage(prev: &PaintedScene, cur: &PaintedScene) -> Vec<PixelRect> {
    let mut rects = Vec::new();

    // Removed: in prev, not in cur.
    for p in &prev.items {
        if !cur.items.iter().any(|c| c.node_id == p.node_id) {
            rects.push(p.bbox);
        }
    }

    // Added or changed: walk current.
    for c in &cur.items {
        match prev.items.iter().find(|p| p.node_id == c.node_id) {
            Some(p) if p.content_hash == c.content_hash => {
                // Unchanged.
            }
            Some(p) => {
                // Changed — include both old and new bbox to ensure
                // any pixels the old item covered get cleared even
                // if the new bbox is smaller.
                rects.push(p.bbox);
                if p.bbox != c.bbox {
                    rects.push(c.bbox);
                }
            }
            None => {
                rects.push(c.bbox);
            }
        }
    }

    dedupe_contained(rects)
}

/// Drop any rect that is fully contained in another rect of the
/// same list. O(N²) but `N` is the count of damaged drawables —
/// realistically <30 even for a busy tween frame. Identical rects
/// are kept by checking strict containment + index ordering.
fn dedupe_contained(rects: Vec<PixelRect>) -> Vec<PixelRect> {
    let mut keep: Vec<bool> = vec![true; rects.len()];
    for i in 0..rects.len() {
        if !keep[i] {
            continue;
        }
        for j in 0..rects.len() {
            if i == j || !keep[j] {
                continue;
            }
            // rects[i] contained in rects[j] → drop i. Tie-break on
            // equal rects: keep the lower-indexed one.
            if rects[i].contained_in(&rects[j])
                && (rects[i] != rects[j] || i > j)
            {
                keep[i] = false;
                break;
            }
        }
    }
    rects
        .into_iter()
        .zip(keep)
        .filter_map(|(r, k)| if k { Some(r) } else { None })
        .collect()
}

/// Bounding-rect union of every input rect. Returns `None` if the
/// list is empty (caller treats as "no damage, skip the frame").
pub fn union_rects(rects: &[PixelRect]) -> Option<PixelRect> {
    let mut iter = rects.iter().copied();
    let first = iter.next()?;
    let mut min_x = first.x as u32;
    let mut min_y = first.y as u32;
    let mut max_x = first.x as u32 + first.w as u32;
    let mut max_y = first.y as u32 + first.h as u32;
    for r in iter {
        min_x = min_x.min(r.x as u32);
        min_y = min_y.min(r.y as u32);
        max_x = max_x.max(r.x as u32 + r.w as u32);
        max_y = max_y.max(r.y as u32 + r.h as u32);
    }
    Some(PixelRect {
        x: min_x.min(u16::MAX as u32) as u16,
        y: min_y.min(u16::MAX as u32) as u16,
        w: (max_x - min_x).min(u16::MAX as u32) as u16,
        h: (max_y - min_y).min(u16::MAX as u32) as u16,
    })
}

#[allow(dead_code)]
pub fn rect_intersects_any(node: &PixelRect, rects: &[PixelRect]) -> bool {
    rects.iter().any(|r| node.intersects(r))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(x: u16, y: u16, w: u16, h: u16) -> PixelRect {
        PixelRect { x, y, w, h }
    }

    #[test]
    fn union_of_two_rects_covers_both() {
        let u = union_rects(&[r(10, 20, 100, 50), r(200, 30, 50, 100)]).unwrap();
        assert_eq!(u, r(10, 20, 240, 110));
    }

    #[test]
    fn union_of_empty_is_none() {
        assert_eq!(union_rects(&[]), None);
    }

    #[test]
    fn intersects_self_and_overlap() {
        let a = r(0, 0, 100, 100);
        let b = r(50, 50, 100, 100);
        let c = r(200, 200, 50, 50);
        assert!(a.intersects(&a));
        assert!(a.intersects(&b));
        assert!(!a.intersects(&c));
    }

    #[test]
    fn no_damage_when_scenes_match() {
        let s = PaintedScene {
            items: vec![PaintedItem {
                node_id: NodeId(1),
                bbox: r(0, 0, 100, 100),
                content_hash: 42,
            }],
        };
        assert!(compute_damage(&s, &s).is_empty());
    }

    #[test]
    fn damage_includes_old_and_new_when_bbox_changes() {
        let prev = PaintedScene {
            items: vec![PaintedItem {
                node_id: NodeId(1),
                bbox: r(0, 0, 100, 100),
                content_hash: 1,
            }],
        };
        let cur = PaintedScene {
            items: vec![PaintedItem {
                node_id: NodeId(1),
                bbox: r(50, 50, 100, 100),
                content_hash: 2,
            }],
        };
        let damage = compute_damage(&prev, &cur);
        assert_eq!(damage.len(), 2);
        assert!(damage.contains(&r(0, 0, 100, 100)));
        assert!(damage.contains(&r(50, 50, 100, 100)));
    }
}
