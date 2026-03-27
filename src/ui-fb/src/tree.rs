use crate::dom::{Node, NodeId, NodeType, StyleProps};

/// Arena-based DOM tree. Nodes are stored in a Vec with stable indices.
/// Removed nodes are added to a free list for reuse.
pub struct DomTree {
    nodes: Vec<Option<Node>>,
    free_list: Vec<NodeId>,
    root: NodeId,
    dirty: bool,
}

impl DomTree {
    /// Create a new DomTree with a root node.
    pub fn new() -> Self {
        let root = Node {
            id: 0,
            node_type: NodeType::Root,
            parent: None,
            children: Vec::new(),
            style: StyleProps::default(),
            text_content: None,
            image_src: None,
        };
        Self {
            nodes: vec![Some(root)],
            free_list: Vec::new(),
            root: 0,
            dirty: true,
        }
    }

    /// Get the root node ID.
    pub fn root(&self) -> NodeId {
        self.root
    }

    /// Whether the tree has been modified since the last layout computation.
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Mark the tree as clean (after layout computation).
    pub fn mark_clean(&mut self) {
        self.dirty = false;
    }

    /// Mark the tree as dirty (needs re-layout).
    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    /// Get a reference to a node by ID.
    pub fn get(&self, id: NodeId) -> Option<&Node> {
        self.nodes.get(id as usize).and_then(|n| n.as_ref())
    }

    /// Get a mutable reference to a node by ID.
    pub fn get_mut(&mut self, id: NodeId) -> Option<&mut Node> {
        self.nodes.get_mut(id as usize).and_then(|n| n.as_mut())
    }

    /// Create a new node and return its ID.
    pub fn create_node(&mut self, node_type: NodeType) -> NodeId {
        let id = if let Some(id) = self.free_list.pop() {
            id
        } else {
            let id = self.nodes.len() as NodeId;
            self.nodes.push(None);
            id
        };

        self.nodes[id as usize] = Some(Node {
            id,
            node_type,
            parent: None,
            children: Vec::new(),
            style: StyleProps::default(),
            text_content: None,
            image_src: None,
        });
        self.dirty = true;
        id
    }

    /// Remove a node and all its descendants from the tree.
    pub fn remove_node(&mut self, id: NodeId) {
        if id == self.root {
            return; // Cannot remove root
        }

        // Detach from parent
        if let Some(parent_id) = self.get(id).and_then(|n| n.parent) {
            if let Some(parent) = self.get_mut(parent_id) {
                parent.children.retain(|&child| child != id);
            }
        }

        // Collect all descendants to remove
        let mut to_remove = vec![id];
        let mut i = 0;
        while i < to_remove.len() {
            let node_id = to_remove[i];
            if let Some(node) = self.get(node_id) {
                to_remove.extend_from_slice(&node.children.clone());
            }
            i += 1;
        }

        // Remove all collected nodes
        for node_id in to_remove {
            self.nodes[node_id as usize] = None;
            self.free_list.push(node_id);
        }
        self.dirty = true;
    }

    /// Append a child to a parent node.
    pub fn append_child(&mut self, parent_id: NodeId, child_id: NodeId) {
        // Detach from old parent if any
        if let Some(old_parent_id) = self.get(child_id).and_then(|n| n.parent) {
            if let Some(old_parent) = self.get_mut(old_parent_id) {
                old_parent.children.retain(|&c| c != child_id);
            }
        }

        // Set new parent
        if let Some(child) = self.get_mut(child_id) {
            child.parent = Some(parent_id);
        }
        if let Some(parent) = self.get_mut(parent_id) {
            parent.children.push(child_id);
        }
        self.dirty = true;
    }

    /// Insert a child before another child in the parent's children list.
    pub fn insert_before(&mut self, parent_id: NodeId, child_id: NodeId, before_id: NodeId) {
        // Detach from old parent if any
        if let Some(old_parent_id) = self.get(child_id).and_then(|n| n.parent) {
            if let Some(old_parent) = self.get_mut(old_parent_id) {
                old_parent.children.retain(|&c| c != child_id);
            }
        }

        // Set new parent
        if let Some(child) = self.get_mut(child_id) {
            child.parent = Some(parent_id);
        }
        if let Some(parent) = self.get_mut(parent_id) {
            if let Some(pos) = parent.children.iter().position(|&c| c == before_id) {
                parent.children.insert(pos, child_id);
            } else {
                parent.children.push(child_id);
            }
        }
        self.dirty = true;
    }

    /// Remove a child from its parent (does not delete the node).
    pub fn remove_child(&mut self, parent_id: NodeId, child_id: NodeId) {
        if let Some(parent) = self.get_mut(parent_id) {
            parent.children.retain(|&c| c != child_id);
        }
        if let Some(child) = self.get_mut(child_id) {
            child.parent = None;
        }
        self.dirty = true;
    }

    /// Set the style properties of a node.
    pub fn set_style(&mut self, id: NodeId, style: StyleProps) {
        if let Some(node) = self.get_mut(id) {
            node.style = style;
            self.dirty = true;
        }
    }

    /// Update a single style property of a node via a closure.
    pub fn update_style(&mut self, id: NodeId, f: impl FnOnce(&mut StyleProps)) {
        if let Some(node) = self.get_mut(id) {
            f(&mut node.style);
            self.dirty = true;
        }
    }

    /// Set the text content of a Text node.
    pub fn set_text(&mut self, id: NodeId, text: String) {
        if let Some(node) = self.get_mut(id) {
            node.text_content = Some(text);
            self.dirty = true;
        }
    }

    /// Set the image source of an Image node.
    pub fn set_image_src(&mut self, id: NodeId, src: String) {
        if let Some(node) = self.get_mut(id) {
            node.image_src = Some(src);
            self.dirty = true;
        }
    }
}

impl Default for DomTree {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_and_get() {
        let mut tree = DomTree::new();
        let root = tree.root();
        assert_eq!(tree.get(root).unwrap().node_type, NodeType::Root);

        let child = tree.create_node(NodeType::View);
        tree.append_child(root, child);
        assert_eq!(tree.get(child).unwrap().parent, Some(root));
        assert_eq!(tree.get(root).unwrap().children, vec![child]);
    }

    #[test]
    fn test_remove_node() {
        let mut tree = DomTree::new();
        let root = tree.root();
        let child = tree.create_node(NodeType::View);
        let grandchild = tree.create_node(NodeType::Text);

        tree.append_child(root, child);
        tree.append_child(child, grandchild);

        tree.remove_node(child);
        assert!(tree.get(child).is_none());
        assert!(tree.get(grandchild).is_none());
        assert!(tree.get(root).unwrap().children.is_empty());
    }

    #[test]
    fn test_insert_before() {
        let mut tree = DomTree::new();
        let root = tree.root();
        let a = tree.create_node(NodeType::View);
        let b = tree.create_node(NodeType::View);
        let c = tree.create_node(NodeType::View);

        tree.append_child(root, a);
        tree.append_child(root, c);
        tree.insert_before(root, b, c);

        assert_eq!(tree.get(root).unwrap().children, vec![a, b, c]);
    }

    #[test]
    fn test_free_list_reuse() {
        let mut tree = DomTree::new();
        let root = tree.root();
        let a = tree.create_node(NodeType::View);
        tree.append_child(root, a);
        tree.remove_node(a);

        let b = tree.create_node(NodeType::View);
        // Should reuse the freed slot
        assert_eq!(b, a);
    }
}
