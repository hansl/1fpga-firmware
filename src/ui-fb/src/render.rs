use embedded_graphics::pixelcolor::Rgb888;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::{PrimitiveStyle, Rectangle};

use crate::animation::AnimationController;
use crate::dom::{NodeId, NodeType};
use crate::image_cache::ImageCache;
use crate::layout::LayoutEngine;
use crate::text;
use crate::tree::DomTree;

/// Renders the DOM tree to any DrawTarget.
pub struct Renderer {
    image_cache: ImageCache,
}

impl Renderer {
    pub fn new() -> Self {
        Self {
            image_cache: ImageCache::new(),
        }
    }

    /// Render the full tree to a DrawTarget.
    /// The caller is responsible for clearing and flipping the buffer.
    pub fn render(
        &mut self,
        tree: &DomTree,
        layout: &LayoutEngine,
        animations: &AnimationController,
        target: &mut impl DrawTarget<Color = Rgb888>,
    ) {
        let _ = target.clear(Rgb888::BLACK);
        self.render_node(tree, layout, animations, tree.root(), target);
    }

    fn render_node(
        &mut self,
        tree: &DomTree,
        layout: &LayoutEngine,
        animations: &AnimationController,
        node_id: NodeId,
        target: &mut impl DrawTarget<Color = Rgb888>,
    ) {
        let Some(node) = tree.get(node_id) else {
            return;
        };
        let Some(rect) = layout.get_layout(node_id) else {
            return;
        };

        // Get animated state
        let anim_state = animations.get_state(node_id);

        // Check opacity
        let opacity = anim_state
            .and_then(|s| s.opacity)
            .or(node.style.opacity)
            .unwrap_or(1.0);
        if opacity <= 0.0 {
            return;
        }

        // Apply animation translate
        let translate_x = anim_state.and_then(|s| s.translate_x).unwrap_or(0.0);
        let translate_y = anim_state.and_then(|s| s.translate_y).unwrap_or(0.0);

        let x = (rect.x + translate_x) as i32;
        let y = (rect.y + translate_y) as i32;
        let w = rect.width as u32;
        let h = rect.height as u32;

        if w == 0 || h == 0 {
            return;
        }

        match node.node_type {
            NodeType::Root => {}
            NodeType::View => {
                if let Some(bg) = node.style.background_color {
                    let bg = if opacity < 1.0 {
                        blend_with_opacity(bg, opacity)
                    } else {
                        bg
                    };
                    let _ = Rectangle::new(Point::new(x, y), Size::new(w, h))
                        .into_styled(PrimitiveStyle::with_fill(bg))
                        .draw(target);
                }
            }
            NodeType::Image => {
                if let Some(ref src) = node.image_src {
                    self.render_image(target, src, x, y, w, h);
                }
            }
            NodeType::Text => {
                if let Some(ref content) = node.text_content {
                    let color = node.style.color.unwrap_or(Rgb888::WHITE);
                    let font_size = node.style.font_size.unwrap_or(16);
                    text::render_text(target, content, x, y, color, font_size);
                }
            }
        }

        // Render children (clone to avoid borrow conflict with self)
        let children = node.children.clone();
        for child_id in children {
            self.render_node(tree, layout, animations, child_id, target);
        }
    }

    fn render_image(
        &mut self,
        target: &mut impl DrawTarget<Color = Rgb888>,
        src: &str,
        x: i32,
        y: i32,
        w: u32,
        h: u32,
    ) {
        let Some(cached) = self.image_cache.get_or_load(src, w, h) else {
            return;
        };

        for iy in 0..cached.height {
            for ix in 0..cached.width {
                let px = cached.pixels[(iy * cached.width + ix) as usize];
                let _ = Pixel(Point::new(x + ix as i32, y + iy as i32), px).draw(target);
            }
        }
    }
}

impl Default for Renderer {
    fn default() -> Self {
        Self::new()
    }
}

fn blend_with_opacity(color: Rgb888, opacity: f32) -> Rgb888 {
    Rgb888::new(
        (color.r() as f32 * opacity) as u8,
        (color.g() as f32 * opacity) as u8,
        (color.b() as f32 * opacity) as u8,
    )
}
