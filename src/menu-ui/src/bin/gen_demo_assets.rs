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
    let out_dir = workspace_root().join("docs/assets/menu-ui-demo");
    fs::create_dir_all(&out_dir)?;

    write_png(&out_dir.join("bg.png"), BG_W, BG_H, &background())?;
    write_png(&out_dir.join("nes.png"),     ICON, ICON, &icon_nes())?;
    write_png(&out_dir.join("snes.png"),    ICON, ICON, &icon_snes())?;
    write_png(&out_dir.join("genesis.png"), ICON, ICON, &icon_genesis())?;
    write_png(&out_dir.join("gameboy.png"), ICON, ICON, &icon_gameboy())?;
    write_png(&out_dir.join("atari.png"),   ICON, ICON, &icon_atari())?;

    println!("wrote {} PNGs to {}", 6, out_dir.display());
    Ok(())
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

// ---- Background -----------------------------------------------------

/// Subtle radial-ish gradient from a slightly warmer dark blue at the
/// center toward near-black at the edges. Adds vignette so the focused
/// card pops more.
fn background() -> Vec<u8> {
    let cx = (BG_W as f32) * 0.5;
    let cy = (BG_H as f32) * 0.5;
    let r_max = ((cx * cx + cy * cy) as f32).sqrt();
    let mut out = Vec::with_capacity((BG_W * BG_H * 4) as usize);
    for y in 0..BG_H {
        for x in 0..BG_W {
            let dx = x as f32 - cx;
            let dy = y as f32 - cy;
            let r = (dx * dx + dy * dy).sqrt() / r_max;       // 0..1
            // Inner color (center): #181830; outer color (corners): #050510.
            let t = clamp01(r);
            let (ir, ig, ib) = (0x18, 0x18, 0x30);
            let (or_, og, ob) = (0x05, 0x05, 0x10);
            let r8 = lerp(ir, or_, t);
            let g8 = lerp(ig, og, t);
            let b8 = lerp(ib, ob, t);
            out.extend_from_slice(&[r8, g8, b8, 0xFF]);
        }
    }
    out
}

fn lerp(a: u8, b: u8, t: f32) -> u8 {
    let a = a as f32;
    let b = b as f32;
    (a + (b - a) * t).round().clamp(0.0, 255.0) as u8
}

fn clamp01(t: f32) -> f32 {
    t.clamp(0.0, 1.0)
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
