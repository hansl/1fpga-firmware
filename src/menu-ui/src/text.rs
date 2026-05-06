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
use menu_core_host::texture::TextureHandle;

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

/// Cache key uniquely identifying a rendered text line. A change in
/// any of these fields requires a fresh raster (and a fresh cache
/// entry).
#[derive(Debug, Hash, Eq, PartialEq, Clone)]
pub struct CacheKey {
    pub content: String,
    pub font_name: String,
    pub px_size: u16,
    /// `Rgba` packed as a `u32` so the key is `Hash`/`Eq`-cheap.
    pub color: u32,
}

/// One cached pre-rendered text line: a render-target texture sized
/// to the glyph extent, with the resolved color baked in.
#[derive(Debug, Clone, Copy)]
pub struct CachedLine {
    pub texture: TextureHandle,
    pub width: u16,
    pub height: u16,
}

/// Work item produced by [`TextCache::populate`]: the caller renders
/// glyphs into `texture` once during the next [`Frame`], after which
/// repeated frames just `COPY_RECT` the cached texture.
///
/// [`Frame`]: menu_core_host::frame::Frame
#[derive(Debug, Clone)]
pub struct PendingRender {
    pub texture: TextureHandle,
    pub width: u16,
    pub height: u16,
    pub content: String,
    pub font_name: String,
    pub px_size: u16,
    pub color: Rgba,
}

/// Persistent cache of rendered text lines. Indexed by
/// `(content, font, px_size, color)`. Cache entries hold a
/// `TextureHandle` into the device texture pool — they survive across
/// frames and across React re-renders that keep the same text props.
///
/// Eviction is not implemented (v1). Long-running apps with rapidly
/// changing text content will eventually exhaust the texture pool;
/// future work adds LRU eviction (the `last_used_frame` field already
/// exists for that).
#[derive(Debug, Default)]
pub struct TextCache {
    entries: HashMap<CacheKey, CachedLine>,
}

impl TextCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Look up a cached line. Used by paint.
    pub fn lookup(&self, key: &CacheKey) -> Option<&CachedLine> {
        self.entries.get(key)
    }

    /// Number of cached lines (for diagnostics).
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// For every text node in `tree`, ensure a cache entry exists for
    /// its `(content, font, size, color)`. New entries allocate a
    /// render-target texture sized to the glyph extent; the caller
    /// must render glyphs into each returned `PendingRender` during
    /// the next frame, before using the cache for paint.
    pub fn populate(
        &mut self,
        tree: &Tree,
        text_styles: &HashMap<NodeId, ResolvedTextStyle>,
        fonts: &FontRegistry,
        device: &mut Device,
    ) -> Result<Vec<PendingRender>, FontError> {
        let mut pendings = Vec::new();
        for (id, rs) in text_styles {
            let Some(node) = tree.get(*id) else {
                continue;
            };
            let NodeKind::Text { content } = &node.kind else {
                continue;
            };
            if content.is_empty() {
                continue;
            }
            let key = CacheKey {
                content: content.clone(),
                font_name: rs.font_name.clone(),
                px_size: rs.px_size.round() as u16,
                color: rs.color.to_u32(),
            };
            if self.entries.contains_key(&key) {
                continue;
            }
            let cached_atlas = match fonts.get(&rs.font_name, key.px_size) {
                Some(c) => c,
                None => continue, // prepare() failed for this font; skip
            };
            let w = cached_atlas.atlas.measure(content) as u16;
            let h = cached_atlas.atlas.line_height;
            if w == 0 || h == 0 {
                continue;
            }
            let texture = device.create_render_target(w, h)?;
            let cached = CachedLine {
                texture,
                width: w,
                height: h,
            };
            self.entries.insert(key.clone(), cached);
            pendings.push(PendingRender {
                texture,
                width: w,
                height: h,
                content: content.clone(),
                font_name: rs.font_name.clone(),
                px_size: key.px_size,
                color: rs.color,
            });
        }
        Ok(pendings)
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
