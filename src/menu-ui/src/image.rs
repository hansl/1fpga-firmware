//! Image registry: PNG decode + texture cache.
//!
//! N5 supports PNG only. `<img src="...">` references a filesystem
//! path; the registry decodes once per unique `src`, uploads the
//! pixel data as an RGBA8888 texture, and caches the resulting
//! `TextureHandle` plus intrinsic dimensions. Subsequent references
//! to the same `src` are pure hashmap lookups.
//!
//! Images that fail to load (missing file, bad PNG, format unsupported)
//! return a `Failed` entry so we don't retry every frame; the paint
//! path skips drawing them.

use std::collections::HashMap;
use std::io::BufReader;
use std::path::Path;

use menu_core_host::device::Device;
use menu_core_host::error::DeviceError;
use menu_core_host::protocol::TextureFormat;
use menu_core_host::texture::{TextureHandle, TextureSpec};

#[derive(Debug, thiserror::Error)]
pub enum ImageError {
    #[error("io reading image '{path}': {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("png decode failed for '{path}': {detail}")]
    Decode { path: String, detail: String },
    #[error("device error uploading image '{path}': {source}")]
    Device {
        path: String,
        #[source]
        source: DeviceError,
    },
}

/// One cached image. `Loaded` carries the texture + intrinsic dims;
/// `Failed` records the error string so paint can skip without
/// retrying every frame.
#[derive(Debug, Clone)]
pub enum CachedImage {
    Loaded {
        texture: TextureHandle,
        width: u16,
        height: u16,
        /// True when every pixel in the source had alpha == 0xFF (or
        /// the source format has no alpha channel at all). Lets the
        /// paint path pick the Opaque blend fast path, which is
        /// write-only — half the DDR traffic of SrcAlpha for these
        /// images. The wallpaper is the dominant example: with
        /// SrcAlpha its 1920×1080 paint costs ~28 ms per frame (read
        /// + write), with Opaque ~14 ms (write only).
        fully_opaque: bool,
    },
    Failed {
        reason: String,
    },
}

/// Path-keyed image cache. Lookup-and-load is `&mut` because misses
/// upload a new texture; once cached, lookups are read-only.
///
/// `max_dims` is the render-target dimensions: any decoded image
/// larger than this in either axis is downscaled (aspect-preserving,
/// `image::imageops::Triangle`) before being uploaded. This makes
/// the runtime resolution-agnostic without forcing assets to ship
/// at the exact FB size — a 1920×1080 wallpaper PNG used at 1280×720
/// resizes once at startup instead of paying per-pixel FPGA scaling
/// on every frame's wallpaper paint (the COPY_RECT scale path is
/// much slower than the 1:1 burst path).
#[derive(Debug, Default)]
pub struct ImageRegistry {
    entries: HashMap<String, CachedImage>,
    max_dims: Option<(u16, u16)>,
}

impl ImageRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Cap the dimensions of any image loaded after this call.
    /// Larger images are aspect-fit-resized; smaller ones (icons,
    /// glyphs) are left at native size.
    pub fn set_max_dims(&mut self, width: u16, height: u16) {
        self.max_dims = Some((width, height));
    }

    /// Get-or-load. Returns the cached entry for `src`, decoding
    /// + uploading on the first reference. Failures are cached as
    /// `CachedImage::Failed` so we don't pound the disk every frame.
    pub fn get_or_load(&mut self, device: &mut Device, src: &str) -> CachedImage {
        if let Some(entry) = self.entries.get(src) {
            return entry.clone();
        }
        let entry = match decode_and_upload(device, src, self.max_dims) {
            Ok((texture, width, height, fully_opaque)) => CachedImage::Loaded {
                texture,
                width,
                height,
                fully_opaque,
            },
            Err(e) => {
                tracing::warn!("image '{src}' failed to load: {e}");
                CachedImage::Failed {
                    reason: format!("{e}"),
                }
            }
        };
        self.entries.insert(src.to_string(), entry.clone());
        entry
    }

    /// Read-only lookup. Returns `None` if `src` hasn't been seen
    /// before. Used by layout's measure function — paint already
    /// owns `&mut device` so it goes through `get_or_load`.
    pub fn get(&self, src: &str) -> Option<&CachedImage> {
        self.entries.get(src)
    }
}

fn decode_and_upload(
    device: &mut Device,
    src: &str,
    max_dims: Option<(u16, u16)>,
) -> Result<(TextureHandle, u16, u16, bool), ImageError> {
    let file = std::fs::File::open(Path::new(src)).map_err(|e| ImageError::Io {
        path: src.to_string(),
        source: e,
    })?;
    let decoder = png::Decoder::new(BufReader::new(file));
    let mut reader = decoder.read_info().map_err(|e| ImageError::Decode {
        path: src.to_string(),
        detail: e.to_string(),
    })?;
    let info = reader.info();
    let width = info.width as u16;
    let height = info.height as u16;
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let frame = reader.next_frame(&mut buf).map_err(|e| ImageError::Decode {
        path: src.to_string(),
        detail: e.to_string(),
    })?;
    let info = reader.info();

    // Convert to BGRA8888 (the FB / texture format) regardless of
    // the input PNG's color type. We use a small per-source-format
    // expander rather than pulling in the heavier `image` crate
    // for decoding.
    let pixels = expand_to_bgra(
        &buf[..frame.buffer_size()],
        info.color_type,
        info.bit_depth,
        width,
        height,
    )
    .map_err(|d| ImageError::Decode {
        path: src.to_string(),
        detail: d,
    })?;

    // RGB / Grayscale sources always become alpha=0xFF; RGBA /
    // GrayscaleAlpha need a scan. Scanning the converted BGRA is
    // straightforward — every 4th byte is the alpha.
    let src_opaque = match info.color_type {
        png::ColorType::Rgb | png::ColorType::Grayscale => true,
        png::ColorType::Rgba | png::ColorType::GrayscaleAlpha => {
            pixels.chunks_exact(4).all(|px| px[3] == 0xFF)
        }
        png::ColorType::Indexed => false,
    };

    // Aspect-fit downscale if the source exceeds the render target.
    // The FPGA's COPY_RECT scale path reads per-pixel (no burst) and
    // is much slower than the 1:1 burst path; resizing once at load
    // means every subsequent frame's wallpaper paint runs the fast
    // path. Small images (icons, glyphs) are below max_dims in both
    // axes and pass through unchanged.
    let (final_w, final_h, final_pixels) = match max_dims {
        Some((mw, mh)) if width > mw || height > mh => {
            let (rw, rh) = aspect_fit(width, height, mw, mh);
            let resized = resize_bgra(&pixels, width, height, rw, rh);
            tracing::info!("image '{src}' downscaled {width}x{height} → {rw}x{rh} (max {mw}x{mh})");
            (rw, rh, resized)
        }
        _ => (width, height, pixels),
    };

    let texture = device
        .upload_texture(&TextureSpec {
            format: TextureFormat::Rgba8888,
            width: final_w,
            height: final_h,
            stride: (final_w as u32) * 4,
            data: &final_pixels,
        })
        .map_err(|e| ImageError::Device {
            path: src.to_string(),
            source: e,
        })?;
    tracing::info!(
        "image '{}' loaded: {}x{}, tex_id={}, opaque={}",
        src,
        final_w,
        final_h,
        texture.id,
        src_opaque,
    );
    Ok((texture, final_w, final_h, src_opaque))
}

/// Largest (w, h) within `(max_w, max_h)` that preserves the source's
/// aspect ratio. Rounded to nearest integer; both dims at least 1.
fn aspect_fit(src_w: u16, src_h: u16, max_w: u16, max_h: u16) -> (u16, u16) {
    let sx = max_w as f32 / src_w as f32;
    let sy = max_h as f32 / src_h as f32;
    let s = sx.min(sy);
    let w = ((src_w as f32 * s).round() as u16).max(1);
    let h = ((src_h as f32 * s).round() as u16).max(1);
    (w, h)
}

/// Resize a packed BGRA8888 buffer with a Lanczos3 filter. Uses the
/// `image` crate because rolling a proper filter by hand isn't worth
/// it; the cost lands once per load, never per frame. Lanczos3 over
/// Triangle (bilinear) gives noticeably sharper output for static
/// wallpapers — at the small extra CPU cost of a one-shot operation
/// this is a clear win.
fn resize_bgra(pixels: &[u8], src_w: u16, src_h: u16, dst_w: u16, dst_h: u16) -> Vec<u8> {
    // `image::RgbaImage` stores RGBA in memory order. Our buffer is
    // BGRA. Build the wrapper as if it were RGBA — the channel swap
    // is irrelevant to the resize math (each channel is filtered
    // independently). The output stays in BGRA byte order.
    let buf = image::RgbaImage::from_raw(src_w as u32, src_h as u32, pixels.to_vec())
        .expect("BGRA buffer length matches src dims");
    let resized = image::imageops::resize(
        &buf,
        dst_w as u32,
        dst_h as u32,
        image::imageops::FilterType::Lanczos3,
    );
    resized.into_raw()
}

/// Convert PNG-decoded bytes to in-memory BGRA8888 (the FB / texture
/// format). Supports the most common 8-bit color types: RGB, RGBA,
/// Grayscale, GrayscaleAlpha, and Indexed (palette) is rejected for
/// now (PROTOCOL.md §6.2 doesn't expose paletted formats).
fn expand_to_bgra(
    src: &[u8],
    color: png::ColorType,
    depth: png::BitDepth,
    width: u16,
    height: u16,
) -> Result<Vec<u8>, String> {
    if depth != png::BitDepth::Eight {
        return Err(format!("unsupported bit depth {depth:?}; only 8-bit supported"));
    }
    let n_pixels = (width as usize) * (height as usize);
    let mut out = vec![0u8; n_pixels * 4];
    match color {
        png::ColorType::Rgba => {
            // R G B A -> B G R A
            for (i, chunk) in src.chunks_exact(4).enumerate() {
                let off = i * 4;
                out[off] = chunk[2];
                out[off + 1] = chunk[1];
                out[off + 2] = chunk[0];
                out[off + 3] = chunk[3];
            }
        }
        png::ColorType::Rgb => {
            for (i, chunk) in src.chunks_exact(3).enumerate() {
                let off = i * 4;
                out[off] = chunk[2];
                out[off + 1] = chunk[1];
                out[off + 2] = chunk[0];
                out[off + 3] = 0xFF;
            }
        }
        png::ColorType::GrayscaleAlpha => {
            for (i, chunk) in src.chunks_exact(2).enumerate() {
                let off = i * 4;
                let g = chunk[0];
                out[off] = g;
                out[off + 1] = g;
                out[off + 2] = g;
                out[off + 3] = chunk[1];
            }
        }
        png::ColorType::Grayscale => {
            for (i, &g) in src.iter().enumerate() {
                let off = i * 4;
                out[off] = g;
                out[off + 1] = g;
                out[off + 2] = g;
                out[off + 3] = 0xFF;
            }
        }
        png::ColorType::Indexed => {
            return Err("indexed/paletted PNG not supported (use RGB/RGBA)".into());
        }
    }
    Ok(out)
}
