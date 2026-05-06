//! Layout computation via Taffy.
//!
//! Builds a Taffy tree from the vdom each pass and computes layout
//! against the framebuffer viewport. Text nodes get a measure function
//! that consults the font atlas; div nodes flow through Taffy's
//! flexbox / block algorithm.
//!
//! For menu-sized trees, full rebuild per pass is fast enough — Taffy
//! is sub-millisecond. An incremental sync layer can replace this
//! later if profiling demands it.

use std::collections::HashMap;

use taffy::TaffyTree;
use taffy::prelude::*;

use crate::font::FontRegistry;
use crate::style::{
    AlignItems as MAlignItems, Display as MDisplay, FlexDirection as MFlexDirection,
    FlexWrap as MFlexWrap, JustifyContent as MJustifyContent, Position as MPosition, Style,
};
use crate::text::ResolvedTextStyle;
use crate::vdom::{NodeId, NodeKind, Tree};

/// Computed absolute layout for one host node.
#[derive(Debug, Default, Clone, Copy)]
pub struct ComputedLayout {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// Per-Taffy-node context so the measure function knows what to ask
/// the font atlas for.
#[derive(Debug, Clone)]
struct NodeContext {
    text: Option<TextContext>,
}

#[derive(Debug, Clone)]
struct TextContext {
    content: String,
    font_name: String,
    px_size: u16,
}

/// Compute layouts for every node reachable from `root`. Sizes the
/// root against `(fb_w, fb_h)` (the framebuffer viewport).
pub fn compute(
    tree: &Tree,
    root: NodeId,
    fb_w: f32,
    fb_h: f32,
    text_styles: &HashMap<NodeId, ResolvedTextStyle>,
    fonts: &FontRegistry,
) -> HashMap<NodeId, ComputedLayout> {
    let mut taffy: TaffyTree<NodeContext> = TaffyTree::new();
    let mut node_map: HashMap<NodeId, taffy::NodeId> = HashMap::new();
    let root_t = build(&mut taffy, tree, &mut node_map, text_styles, root);

    let _ = taffy.compute_layout_with_measure(
        root_t,
        Size {
            width: AvailableSpace::Definite(fb_w),
            height: AvailableSpace::Definite(fb_h),
        },
        |_known, _avail, _node, ctx, _style| measure(ctx, fonts),
    );

    let mut layouts = HashMap::new();
    walk_layouts(&taffy, &node_map, &mut layouts, tree, root, 0.0, 0.0);
    layouts
}

fn measure(ctx: Option<&mut NodeContext>, fonts: &FontRegistry) -> Size<f32> {
    let Some(ctx) = ctx.and_then(|c| c.text.as_ref()) else {
        return Size::ZERO;
    };
    if let Some(cached) = fonts.get(&ctx.font_name, ctx.px_size) {
        let w = cached.atlas.measure(&ctx.content) as f32;
        let h = cached.atlas.line_height as f32;
        Size {
            width: w,
            height: h,
        }
    } else {
        // Atlas wasn't pre-built — fall back to a heuristic so layout
        // still gets some answer rather than 0×0. (Should never happen
        // when prepare() ran first.)
        let est_w = (ctx.content.chars().count() as f32) * (ctx.px_size as f32 * 0.5);
        Size {
            width: est_w,
            height: ctx.px_size as f32,
        }
    }
}

fn build(
    taffy: &mut TaffyTree<NodeContext>,
    tree: &Tree,
    node_map: &mut HashMap<NodeId, taffy::NodeId>,
    text_styles: &HashMap<NodeId, ResolvedTextStyle>,
    id: NodeId,
) -> taffy::NodeId {
    let Some(node) = tree.get(id) else {
        return taffy
            .new_leaf(taffy::Style::default())
            .expect("create leaf");
    };
    let style = to_taffy_style(&node.style);

    let tnode = match &node.kind {
        NodeKind::Div => {
            let children: Vec<taffy::NodeId> = node
                .children
                .iter()
                .map(|&c| build(taffy, tree, node_map, text_styles, c))
                .collect();
            taffy
                .new_with_children(style, &children)
                .expect("create div node")
        }
        NodeKind::Text { content } => {
            let resolved = text_styles
                .get(&id)
                .cloned()
                .unwrap_or_default();
            let ctx = NodeContext {
                text: Some(TextContext {
                    content: content.clone(),
                    font_name: resolved.font_name,
                    px_size: resolved.px_size.round() as u16,
                }),
            };
            taffy
                .new_leaf_with_context(style, ctx)
                .expect("create text leaf")
        }
    };
    node_map.insert(id, tnode);
    tnode
}

fn walk_layouts(
    taffy: &TaffyTree<NodeContext>,
    node_map: &HashMap<NodeId, taffy::NodeId>,
    layouts: &mut HashMap<NodeId, ComputedLayout>,
    tree: &Tree,
    id: NodeId,
    parent_x: f32,
    parent_y: f32,
) {
    let Some(&tnode) = node_map.get(&id) else {
        return;
    };
    let lay = taffy.layout(tnode).expect("computed layout");
    let x = parent_x + lay.location.x;
    let y = parent_y + lay.location.y;
    layouts.insert(
        id,
        ComputedLayout {
            x,
            y,
            w: lay.size.width,
            h: lay.size.height,
        },
    );
    if let Some(node) = tree.get(id) {
        for &child in &node.children {
            walk_layouts(taffy, node_map, layouts, tree, child, x, y);
        }
    }
}

fn to_taffy_style(s: &Style) -> taffy::Style {
    let mut t = taffy::Style::default();

    t.display = match s.display.unwrap_or_default() {
        MDisplay::Block => taffy::Display::Block,
        MDisplay::Flex => taffy::Display::Flex,
    };
    t.position = match s.position.unwrap_or_default() {
        MPosition::Relative => taffy::Position::Relative,
        MPosition::Absolute => taffy::Position::Absolute,
    };
    t.flex_direction = match s.flex_direction.unwrap_or_default() {
        MFlexDirection::Row => taffy::FlexDirection::Row,
        MFlexDirection::Column => taffy::FlexDirection::Column,
        MFlexDirection::RowReverse => taffy::FlexDirection::RowReverse,
        MFlexDirection::ColumnReverse => taffy::FlexDirection::ColumnReverse,
    };
    t.flex_wrap = match s.flex_wrap.unwrap_or_default() {
        MFlexWrap::NoWrap => taffy::FlexWrap::NoWrap,
        MFlexWrap::Wrap => taffy::FlexWrap::Wrap,
        MFlexWrap::WrapReverse => taffy::FlexWrap::WrapReverse,
    };
    t.justify_content = s.justify_content.map(|j| match j {
        MJustifyContent::FlexStart => taffy::JustifyContent::FlexStart,
        MJustifyContent::FlexEnd => taffy::JustifyContent::FlexEnd,
        MJustifyContent::Center => taffy::JustifyContent::Center,
        MJustifyContent::SpaceBetween => taffy::JustifyContent::SpaceBetween,
        MJustifyContent::SpaceAround => taffy::JustifyContent::SpaceAround,
        MJustifyContent::SpaceEvenly => taffy::JustifyContent::SpaceEvenly,
    });
    t.align_items = s.align_items.map(map_align_items);
    t.align_self = s.align_self.map(map_align_items);

    if let Some(g) = s.flex_grow {
        t.flex_grow = g;
    }
    if let Some(g) = s.flex_shrink {
        t.flex_shrink = g;
    }
    if let Some(b) = s.flex_basis {
        t.flex_basis = length(b);
    }
    if let Some(g) = s.gap {
        t.gap = Size {
            width: length(g),
            height: length(g),
        };
    }

    if let Some(v) = s.width {
        t.size.width = length(v);
    }
    if let Some(v) = s.height {
        t.size.height = length(v);
    }
    if let Some(v) = s.min_width {
        t.min_size.width = length(v);
    }
    if let Some(v) = s.min_height {
        t.min_size.height = length(v);
    }
    if let Some(v) = s.max_width {
        t.max_size.width = length(v);
    }
    if let Some(v) = s.max_height {
        t.max_size.height = length(v);
    }

    t.padding = taffy::Rect {
        top: length(s.padding_top.unwrap_or(0.0)),
        right: length(s.padding_right.unwrap_or(0.0)),
        bottom: length(s.padding_bottom.unwrap_or(0.0)),
        left: length(s.padding_left.unwrap_or(0.0)),
    };
    t.margin = taffy::Rect {
        top: length(s.margin_top.unwrap_or(0.0)),
        right: length(s.margin_right.unwrap_or(0.0)),
        bottom: length(s.margin_bottom.unwrap_or(0.0)),
        left: length(s.margin_left.unwrap_or(0.0)),
    };
    t.inset = taffy::Rect {
        top: s.top.map(length).unwrap_or_else(|| auto()),
        right: s.right.map(length).unwrap_or_else(|| auto()),
        bottom: s.bottom.map(length).unwrap_or_else(|| auto()),
        left: s.left.map(length).unwrap_or_else(|| auto()),
    };

    t
}

fn map_align_items(a: MAlignItems) -> taffy::AlignItems {
    match a {
        MAlignItems::Stretch => taffy::AlignItems::Stretch,
        MAlignItems::FlexStart => taffy::AlignItems::FlexStart,
        MAlignItems::FlexEnd => taffy::AlignItems::FlexEnd,
        MAlignItems::Center => taffy::AlignItems::Center,
        MAlignItems::Baseline => taffy::AlignItems::Baseline,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::font::FontRegistry;
    use crate::style::{AlignItems, Display, JustifyContent};
    use crate::vdom::{NodeKind, Tree};

    #[test]
    fn flex_centering_places_child_in_middle() {
        let mut tree = Tree::new();
        let root = tree.create(
            NodeKind::Div,
            Style {
                display: Some(Display::Flex),
                width: Some(1000.0),
                height: Some(800.0),
                justify_content: Some(JustifyContent::Center),
                align_items: Some(AlignItems::Center),
                ..Style::default()
            },
        );
        let child = tree.create(
            NodeKind::Div,
            Style {
                width: Some(100.0),
                height: Some(50.0),
                ..Style::default()
            },
        );
        tree.append_child(root, child);

        let text_styles = HashMap::new();
        let fonts = FontRegistry::new();
        let layouts = compute(&tree, root, 1000.0, 800.0, &text_styles, &fonts);
        let lay = layouts.get(&child).expect("child layout");
        assert!((lay.x - 450.0).abs() < 0.5, "x={}", lay.x);
        assert!((lay.y - 375.0).abs() < 0.5, "y={}", lay.y);
        assert_eq!(lay.w as i32, 100);
        assert_eq!(lay.h as i32, 50);
    }

    #[test]
    fn block_default_stacks_vertically() {
        let mut tree = Tree::new();
        let root = tree.create(
            NodeKind::Div,
            Style {
                width: Some(500.0),
                height: Some(500.0),
                ..Style::default()
            },
        );
        let a = tree.create(
            NodeKind::Div,
            Style {
                width: Some(500.0),
                height: Some(100.0),
                ..Style::default()
            },
        );
        let b = tree.create(
            NodeKind::Div,
            Style {
                width: Some(500.0),
                height: Some(150.0),
                ..Style::default()
            },
        );
        tree.append_child(root, a);
        tree.append_child(root, b);

        let text_styles = HashMap::new();
        let fonts = FontRegistry::new();
        let layouts = compute(&tree, root, 500.0, 500.0, &text_styles, &fonts);
        assert_eq!(layouts[&a].y as i32, 0);
        assert_eq!(layouts[&b].y as i32, 100);
    }
}
