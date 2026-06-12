//! Host node tree.
//!
//! Nodes are stored in a freelist-backed `Vec` so [`NodeId`]s are
//! stable across mutations. Children are tracked with an explicit
//! `Vec<NodeId>` per node — order matters for paint.
//!
//! N1 supports a single node kind (`Div`). Text nodes and Img nodes
//! land in N4 / N5 alongside their renderers.

use crate::style::Style;

/// Stable identifier for a node in the [`Tree`]. Maps to the index of
/// the node's slot in `Tree::nodes`. JS references nodes by raw `u32`
/// values; this newtype keeps the Rust side type-safe.
#[derive(Debug, Default, Copy, Clone, PartialEq, Eq, Hash)]
pub struct NodeId(pub u32);

impl NodeId {
    /// The `0` id is reserved for "no node" — never handed out.
    pub const NONE: NodeId = NodeId(0);
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeKind {
    Div,
    /// A text node: leaf, no children. Layout measures its content
    /// against the resolved font & size; paint emits one COPY_RECT
    /// per glyph.
    Text { content: String },
    /// An image leaf. `src` is a filesystem path resolved by the
    /// `ImageRegistry`. Intrinsic size comes from the decoded PNG
    /// unless `style.width`/`height` overrides.
    Img { src: String },
}

#[derive(Debug, Clone)]
pub struct Node {
    pub kind: NodeKind,
    pub style: Style,
    pub children: Vec<NodeId>,
    pub parent: NodeId,
}

#[derive(Debug)]
pub struct Tree {
    /// Slot 0 reserved for `NodeId::NONE`. Slot `i > 0` either holds a
    /// live node or `None` if the slot is free.
    nodes: Vec<Option<Node>>,
    free: Vec<u32>,
    /// Marks whether the tree changed since the last paint. Cleared by
    /// the runtime after each successful frame.
    dirty: bool,
    /// Marks whether anything LAYOUT-affecting changed since the last
    /// `layout::compute` (structure, text, or a layout style field).
    /// Paint-only changes (opacity/scale/rotate tweens, color, img src)
    /// do NOT set it, so the runtime can reuse the cached layout across
    /// those frames — the whole point being cheap transform animations.
    /// Cleared by the runtime when it (re)computes layout.
    layout_dirty: bool,
}

impl Default for Tree {
    fn default() -> Self {
        // The derived default would leave `nodes` empty, which makes
        // the first `create()` return `NodeId(0)` — colliding with
        // `NodeId::NONE`. Always reserve slot 0.
        Self::new()
    }
}

impl Tree {
    pub fn new() -> Self {
        Self {
            nodes: vec![None],
            free: Vec::new(),
            dirty: false,
            // First frame must lay out: the tree starts "needs layout".
            layout_dirty: true,
        }
    }

    pub fn create(&mut self, kind: NodeKind, style: Style) -> NodeId {
        let node = Node {
            kind,
            style,
            children: Vec::new(),
            parent: NodeId::NONE,
        };
        let id = if let Some(slot) = self.free.pop() {
            self.nodes[slot as usize] = Some(node);
            NodeId(slot)
        } else {
            self.nodes.push(Some(node));
            NodeId((self.nodes.len() - 1) as u32)
        };
        self.dirty = true; self.layout_dirty = true;
        id
    }

    pub fn remove(&mut self, id: NodeId) {
        if id == NodeId::NONE {
            return;
        }
        // Detach from parent.
        let parent = self.nodes[id.0 as usize]
            .as_ref()
            .map(|n| n.parent)
            .unwrap_or(NodeId::NONE);
        if parent != NodeId::NONE
            && let Some(p) = self.nodes[parent.0 as usize].as_mut() {
            p.children.retain(|&c| c != id);
        }
        // Recursively remove children.
        let children = self.nodes[id.0 as usize]
            .as_ref()
            .map(|n| n.children.clone())
            .unwrap_or_default();
        for child in children {
            self.remove(child);
        }
        self.nodes[id.0 as usize] = None;
        self.free.push(id.0);
        self.dirty = true; self.layout_dirty = true;
    }

    pub fn append_child(&mut self, parent: NodeId, child: NodeId) {
        if parent == NodeId::NONE || child == NodeId::NONE {
            return;
        }
        self.detach_from_parent(child);
        if let Some(c) = self.nodes[child.0 as usize].as_mut() {
            c.parent = parent;
        }
        if let Some(p) = self.nodes[parent.0 as usize].as_mut() {
            p.children.push(child);
        }
        self.dirty = true; self.layout_dirty = true;
    }

    /// Insert `child` into `parent`'s children list immediately before
    /// `before`. If `before` is not a child of `parent`, the call is a
    /// no-op (matches the `insertBefore` semantics react-reconciler
    /// expects when a sibling has already been removed).
    pub fn insert_before(&mut self, parent: NodeId, child: NodeId, before: NodeId) {
        if parent == NodeId::NONE || child == NodeId::NONE {
            return;
        }
        self.detach_from_parent(child);
        if let Some(c) = self.nodes[child.0 as usize].as_mut() {
            c.parent = parent;
        }
        if let Some(p) = self.nodes[parent.0 as usize].as_mut() {
            match p.children.iter().position(|&c| c == before) {
                Some(idx) => p.children.insert(idx, child),
                None => p.children.push(child),
            }
        }
        self.dirty = true; self.layout_dirty = true;
    }

    /// Detach `child` from `parent`'s children list without recursively
    /// removing the subtree. The child becomes orphaned (parent =
    /// NONE) but its descendants remain. Used by react-reconciler's
    /// `removeChild` which expects the node and its subtree to be
    /// re-mountable later.
    pub fn detach_child(&mut self, parent: NodeId, child: NodeId) {
        if parent == NodeId::NONE || child == NodeId::NONE {
            return;
        }
        if let Some(p) = self.nodes[parent.0 as usize].as_mut() {
            p.children.retain(|&c| c != child);
        }
        if let Some(c) = self.nodes[child.0 as usize].as_mut() {
            c.parent = NodeId::NONE;
        }
        self.dirty = true; self.layout_dirty = true;
    }

    fn detach_from_parent(&mut self, child: NodeId) {
        let prev_parent = self.nodes[child.0 as usize]
            .as_ref()
            .map(|n| n.parent)
            .unwrap_or(NodeId::NONE);
        if prev_parent != NodeId::NONE
            && let Some(p) = self.nodes[prev_parent.0 as usize].as_mut()
        {
            p.children.retain(|&c| c != child);
        }
    }

    pub fn set_style(&mut self, id: NodeId, style: Style) {
        if let Some(n) = self.nodes.get_mut(id.0 as usize).and_then(|s| s.as_mut()) {
            // React's commitUpdate funnels EVERY style change through here.
            // Only a layout-affecting field change forces a reflow; a
            // paint-only change (opacity/scale/rotate/translate/color)
            // keeps the cached layout. This is what lets paint-only
            // re-renders — e.g. brightening the selected row on vertical
            // nav — skip the relayout entirely.
            if !n.style.layout_eq(&style) {
                self.layout_dirty = true;
            }
            n.style = style;
            self.dirty = true;
        }
    }

    /// Like [`Self::set_style`] but for a style change the caller knows is
    /// PAINT-ONLY (opacity/scale/rotate/color) — it marks the tree dirty for
    /// repaint but NOT layout-dirty, so the runtime keeps the cached layout.
    /// Used by transform/opacity tweens (see `runtime::anim`).
    pub fn set_style_paint(&mut self, id: NodeId, style: Style) {
        if let Some(n) = self.nodes.get_mut(id.0 as usize).and_then(|s| s.as_mut()) {
            n.style = style;
            self.dirty = true;
        }
    }

    /// Replace the content of a text node. No-op for non-text nodes.
    pub fn set_text(&mut self, id: NodeId, content: String) {
        if let Some(n) = self.nodes.get_mut(id.0 as usize).and_then(|s| s.as_mut())
            && matches!(n.kind, NodeKind::Text { .. })
        {
            n.kind = NodeKind::Text { content };
            self.dirty = true; self.layout_dirty = true;
        }
    }

    /// Replace the `src` path of an image node. No-op for non-image
    /// nodes. Subsequent paint will look up the new src in the
    /// ImageRegistry (which lazy-loads on first reference).
    pub fn set_img_src(&mut self, id: NodeId, src: String) {
        if let Some(n) = self.nodes.get_mut(id.0 as usize).and_then(|s| s.as_mut())
            && matches!(n.kind, NodeKind::Img { .. })
        {
            n.kind = NodeKind::Img { src };
            // paint-only: a fixed-size <img>'s pixels change, not the layout.
            self.dirty = true;
        }
    }

    pub fn get(&self, id: NodeId) -> Option<&Node> {
        self.nodes.get(id.0 as usize).and_then(|s| s.as_ref())
    }

    pub fn dirty(&self) -> bool {
        self.dirty
    }

    pub fn clear_dirty(&mut self) {
        self.dirty = false;
    }

    /// True if a layout-affecting change happened since the last
    /// [`Self::clear_layout_dirty`] — the runtime recomputes layout only
    /// then, otherwise it reuses the cached `ComputedLayout` map.
    pub fn is_layout_dirty(&self) -> bool {
        self.layout_dirty
    }

    pub fn clear_layout_dirty(&mut self) {
        self.layout_dirty = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_assigns_unique_ids() {
        let mut t = Tree::new();
        let a = t.create(NodeKind::Div, Style::default());
        let b = t.create(NodeKind::Div, Style::default());
        assert_ne!(a, b);
        assert_eq!(a.0, 1); // first non-reserved id
    }

    #[test]
    fn append_child_links_both_directions() {
        let mut t = Tree::new();
        let p = t.create(NodeKind::Div, Style::default());
        let c = t.create(NodeKind::Div, Style::default());
        t.append_child(p, c);
        assert_eq!(t.get(p).unwrap().children, vec![c]);
        assert_eq!(t.get(c).unwrap().parent, p);
    }

    #[test]
    fn remove_recursively_drops_descendants() {
        let mut t = Tree::new();
        let p = t.create(NodeKind::Div, Style::default());
        let c = t.create(NodeKind::Div, Style::default());
        t.append_child(p, c);
        t.remove(p);
        assert!(t.get(p).is_none());
        assert!(t.get(c).is_none());
    }

    #[test]
    fn dirty_flips_on_mutation() {
        let mut t = Tree::new();
        assert!(!t.dirty());
        t.create(NodeKind::Div, Style::default());
        assert!(t.dirty());
        t.clear_dirty();
        assert!(!t.dirty());
    }

    #[test]
    fn set_style_paint_only_change_skips_layout_dirty() {
        let mut t = Tree::new();
        let a = t.create(NodeKind::Div, Style::default());
        // Simulate a completed layout pass.
        t.clear_layout_dirty();
        assert!(!t.is_layout_dirty());

        // Paint-only change (opacity): repaint-dirty, NOT layout-dirty.
        let paint = Style { opacity: Some(0.5), ..Style::default() };
        t.set_style(a, paint.clone());
        assert!(t.dirty());
        assert!(
            !t.is_layout_dirty(),
            "an opacity-only change must reuse the cached layout"
        );

        // Layout change (width) on top of the paint field: must relayout.
        let layout = Style { width: Some(100.0), ..paint };
        t.set_style(a, layout);
        assert!(
            t.is_layout_dirty(),
            "a width change must force a relayout"
        );
    }
}
