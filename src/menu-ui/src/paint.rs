//! Paint a [`Tree`] into a `menu_core_host::Frame`.
//!
//! N1: walk depth-first, emit a `FILL_RECT` for every node that has a
//! `background-color`. Position is `top` / `left` (default `0`); size
//! is `width` / `height` falling back to the framebuffer dimensions
//! for the root.

use menu_core_host::device::FramebufferConfig;
use menu_core_host::error::DeviceError;
use menu_core_host::frame::Frame;
use menu_core_host::protocol::{BlendMode, Rect, Rgba};

use crate::vdom::{NodeId, Tree};

/// Walk the tree rooted at `root` and paint each node into `frame`.
/// Returns the (possibly chained-onto) frame so the caller can append
/// `present()` + `submit()` afterwards.
pub fn paint<'a>(
    tree: &Tree,
    root: NodeId,
    fb: &FramebufferConfig,
    mut frame: Frame<'a>,
) -> Result<Frame<'a>, DeviceError> {
    // Background clear: paint the root's background color over the
    // whole framebuffer (or black if the root has none). Subsequent
    // children are positioned absolutely on top.
    let root_node = match tree.get(root) {
        Some(n) => n,
        None => return Ok(frame),
    };
    let bg = root_node.style.background_color.unwrap_or(Rgba::BLACK);
    frame = frame.fill_rect_unclipped(
        Rect::new(0, 0, fb.width, fb.height),
        bg,
        BlendMode::Opaque,
    )?;

    // Children of the root are painted at their own (left, top, w, h).
    for &child in &root_node.children {
        frame = paint_subtree(tree, child, 0, 0, frame)?;
    }
    Ok(frame)
}

fn paint_subtree<'a>(
    tree: &Tree,
    id: NodeId,
    parent_x: i32,
    parent_y: i32,
    mut frame: Frame<'a>,
) -> Result<Frame<'a>, DeviceError> {
    let Some(node) = tree.get(id) else {
        return Ok(frame);
    };
    let x = parent_x + node.style.left.unwrap_or(0) as i32;
    let y = parent_y + node.style.top.unwrap_or(0) as i32;
    let w = node.style.width.unwrap_or(0);
    let h = node.style.height.unwrap_or(0);

    if let Some(color) = node.style.background_color
        && w > 0
        && h > 0
    {
        // Clamp against u16 just in case JS hands us a negative.
        let dx = x.max(0).min(u16::MAX as i32) as u16;
        let dy = y.max(0).min(u16::MAX as i32) as u16;
        frame = frame.fill_rect_unclipped(Rect::new(dx, dy, w, h), color, BlendMode::Opaque)?;
    }

    for &child in &node.children {
        frame = paint_subtree(tree, child, x, y, frame)?;
    }
    Ok(frame)
}
