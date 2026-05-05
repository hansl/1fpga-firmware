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

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum NodeKind {
    Div,
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
        self.dirty = true;
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
        self.dirty = true;
    }

    pub fn append_child(&mut self, parent: NodeId, child: NodeId) {
        if parent == NodeId::NONE || child == NodeId::NONE {
            return;
        }
        // Detach from previous parent.
        let prev_parent = self.nodes[child.0 as usize]
            .as_ref()
            .map(|n| n.parent)
            .unwrap_or(NodeId::NONE);
        if prev_parent != NodeId::NONE
            && let Some(p) = self.nodes[prev_parent.0 as usize].as_mut() {
            p.children.retain(|&c| c != child);
        }
        if let Some(c) = self.nodes[child.0 as usize].as_mut() {
            c.parent = parent;
        }
        if let Some(p) = self.nodes[parent.0 as usize].as_mut() {
            p.children.push(child);
        }
        self.dirty = true;
    }

    pub fn set_style(&mut self, id: NodeId, style: Style) {
        if let Some(n) = self.nodes.get_mut(id.0 as usize).and_then(|s| s.as_mut()) {
            n.style = style;
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
}
