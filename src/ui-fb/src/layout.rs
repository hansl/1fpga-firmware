use std::collections::HashMap;

use taffy::prelude::*;

use crate::dom::{
    AlignItems, FlexDirection, JustifyContent, NodeId, NodeType, PositionType, StyleProps,
};
use crate::text::measure_text;
use crate::tree::DomTree;

/// Computed position and size for a node after layout.
#[derive(Debug, Clone, Copy, Default)]
pub struct ComputedRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// Context attached to taffy leaf nodes that need custom measurement (text nodes).
struct MeasureContext {
    dom_node_id: NodeId,
}

/// Manages the taffy layout tree and maps between DOM NodeIds and taffy NodeIds.
pub struct LayoutEngine {
    taffy: TaffyTree<MeasureContext>,
    dom_to_taffy: HashMap<NodeId, taffy::NodeId>,
    taffy_to_dom: HashMap<taffy::NodeId, NodeId>,
    layouts: HashMap<NodeId, ComputedRect>,
}

impl LayoutEngine {
    pub fn new() -> Self {
        Self {
            taffy: TaffyTree::new(),
            dom_to_taffy: HashMap::new(),
            taffy_to_dom: HashMap::new(),
            layouts: HashMap::new(),
        }
    }

    /// Get the computed layout for a node.
    pub fn get_layout(&self, id: NodeId) -> Option<ComputedRect> {
        self.layouts.get(&id).copied()
    }

    /// Synchronize the taffy tree with the DomTree and compute layout.
    pub fn compute(&mut self, tree: &DomTree, viewport_width: u32, viewport_height: u32) {
        // Clear old state
        self.taffy.clear();
        self.dom_to_taffy.clear();
        self.taffy_to_dom.clear();
        self.layouts.clear();

        // Build taffy tree from DOM tree
        let root_id = tree.root();
        self.build_taffy_node(tree, root_id, viewport_width, viewport_height);

        // Compute layout
        if let Some(&taffy_root) = self.dom_to_taffy.get(&root_id) {
            let available = Size {
                width: AvailableSpace::Definite(viewport_width as f32),
                height: AvailableSpace::Definite(viewport_height as f32),
            };

            self.taffy
                .compute_layout_with_measure(
                    taffy_root,
                    available,
                    |known_size, _available, _node_id, context, _style| {
                        if let Some(ctx) = context {
                            let node = tree.get(ctx.dom_node_id);
                            if let Some(node) = node {
                                if let Some(text) = &node.text_content {
                                    let font_size = node.style.font_size.unwrap_or(16);
                                    let (w, h) = measure_text(text, font_size);
                                    return Size {
                                        width: known_size.width.unwrap_or(w as f32),
                                        height: known_size.height.unwrap_or(h as f32),
                                    };
                                }
                            }
                        }
                        Size::ZERO
                    },
                )
                .ok();

            // Read back computed layouts
            self.read_layouts(taffy_root, 0.0, 0.0);
        }
    }

    fn build_taffy_node(
        &mut self,
        tree: &DomTree,
        dom_id: NodeId,
        viewport_width: u32,
        viewport_height: u32,
    ) -> Option<taffy::NodeId> {
        let node = tree.get(dom_id)?;
        let style = convert_style(&node.style, node.node_type, viewport_width, viewport_height);

        let taffy_id = if node.node_type == NodeType::Text {
            // Text nodes are leaf nodes with a measure function
            self.taffy
                .new_leaf_with_context(
                    style,
                    MeasureContext {
                        dom_node_id: dom_id,
                    },
                )
                .ok()?
        } else {
            // Build children first
            let child_ids: Vec<taffy::NodeId> = node
                .children
                .iter()
                .filter_map(|&child_id| {
                    self.build_taffy_node(tree, child_id, viewport_width, viewport_height)
                })
                .collect();

            self.taffy.new_with_children(style, &child_ids).ok()?
        };

        self.dom_to_taffy.insert(dom_id, taffy_id);
        self.taffy_to_dom.insert(taffy_id, dom_id);
        Some(taffy_id)
    }

    fn read_layouts(&mut self, taffy_id: taffy::NodeId, parent_x: f32, parent_y: f32) {
        let Ok(layout) = self.taffy.layout(taffy_id) else {
            return;
        };
        let x = parent_x + layout.location.x;
        let y = parent_y + layout.location.y;
        let rect = ComputedRect {
            x,
            y,
            width: layout.size.width,
            height: layout.size.height,
        };

        if let Some(&dom_id) = self.taffy_to_dom.get(&taffy_id) {
            self.layouts.insert(dom_id, rect);
        }

        let children: Vec<_> = self
            .taffy
            .children(taffy_id)
            .unwrap_or_default()
            .to_vec();
        for child in children {
            self.read_layouts(child, x, y);
        }
    }
}

impl Default for LayoutEngine {
    fn default() -> Self {
        Self::new()
    }
}

fn convert_style(
    props: &StyleProps,
    node_type: NodeType,
    viewport_width: u32,
    viewport_height: u32,
) -> Style {
    let mut style = Style::default();

    // Display: always flex for containers
    style.display = match node_type {
        NodeType::Root | NodeType::View => Display::Flex,
        NodeType::Text | NodeType::Image => Display::Flex,
    };

    // Size
    if let Some(w) = props.width {
        style.size.width = if w <= 1.0 && w > 0.0 {
            Dimension::Percent(w)
        } else {
            length(w)
        };
    }
    if let Some(h) = props.height {
        style.size.height = if h <= 1.0 && h > 0.0 {
            Dimension::Percent(h)
        } else {
            length(h)
        };
    }

    // Root node fills viewport
    if node_type == NodeType::Root {
        style.size.width = length(viewport_width as f32);
        style.size.height = length(viewport_height as f32);
    }

    // Flex properties
    if let Some(dir) = props.flex_direction {
        style.flex_direction = match dir {
            FlexDirection::Row => taffy::FlexDirection::Row,
            FlexDirection::Column => taffy::FlexDirection::Column,
        };
    }

    if let Some(jc) = props.justify_content {
        style.justify_content = Some(match jc {
            JustifyContent::FlexStart => taffy::JustifyContent::FlexStart,
            JustifyContent::FlexEnd => taffy::JustifyContent::FlexEnd,
            JustifyContent::Center => taffy::JustifyContent::Center,
            JustifyContent::SpaceBetween => taffy::JustifyContent::SpaceBetween,
            JustifyContent::SpaceAround => taffy::JustifyContent::SpaceAround,
            JustifyContent::SpaceEvenly => taffy::JustifyContent::SpaceEvenly,
        });
    }

    if let Some(ai) = props.align_items {
        style.align_items = Some(match ai {
            AlignItems::FlexStart => taffy::AlignItems::FlexStart,
            AlignItems::FlexEnd => taffy::AlignItems::FlexEnd,
            AlignItems::Center => taffy::AlignItems::Center,
            AlignItems::Stretch => taffy::AlignItems::Stretch,
            AlignItems::Baseline => taffy::AlignItems::Baseline,
        });
    }

    if let Some(grow) = props.flex_grow {
        style.flex_grow = grow;
    }
    if let Some(shrink) = props.flex_shrink {
        style.flex_shrink = shrink;
    }
    if let Some(basis) = props.flex_basis {
        style.flex_basis = length(basis);
    }

    // Gap
    if let Some(gap) = props.gap {
        style.gap = Size {
            width: length(gap),
            height: length(gap),
        };
    }

    // Position
    if let Some(pos) = props.position_type {
        style.position = match pos {
            PositionType::Relative => taffy::Position::Relative,
            PositionType::Absolute => taffy::Position::Absolute,
        };
    }
    if let Some(top) = props.top {
        style.inset.top = length(top);
    }
    if let Some(left) = props.left {
        style.inset.left = length(left);
    }
    if let Some(right) = props.right {
        style.inset.right = length(right);
    }
    if let Some(bottom) = props.bottom {
        style.inset.bottom = length(bottom);
    }

    // Padding
    let [pt, pr, pb, pl] = props.padding;
    style.padding = Rect {
        top: length(pt),
        right: length(pr),
        bottom: length(pb),
        left: length(pl),
    };

    // Margin
    let [mt, mr, mb, ml] = props.margin;
    style.margin = Rect {
        top: length(mt),
        right: length(mr),
        bottom: length(mb),
        left: length(ml),
    };

    style
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dom::NodeType;
    use crate::tree::DomTree;

    #[test]
    fn test_basic_layout() {
        let mut tree = DomTree::new();
        let root = tree.root();

        let child = tree.create_node(NodeType::View);
        tree.update_style(child, |s| {
            s.width = Some(100.0);
            s.height = Some(50.0);
        });
        tree.append_child(root, child);

        let mut engine = LayoutEngine::new();
        engine.compute(&tree, 800, 600);

        let root_layout = engine.get_layout(root).unwrap();
        assert_eq!(root_layout.width, 800.0);
        assert_eq!(root_layout.height, 600.0);

        let child_layout = engine.get_layout(child).unwrap();
        assert_eq!(child_layout.width, 100.0);
        assert_eq!(child_layout.height, 50.0);
    }

    #[test]
    fn test_centering() {
        let mut tree = DomTree::new();
        let root = tree.root();
        tree.update_style(root, |s| {
            s.justify_content = Some(JustifyContent::Center);
            s.align_items = Some(AlignItems::Center);
        });

        let child = tree.create_node(NodeType::View);
        tree.update_style(child, |s| {
            s.width = Some(200.0);
            s.height = Some(100.0);
        });
        tree.append_child(root, child);

        let mut engine = LayoutEngine::new();
        engine.compute(&tree, 800, 600);

        let child_layout = engine.get_layout(child).unwrap();
        // Centered: (800 - 200) / 2 = 300, (600 - 100) / 2 = 250
        assert_eq!(child_layout.x, 300.0);
        assert_eq!(child_layout.y, 250.0);
    }
}
