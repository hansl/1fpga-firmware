//! Text style resolution + the prepare phase that ensures every
//! `(font, size)` referenced by text nodes has a built atlas before
//! layout / paint run.
//!
//! CSS inheritance is simple here: `color`, `fontFamily`, `fontSize`
//! flow from a `<div>` to its descendants until overridden. We resolve
//! once per frame into a `HashMap<NodeId, ResolvedTextStyle>` so
//! layout's measure function and paint share the same answer.

use std::collections::HashMap;

use menu_core_host::device::Device;
use menu_core_host::protocol::Rgba;

use crate::font::{DEFAULT_FONT_NAME, FontError, FontRegistry};
use crate::style::Style;
use crate::vdom::{NodeId, NodeKind, Tree};

pub const DEFAULT_FONT_SIZE: f32 = 24.0;
pub const DEFAULT_TEXT_COLOR: Rgba = Rgba::new(0xFF, 0xFF, 0xFF, 0xFF);

/// What a text node ends up actually using to render — derived from
/// its own style merged with its ancestors' (CSS inheritance).
#[derive(Debug, Clone)]
pub struct ResolvedTextStyle {
    pub font_name: String,
    pub px_size: f32,
    pub color: Rgba,
}

impl Default for ResolvedTextStyle {
    fn default() -> Self {
        Self {
            font_name: DEFAULT_FONT_NAME.to_string(),
            px_size: DEFAULT_FONT_SIZE,
            color: DEFAULT_TEXT_COLOR,
        }
    }
}

/// Walk the tree from `root`, recording each node's resolved text
/// style. Used by prepare / layout / paint as a shared lookup.
pub fn resolve(tree: &Tree, root: NodeId) -> HashMap<NodeId, ResolvedTextStyle> {
    let mut out = HashMap::new();
    walk(tree, root, &ResolvedTextStyle::default(), &mut out);
    out
}

fn walk(
    tree: &Tree,
    id: NodeId,
    parent: &ResolvedTextStyle,
    out: &mut HashMap<NodeId, ResolvedTextStyle>,
) {
    let Some(node) = tree.get(id) else {
        return;
    };
    let here = merge(parent, &node.style);
    out.insert(id, here.clone());
    for &child in &node.children {
        walk(tree, child, &here, out);
    }
}

fn merge(parent: &ResolvedTextStyle, here: &Style) -> ResolvedTextStyle {
    ResolvedTextStyle {
        font_name: here
            .font_family
            .clone()
            .unwrap_or_else(|| parent.font_name.clone()),
        px_size: here.font_size.unwrap_or(parent.px_size),
        color: here.color.unwrap_or(parent.color),
    }
}

/// Ensure every `(font, px_size)` pair used by text nodes in the tree
/// has a built+uploaded atlas. Called before layout each frame; cheap
/// when nothing new is needed (just hashmap lookups in `ensure`).
pub fn prepare(
    tree: &Tree,
    resolved: &HashMap<NodeId, ResolvedTextStyle>,
    registry: &mut FontRegistry,
    device: &mut Device,
) -> Result<(), FontError> {
    for (id, rs) in resolved {
        let Some(node) = tree.get(*id) else {
            continue;
        };
        if matches!(node.kind, NodeKind::Text { .. }) {
            registry.ensure(device, &rs.font_name, rs.px_size.round() as u16)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style::Style;
    use crate::vdom::Tree;

    #[test]
    fn text_inherits_color_from_ancestor() {
        let mut tree = Tree::new();
        let root = tree.create(
            NodeKind::Div,
            Style {
                color: Some(Rgba::new(0xFF, 0x00, 0x00, 0xFF)),
                font_size: Some(48.0),
                ..Style::default()
            },
        );
        let child = tree.create(NodeKind::Div, Style::default());
        let text = tree.create(NodeKind::Text { content: "hi".into() }, Style::default());
        tree.append_child(root, child);
        tree.append_child(child, text);

        let resolved = resolve(&tree, root);
        let r = resolved.get(&text).expect("text resolved");
        assert_eq!(r.color, Rgba::new(0xFF, 0x00, 0x00, 0xFF));
        assert_eq!(r.px_size, 48.0);
        assert_eq!(r.font_name, "default");
    }

    #[test]
    fn text_overrides_inherited_size() {
        let mut tree = Tree::new();
        let root = tree.create(
            NodeKind::Div,
            Style {
                font_size: Some(48.0),
                ..Style::default()
            },
        );
        let text = tree.create(
            NodeKind::Text { content: "hi".into() },
            Style {
                font_size: Some(16.0),
                ..Style::default()
            },
        );
        tree.append_child(root, text);

        let resolved = resolve(&tree, root);
        assert_eq!(resolved[&text].px_size, 16.0);
    }
}
