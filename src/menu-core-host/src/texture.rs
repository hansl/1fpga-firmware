//! Texture upload and handles.
//!
//! Textures live in the DDR3 texture pool described in PROTOCOL.md §2.
//! [`Device::upload_texture`] copies pixel data into the pool, writes
//! a 32-byte descriptor into the descriptor table, bumps
//! `TEX_TABLE_COUNT`, and returns a [`TextureHandle`] suitable for
//! [`Frame::copy_rect`]. Texture lifetimes are bound to the
//! [`Device`]; there is no destructor — uploads accumulate until the
//! device is closed (or an upcoming reset op is added).
//!
//! [`Device::upload_texture`]: crate::device::Device::upload_texture
//! [`Device`]: crate::device::Device
//! [`Frame::copy_rect`]: crate::frame::Frame::copy_rect

use crate::protocol::TextureFormat;

/// Source description for [`Device::upload_texture`].
///
/// [`Device::upload_texture`]: crate::device::Device::upload_texture
#[derive(Debug)]
pub struct TextureSpec<'a> {
    pub format: TextureFormat,
    pub width: u16,
    pub height: u16,
    /// Bytes between successive scanlines in `data`. Must be at least
    /// `width * format.bytes_per_pixel()`. Larger values are accepted
    /// — they let you upload from a sub-region of a larger atlas.
    pub stride: u32,
    pub data: &'a [u8],
}

/// Handle to a texture uploaded into the DDR3 texture pool.
///
/// Cheap to clone (`Copy`); pass by reference or value to
/// [`Frame::copy_rect`]. The `id` field is the descriptor index the
/// FPGA reads from the descriptor table; the other fields are
/// informational copies so callers can compute source rects without
/// holding a separate metadata struct.
///
/// [`Frame::copy_rect`]: crate::frame::Frame::copy_rect
#[derive(Debug, Copy, Clone)]
pub struct TextureHandle {
    pub id: u16,
    pub width: u16,
    pub height: u16,
    pub format: TextureFormat,
    pub phys_addr: u32,
}
