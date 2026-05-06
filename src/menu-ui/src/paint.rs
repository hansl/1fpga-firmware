//! Paint a [`Tree`] into a `menu_core_host::Frame`.
//!
//! N4: handles both `<div>` (background fill) and `<text>` (per-glyph
//! `COPY_RECT` from the font atlas, SrcAlpha-blended, tinted with the
//! resolved color). All node positions come from the precomputed
//! layout map.

use std::collections::HashMap;

use menu_core_host::device::FramebufferConfig;
use menu_core_host::error::DeviceError;
use menu_core_host::frame::{CopyOpts, Frame};
use menu_core_host::protocol::{BlendMode, Filter, Rect, Rgba};

use crate::font::FontRegistry;
use crate::layout::ComputedLayout;
use crate::text::ResolvedTextStyle;
use crate::vdom::{NodeId, NodeKind, Tree};

/// Walk the tree and paint each node into `frame`.
pub fn paint<'a>(
    tree: &Tree,
    root: NodeId,
    fb: &FramebufferConfig,
    layouts: &HashMap<NodeId, ComputedLayout>,
    text_styles: &HashMap<NodeId, ResolvedTextStyle>,
    fonts: &FontRegistry,
    mut frame: Frame<'a>,
) -> Result<Frame<'a>, DeviceError> {
    // Background clear.
    let root_bg = tree
        .get(root)
        .and_then(|n| n.style.background_color)
        .unwrap_or(Rgba::BLACK);
    frame = frame.fill_rect_unclipped(
        Rect::new(0, 0, fb.width, fb.height),
        root_bg,
        BlendMode::Opaque,
    )?;

    frame = paint_subtree(
        tree,
        root,
        layouts,
        text_styles,
        fonts,
        frame,
        /* skip_root_bg */ true,
    )?;
    Ok(frame)
}

fn paint_subtree<'a>(
    tree: &Tree,
    id: NodeId,
    layouts: &HashMap<NodeId, ComputedLayout>,
    text_styles: &HashMap<NodeId, ResolvedTextStyle>,
    fonts: &FontRegistry,
    mut frame: Frame<'a>,
    skip_root_bg: bool,
) -> Result<Frame<'a>, DeviceError> {
    let Some(node) = tree.get(id) else {
        return Ok(frame);
    };
    let Some(lay) = layouts.get(&id) else {
        return Ok(frame);
    };

    match &node.kind {
        NodeKind::Div => {
            if !skip_root_bg
                && let Some(color) = node.style.background_color
                && lay.w > 0.5
                && lay.h > 0.5
            {
                let dx = clamp_u16(lay.x);
                let dy = clamp_u16(lay.y);
                let dw = clamp_u16(lay.w);
                let dh = clamp_u16(lay.h);
                if dw > 0 && dh > 0 {
                    frame = frame.fill_rect_unclipped(
                        Rect::new(dx, dy, dw, dh),
                        color,
                        BlendMode::Opaque,
                    )?;
                }
            }
        }
        NodeKind::Text { content } => {
            frame = paint_text(content, id, lay, text_styles, fonts, frame)?;
        }
    }

    for &child in &node.children {
        frame = paint_subtree(tree, child, layouts, text_styles, fonts, frame, false)?;
    }
    Ok(frame)
}

fn paint_text<'a>(
    content: &str,
    id: NodeId,
    lay: &ComputedLayout,
    text_styles: &HashMap<NodeId, ResolvedTextStyle>,
    fonts: &FontRegistry,
    mut frame: Frame<'a>,
) -> Result<Frame<'a>, DeviceError> {
    let resolved = match text_styles.get(&id) {
        Some(r) => r,
        None => return Ok(frame),
    };
    let cached = match fonts.get(&resolved.font_name, resolved.px_size.round() as u16) {
        Some(c) => c,
        None => {
            // Atlas missing — prepare phase didn't run or failed for
            // this font. Skip rather than crash; the canary still
            // proves the loop is alive.
            return Ok(frame);
        }
    };
    let atlas = &cached.atlas;
    let tex = &cached.texture;

    let baseline_y = lay.y as i32 + atlas.ascent as i32;
    let mut pen_x: i32 = lay.x as i32;

    for ch in content.chars() {
        let g = match atlas.glyph(ch) {
            Some(g) => *g,
            None => continue,
        };
        if g.width > 0 && g.height > 0 {
            let dst_x = pen_x + g.bearing_x as i32;
            let dst_y = baseline_y - g.ymin as i32 - g.height as i32;
            if dst_x >= 0 && dst_y >= 0 {
                frame = frame.copy_rect(
                    tex,
                    Rect::new(g.atlas_x, g.atlas_y, g.width, g.height),
                    Rect::new(
                        clamp_u16(dst_x as f32),
                        clamp_u16(dst_y as f32),
                        g.width,
                        g.height,
                    ),
                    CopyOpts {
                        blend: BlendMode::SrcAlpha,
                        filter: Filter::Nearest,
                        tint: Some(resolved.color),
                    },
                )?;
            }
        }
        pen_x += g.advance as i32;
    }
    Ok(frame)
}

#[inline]
fn clamp_u16(v: f32) -> u16 {
    if v <= 0.0 {
        0
    } else if v >= u16::MAX as f32 {
        u16::MAX
    } else {
        v as u16
    }
}
