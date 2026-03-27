use std::collections::HashMap;

use embedded_graphics::pixelcolor::Rgb888;
use image::GenericImageView;

/// Pre-decoded image stored as Rgb888 pixels, ready for blitting to framebuffer.
pub struct CachedImage {
    pub pixels: Vec<Rgb888>,
    pub width: u32,
    pub height: u32,
}

/// Cache for decoded and resized images.
pub struct ImageCache {
    cache: HashMap<String, CachedImage>,
}

impl ImageCache {
    pub fn new() -> Self {
        Self {
            cache: HashMap::new(),
        }
    }

    /// Get a cached image, or load and cache it.
    /// The `src` can be:
    /// - `"embedded:background"` for the built-in background image
    /// - A filesystem path for runtime-loaded images
    pub fn get_or_load(
        &mut self,
        src: &str,
        target_width: u32,
        target_height: u32,
    ) -> Option<&CachedImage> {
        let key = format!("{src}:{target_width}x{target_height}");

        if !self.cache.contains_key(&key) {
            let img = if src == "embedded:background" {
                load_embedded_background()
            } else {
                load_from_path(src)
            }?;

            let resized = img.resize_exact(
                target_width,
                target_height,
                image::imageops::FilterType::Triangle,
            );

            let pixels = decode_to_rgb888(&resized);

            self.cache.insert(
                key.clone(),
                CachedImage {
                    pixels,
                    width: target_width,
                    height: target_height,
                },
            );
        }

        self.cache.get(&key)
    }

    /// Invalidate a specific cached image.
    pub fn invalidate(&mut self, src: &str) {
        self.cache.retain(|k, _| !k.starts_with(src));
    }
}

impl Default for ImageCache {
    fn default() -> Self {
        Self::new()
    }
}

fn load_embedded_background() -> Option<image::DynamicImage> {
    // Embedded background image. Uses the same asset as the existing firmware.
    static BACKGROUND_BYTES: &[u8] =
        include_bytes!("../../firmware-script/assets/background.jpg");
    image::load_from_memory(BACKGROUND_BYTES).ok()
}

fn load_from_path(path: &str) -> Option<image::DynamicImage> {
    image::open(path).ok()
}

fn decode_to_rgb888(img: &image::DynamicImage) -> Vec<Rgb888> {
    let (w, h) = img.dimensions();
    let mut pixels = Vec::with_capacity((w * h) as usize);
    for y in 0..h {
        for x in 0..w {
            let pixel = img.get_pixel(x, y);
            pixels.push(Rgb888::new(pixel[0], pixel[1], pixel[2]));
        }
    }
    pixels
}
