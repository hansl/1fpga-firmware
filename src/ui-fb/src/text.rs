use embedded_graphics::pixelcolor::Rgb888;
use embedded_graphics::prelude::*;
use u8g2_fonts::types::{FontColor, VerticalPosition};
use u8g2_fonts::FontRenderer;

/// Approximate text measurement. Returns (width, height) in pixels.
///
/// This uses a simple character-width estimation based on font size.
/// Accurate enough for layout purposes in the PoC.
pub fn measure_text(text: &str, font_size: u8) -> (u32, u32) {
    let char_count = text.chars().count() as u32;
    match font_size {
        0..=12 => (char_count * 6, 10),
        13..=18 => (char_count * 8, 16),
        19..=28 => (char_count * 13, 28),
        _ => (char_count * 16, 36),
    }
}

/// Render text onto a DrawTarget at the given position.
pub fn render_text<D>(
    target: &mut D,
    text: &str,
    x: i32,
    y: i32,
    color: Rgb888,
    font_size: u8,
) where
    D: DrawTarget<Color = Rgb888>,
{
    match font_size {
        0..=12 => render_with_font::<
            u8g2_fonts::fonts::u8g2_font_haxrcorp4089_t_cyrillic,
            D,
        >(target, text, x, y, color),
        13..=18 => render_with_font::<
            u8g2_fonts::fonts::u8g2_font_helvR12_tr,
            D,
        >(target, text, x, y, color),
        19..=28 => render_with_font::<
            u8g2_fonts::fonts::u8g2_font_ncenB24_tr,
            D,
        >(target, text, x, y, color),
        _ => render_with_font::<
            u8g2_fonts::fonts::u8g2_font_ncenB24_tr,
            D,
        >(target, text, x, y, color),
    }
}

fn render_with_font<F: u8g2_fonts::Font, D: DrawTarget<Color = Rgb888>>(
    target: &mut D,
    text: &str,
    x: i32,
    y: i32,
    color: Rgb888,
) {
    let font = FontRenderer::new::<F>();
    let _ = font.render(
        text,
        Point::new(x, y),
        VerticalPosition::Top,
        FontColor::Transparent(color),
        target,
    );
}
