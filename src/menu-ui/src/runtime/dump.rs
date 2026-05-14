//! On-shutdown diagnostic: snapshot the staging RT and the three FB
//! slots straight from DDR3 to PNG. Lets us inspect what the host
//! actually wrote vs. what HDMI showed — narrows down whether
//! artifacts come from the host paint, the FB copies, or scanout.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use menu_core_host::devmem::DevMemMap;
use tracing::{info, warn};

/// Map `phys` for `stride * height` bytes, copy pixels row-by-row
/// converting BGRA → RGBA, and write a PNG to `out_path`.
///
/// `stride` is in bytes; we read `width * 4` bytes per row starting
/// at every `stride`-byte boundary.
pub fn dump_region(
    phys: u32,
    width: u16,
    height: u16,
    stride: u32,
    out_path: &Path,
) -> std::io::Result<()> {
    let region_bytes = (stride as usize) * (height as usize);
    let mut map = DevMemMap::create(phys, region_bytes).map_err(|e| {
        std::io::Error::other(format!(
            "dump: mmap {:#010X}+{} failed: {:?}",
            phys, region_bytes, e
        ))
    })?;

    let row_bytes = (width as usize) * 4;
    let mut rgba: Vec<u8> = Vec::with_capacity(row_bytes * height as usize);
    let src_ptr = map.as_mut_ptr() as *const u8;
    for y in 0..height as usize {
        let row_off = y * (stride as usize);
        for x in 0..width as usize {
            let px_off = row_off + x * 4;
            // SAFETY: region is mapped for `stride * height` bytes,
            // and we only read within `width * 4` of each row.
            unsafe {
                let b = std::ptr::read_volatile(src_ptr.add(px_off));
                let g = std::ptr::read_volatile(src_ptr.add(px_off + 1));
                let r = std::ptr::read_volatile(src_ptr.add(px_off + 2));
                let a = std::ptr::read_volatile(src_ptr.add(px_off + 3));
                rgba.push(r);
                rgba.push(g);
                rgba.push(b);
                rgba.push(a);
            }
        }
    }

    let file = File::create(out_path)?;
    let w = BufWriter::new(file);
    let mut encoder = png::Encoder::new(w, width as u32, height as u32);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder
        .write_header()
        .map_err(|e| std::io::Error::other(format!("png header: {e}")))?;
    writer
        .write_image_data(&rgba)
        .map_err(|e| std::io::Error::other(format!("png data: {e}")))?;
    writer
        .finish()
        .map_err(|e| std::io::Error::other(format!("png finish: {e}")))?;
    Ok(())
}

/// Convenience wrapper: log on success/failure, never propagate the
/// error (this is diagnostic-only and shouldn't kill clean shutdown).
pub fn try_dump(
    label: &str,
    phys: u32,
    width: u16,
    height: u16,
    stride: u32,
    out_path: &Path,
) {
    match dump_region(phys, width, height, stride, out_path) {
        Ok(()) => info!(
            "dump: {} → {} ({}×{}, phys={:#010X})",
            label,
            out_path.display(),
            width,
            height,
            phys
        ),
        Err(e) => warn!("dump: {} failed: {}", label, e),
    }
    // Flush the log line before continuing.
    let _ = std::io::stdout().flush();
}
