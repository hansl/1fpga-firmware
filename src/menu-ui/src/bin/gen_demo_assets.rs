//! Generate the N9 demo's background + per-system icon PNGs.
//!
//! Run on the host (not the device):
//!   cargo run --bin gen_demo_assets
//!
//! Writes into `docs/assets/menu-ui-demo/` (relative to the workspace
//! root). The output PNGs are committed; this binary only needs to
//! re-run when we change the demo's visual style.

use std::fs::{self, File};
use std::io::BufWriter;
use std::path::{Path, PathBuf};

const BG_W: u32 = 1920;
const BG_H: u32 = 1080;
const ICON: u32 = 128;

fn main() -> std::io::Result<()> {
    let root = workspace_root();
    let out_dir = root.join("docs/assets/menu-ui-demo");
    fs::create_dir_all(&out_dir)?;

    // Convert the existing background.jpg into a 16:9 PNG suitable
    // for the wallpaper image element. We pre-darken the RGB during
    // conversion so the host can render it with `opacity: 1.0` and
    // the FPGA can use the Opaque blend path — saves a dst read per
    // pixel relative to the previous "opacity: 0.55 over root_bg"
    // approach, which is the dominant per-frame cost.
    let src_jpg = root.join("src/firmware-script/assets/background.jpg");
    convert_wallpaper(&src_jpg, &out_dir.join("bg.png"))?;

    write_png(&out_dir.join("nes.png"),     ICON, ICON, &icon_nes())?;
    write_png(&out_dir.join("snes.png"),    ICON, ICON, &icon_snes())?;
    write_png(&out_dir.join("genesis.png"), ICON, ICON, &icon_genesis())?;
    write_png(&out_dir.join("gameboy.png"), ICON, ICON, &icon_gameboy())?;
    write_png(&out_dir.join("atari.png"),   ICON, ICON, &icon_atari())?;

    println!("wrote 6 PNGs to {}", out_dir.display());
    Ok(())
}

/// Load `src` (JPG or PNG), centre-crop to 16:9 against the
/// framebuffer's 1920×1080 target, multiply each pixel's RGB by
/// `DARKEN` so the foreground UI reads cleanly without needing a
/// runtime opacity blend, and write `dst` as an opaque PNG.
fn convert_wallpaper(src: &Path, dst: &Path) -> std::io::Result<()> {
    /// How much of the original luminance to keep. 0.55 matches the
    /// previous in-engine `opacity: 0.55` value the App was using.
    const DARKEN: f32 = 0.55;

    let img = image::ImageReader::open(src)
        .map_err(|e| std::io::Error::other(format!("open {}: {e}", src.display())))?
        .decode()
        .map_err(|e| std::io::Error::other(format!("decode {}: {e}", src.display())))?
        .to_rgb8();
    let (w, h) = (img.width(), img.height());

    // Centre-crop to BG_W × BG_H — keep the central strip when the
    // source is taller than 16:9.
    let target_aspect = BG_W as f32 / BG_H as f32;
    let src_aspect = w as f32 / h as f32;
    let (crop_w, crop_h) = if src_aspect > target_aspect {
        // Source is wider — crop sides.
        ((h as f32 * target_aspect) as u32, h)
    } else {
        // Source is taller — crop top + bottom.
        (w, (w as f32 / target_aspect) as u32)
    };
    let x0 = (w - crop_w) / 2;
    let y0 = (h - crop_h) / 2;
    let cropped = image::imageops::crop_imm(&img, x0, y0, crop_w, crop_h).to_image();

    // Resize to exact BG_W × BG_H. Use a triangle (linear) filter —
    // fast and adequate for a softly-textured wallpaper.
    let resized = image::imageops::resize(
        &cropped,
        BG_W,
        BG_H,
        image::imageops::FilterType::Triangle,
    );

    // Darken + emit as RGBA8 (alpha = 0xFF so the FPGA paint can
    // pick the Opaque blend fast path).
    let mut rgba = Vec::with_capacity((BG_W * BG_H * 4) as usize);
    for px in resized.pixels() {
        let [r, g, b] = px.0;
        let darken = |c: u8| ((c as f32 * DARKEN).clamp(0.0, 255.0)) as u8;
        rgba.push(darken(r));
        rgba.push(darken(g));
        rgba.push(darken(b));
        rgba.push(0xFF);
    }
    write_png(dst, BG_W, BG_H, &rgba)
}

/// Walk up from CARGO_MANIFEST_DIR until we find a `Cargo.lock` (the
/// workspace root). Cargo runs us from the crate dir, so this lands us
/// at the repo root regardless of how the binary was invoked.
fn workspace_root() -> PathBuf {
    let mut p: PathBuf = env!("CARGO_MANIFEST_DIR").into();
    while !p.join("Cargo.lock").exists() {
        if !p.pop() {
            panic!("no Cargo.lock found above CARGO_MANIFEST_DIR");
        }
    }
    p
}

fn write_png(path: &Path, w: u32, h: u32, rgba: &[u8]) -> std::io::Result<()> {
    assert_eq!(rgba.len(), (w * h * 4) as usize, "{:?}", path);
    let file = File::create(path)?;
    let mut enc = png::Encoder::new(BufWriter::new(file), w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc
        .write_header()
        .map_err(|e| std::io::Error::other(format!("png header: {e}")))?;
    writer
        .write_image_data(rgba)
        .map_err(|e| std::io::Error::other(format!("png data: {e}")))?;
    Ok(())
}

// ---- Icons ----------------------------------------------------------
//
// Every icon is 128×128 with an 8px transparent margin, then a rounded-
// square body in the system's accent color, then a simple white glyph.
// Glyph shapes are kept primitive (rect + circle + cross) so we can
// draw them in raw pixel space without a font rasterizer. The point is
// only that each system shows a distinct, recognisable mark — not that
// it's pixel-perfect art.

/// Build an empty icon canvas (all transparent).
fn icon_canvas() -> Vec<u8> {
    vec![0u8; (ICON * ICON * 4) as usize]
}

#[inline]
fn put(buf: &mut [u8], x: u32, y: u32, rgba: [u8; 4]) {
    if x >= ICON || y >= ICON {
        return;
    }
    let i = ((y * ICON + x) * 4) as usize;
    buf[i..i + 4].copy_from_slice(&rgba);
}

/// Fill a rounded rect (centred axis-aligned) with `fill` and a 2-px
/// outer border in `border`. `radius` is in pixels.
fn rounded_rect(
    buf: &mut [u8],
    x0: u32,
    y0: u32,
    x1: u32,
    y1: u32,
    radius: u32,
    fill: [u8; 4],
    border: Option<[u8; 4]>,
) {
    for y in y0..y1 {
        for x in x0..x1 {
            if !inside_rounded(x as i32, y as i32, x0 as i32, y0 as i32, x1 as i32, y1 as i32, radius as i32) {
                continue;
            }
            // 2-px border ring: pixels within `radius+2` of the outer
            // boundary but inside the rounded region.
            let on_border = border.is_some()
                && near_rounded_edge(
                    x as i32, y as i32, x0 as i32, y0 as i32, x1 as i32, y1 as i32, radius as i32, 2,
                );
            let color = if on_border { border.unwrap() } else { fill };
            put(buf, x, y, color);
        }
    }
}

fn inside_rounded(x: i32, y: i32, x0: i32, y0: i32, x1: i32, y1: i32, r: i32) -> bool {
    // Outside the bounding rect: out.
    if x < x0 || x >= x1 || y < y0 || y >= y1 {
        return false;
    }
    // Pick the nearest corner; if x/y is in the straight section, the
    // distance check trivially passes.
    let nx = if x < x0 + r { x0 + r } else if x >= x1 - r { x1 - r - 1 } else { x };
    let ny = if y < y0 + r { y0 + r } else if y >= y1 - r { y1 - r - 1 } else { y };
    let dx = x - nx;
    let dy = y - ny;
    dx * dx + dy * dy <= r * r
}

fn near_rounded_edge(
    x: i32, y: i32, x0: i32, y0: i32, x1: i32, y1: i32, r: i32, thickness: i32,
) -> bool {
    if !inside_rounded(x, y, x0, y0, x1, y1, r) {
        return false;
    }
    !inside_rounded(x, y, x0 + thickness, y0 + thickness, x1 - thickness, y1 - thickness, (r - thickness).max(0))
}

/// Filled disc of `radius` centred on (cx, cy).
fn disc(buf: &mut [u8], cx: i32, cy: i32, radius: i32, color: [u8; 4]) {
    for y in (cy - radius)..=(cy + radius) {
        for x in (cx - radius)..=(cx + radius) {
            let dx = x - cx;
            let dy = y - cy;
            if dx * dx + dy * dy <= radius * radius {
                put(buf, x.max(0) as u32, y.max(0) as u32, color);
            }
        }
    }
}

/// Solid filled rect (axis-aligned, no rounding).
fn solid_rect(buf: &mut [u8], x0: i32, y0: i32, x1: i32, y1: i32, color: [u8; 4]) {
    for y in y0..y1 {
        for x in x0..x1 {
            put(buf, x.max(0) as u32, y.max(0) as u32, color);
        }
    }
}

const W: [u8; 4] = [0xFF, 0xFF, 0xFF, 0xFF];

fn icon_with_body(body: [u8; 4]) -> Vec<u8> {
    let mut buf = icon_canvas();
    rounded_rect(&mut buf, 8, 8, ICON - 8, ICON - 8, 16, body, Some(W));
    buf
}

/// NES — red body + white "controller" detail (a horizontal pill).
fn icon_nes() -> Vec<u8> {
    let mut buf = icon_with_body([0xD0, 0x40, 0x40, 0xFF]);
    // D-pad cross — two thin white rects forming a "+".
    solid_rect(&mut buf, 44, 56, 84, 72, W);
    solid_rect(&mut buf, 56, 44, 72, 84, W);
    buf
}

/// SNES — blue body + four small dots arranged as the Y/X/A/B layout.
fn icon_snes() -> Vec<u8> {
    let mut buf = icon_with_body([0x60, 0x70, 0xFF, 0xFF]);
    disc(&mut buf, 64, 40, 8, W);  // top (X)
    disc(&mut buf, 88, 64, 8, W);  // right (A)
    disc(&mut buf, 64, 88, 8, W);  // bottom (B)
    disc(&mut buf, 40, 64, 8, W);  // left (Y)
    buf
}

/// Genesis — neutral dark body + three horizontal stripes.
fn icon_genesis() -> Vec<u8> {
    let mut buf = icon_with_body([0x40, 0x40, 0x40, 0xFF]);
    for &y in &[44i32, 60, 76] {
        solid_rect(&mut buf, 32, y, 96, y + 8, W);
    }
    buf
}

/// Game Boy — green body + a small "screen" rectangle inside.
fn icon_gameboy() -> Vec<u8> {
    let mut buf = icon_with_body([0x80, 0xA0, 0x40, 0xFF]);
    // Outer screen border (white), then dark inner fill so it reads
    // like a screen embedded on the device.
    solid_rect(&mut buf, 36, 32, 92, 80, W);
    solid_rect(&mut buf, 40, 36, 88, 76, [0x10, 0x18, 0x10, 0xFF]);
    // Tiny start/select pills near the bottom.
    solid_rect(&mut buf, 48, 92, 60, 96, W);
    solid_rect(&mut buf, 68, 92, 80, 96, W);
    buf
}

/// Atari — orange body + three concentric squares (the abstract
/// Atari-logo "fuji" idea simplified).
fn icon_atari() -> Vec<u8> {
    let mut buf = icon_with_body([0xD0, 0x90, 0x40, 0xFF]);
    // Three increasingly-inset borders.
    for (inset, _thick) in [(28i32, 2), (40, 2), (52, 2)] {
        let x0 = inset;
        let y0 = inset;
        let x1 = ICON as i32 - inset;
        let y1 = ICON as i32 - inset;
        // Outline only.
        solid_rect(&mut buf, x0, y0, x1, y0 + 2, W);
        solid_rect(&mut buf, x0, y1 - 2, x1, y1, W);
        solid_rect(&mut buf, x0, y0, x0 + 2, y1, W);
        solid_rect(&mut buf, x1 - 2, y0, x1, y1, W);
    }
    buf
}
