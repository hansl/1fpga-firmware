//! Paint a [`Tree`] into a `menu_core_host::Frame`.
//!
//! N3: walks the tree and paints each node at the position computed by
//! [`crate::layout::compute`] (Taffy). Position math no longer comes
//! from raw `top`/`left` style fields — flexbox + absolute positioning
//! land here through the layout map.

use std::collections::HashMap;

use menu_core_host::device::FramebufferConfig;
use menu_core_host::error::DeviceError;
use menu_core_host::frame::Frame;
use menu_core_host::protocol::{BlendMode, Rect, Rgba};

use crate::layout::ComputedLayout;
use crate::vdom::{NodeId, Tree};

/// Walk `tree` rooted at `root` and paint each node into `frame` using
/// `layouts` for positions. The caller appends `present()` + `submit()`
/// to the returned frame.
pub fn paint<'a>(
    tree: &Tree,
    root: NodeId,
    fb: &FramebufferConfig,
    layouts: &HashMap<NodeId, ComputedLayout>,
    mut frame: Frame<'a>,
) -> Result<Frame<'a>, DeviceError> {
    // Background clear: the root's bg, or black if none.
    let root_bg = tree
        .get(root)
        .and_then(|n| n.style.background_color)
        .unwrap_or(Rgba::BLACK);
    frame = frame.fill_rect_unclipped(
        Rect::new(0, 0, fb.width, fb.height),
        root_bg,
        BlendMode::Opaque,
    )?;

    frame = paint_subtree(tree, root, layouts, frame, /* skip_root_bg */ true)?;
    Ok(frame)
}

fn paint_subtree<'a>(
    tree: &Tree,
    id: NodeId,
    layouts: &HashMap<NodeId, ComputedLayout>,
    mut frame: Frame<'a>,
    skip_root_bg: bool,
) -> Result<Frame<'a>, DeviceError> {
    let Some(node) = tree.get(id) else {
        return Ok(frame);
    };
    let Some(lay) = layouts.get(&id) else {
        return Ok(frame);
    };

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
            frame =
                frame.fill_rect_unclipped(Rect::new(dx, dy, dw, dh), color, BlendMode::Opaque)?;
        }
    }

    for &child in &node.children {
        frame = paint_subtree(tree, child, layouts, frame, false)?;
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
