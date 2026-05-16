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
) -> u64 {
    let scene = compute_scene(tree, root, layouts, text_styles);
    let mut h = DefaultHasher::new();
    for item in &scene.items {
        item.node_id.0.hash(&mut h);
        item.bbox.hash(&mut h);
        item.content_hash.hash(&mut h);
    }
    h.finish()
}

/// Walk the tree and build a [`PaintedScene`] reflecting what would
/// be drawn for the current state.
pub fn compute_scene(
    tree: &Tree,
    root: NodeId,
    layouts: &HashMap<NodeId, ComputedLayout>,
    text_styles: &HashMap<NodeId, ResolvedTextStyle>,
) -> PaintedScene {
    let mut items = Vec::new();
    walk(tree, root, layouts, text_styles, &mut items);
    PaintedScene { items }
}

fn walk(
    tree: &Tree,
    id: NodeId,
    layouts: &HashMap<NodeId, ComputedLayout>,
    text_styles: &HashMap<NodeId, ResolvedTextStyle>,
    out: &mut Vec<PaintedItem>,
) {
    let Some(node) = tree.get(id) else {
        return;
    };
    let Some(lay) = layouts.get(&id) else {
        return;
    };
    let bbox = layout_to_bbox(lay);

    match &node.kind {
        NodeKind::Div => {
            if let Some(bg) = node.style.background_color {
                if bbox.w > 0 && bbox.h > 0 {
                    let mut h = DefaultHasher::new();
                    0u8.hash(&mut h); // tag
                    bg.to_u32().hash(&mut h);
                    bbox.hash(&mut h);
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
                let mut h = DefaultHasher::new();
                1u8.hash(&mut h);
                content.hash(&mut h);
                if let Some(s) = text_styles.get(&id) {
                    s.font_name.hash(&mut h);
                    (s.px_size as u32).hash(&mut h);
                    s.color.to_u32().hash(&mut h);
                }
                bbox.hash(&mut h);
                out.push(PaintedItem {
                    node_id: id,
                    bbox,
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
                out.push(PaintedItem {
                    node_id: id,
                    bbox,
                    content_hash: h.finish(),
                });
            }
        }
    }
    for &child in &node.children {
        walk(tree, child, layouts, text_styles, out);
    }
}

fn layout_to_bbox(lay: &ComputedLayout) -> PixelRect {
    let x = lay.x.max(0.0).min(u16::MAX as f32) as u16;
    let y = lay.y.max(0.0).min(u16::MAX as f32) as u16;
    let w = lay.w.max(0.0).min(u16::MAX as f32) as u16;
    let h = lay.h.max(0.0).min(u16::MAX as f32) as u16;
    PixelRect { x, y, w, h }
}

/// Diff `prev` and `cur`; return per-rect damage covering every node
/// that was added, removed, or changed.
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

    rects
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
