//! Video test binary - displays moving text using direct /dev/fb0 framebuffer
//! with software double buffering for tear-free rendering.

use embedded_graphics::pixelcolor::Rgb888;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::{PrimitiveStyle, Rectangle};
use embedded_graphics_framebuf::backends::FrameBufferBackend;
use embedded_graphics_framebuf::FrameBuf;
use linuxfb::TerminalMode;
use std::time::{Duration, Instant};
use u8g2_fonts::types::{FontColor, VerticalPosition};
use u8g2_fonts::FontRenderer;

/// Software double-buffered framebuffer.
/// Draws to a RAM back buffer, then copies to the hardware framebuffer on flip.
struct DoubleBufferedFramebuffer {
    _fb: linuxfb::Framebuffer,
    _mmap: memmap::MmapMut,
    /// Hardware framebuffer slice (front buffer)
    front: &'static mut [Rgb888],
    /// Software back buffer in RAM
    back: Vec<Rgb888>,
    width: u32,
    height: u32,
}

impl DoubleBufferedFramebuffer {
    fn new(path: impl AsRef<std::path::Path>) -> Result<Self, String> {
        // Set TTY to graphics mode to hide cursor and text console
        let tty = std::fs::File::open("/dev/tty1")
            .map_err(|e| format!("Failed to open /dev/tty1: {e}"))?;
        linuxfb::set_terminal_mode(&tty, TerminalMode::Graphics).map_err(|e| format!("{e:?}"))?;
        drop(tty);

        let fb = linuxfb::Framebuffer::new(path).map_err(|err| format!("{err:?}"))?;
        let (width, height) = fb.get_size();
        let bytes_per_pixel = fb.get_bytes_per_pixel();
        let pixel_count = (width * height) as usize;

        eprintln!(
            "Framebuffer: {}x{} @ {} bpp (software double buffered)",
            width,
            height,
            bytes_per_pixel * 8
        );

        let mut mmap = fb.map().map_err(|e| format!("{e:?}"))?;

        // Get front buffer slice from mmap
        let front = unsafe {
            let (prefix, pixels, suffix) = mmap.align_to_mut::<Rgb888>();
            assert_eq!(prefix.len(), 0, "Framebuffer not aligned properly");
            assert_eq!(suffix.len(), 0, "Framebuffer has trailing bytes");
            std::slice::from_raw_parts_mut(pixels.as_mut_ptr(), pixel_count)
        };

        // Allocate back buffer in RAM
        let back = vec![Rgb888::BLACK; pixel_count];

        Ok(Self {
            _fb: fb,
            _mmap: mmap,
            front,
            back,
            width,
            height,
        })
    }

    fn size(&self) -> Size {
        Size::new(self.width, self.height)
    }

    fn clear(&mut self, color: Rgb888) {
        self.back.fill(color);
    }

    /// Copy back buffer to front buffer (hardware framebuffer).
    /// This makes the frame visible.
    fn flip(&mut self) {
        self.front.copy_from_slice(&self.back);
    }
}

impl FrameBufferBackend for &'_ mut DoubleBufferedFramebuffer {
    type Color = Rgb888;

    fn set(&mut self, index: usize, color: Self::Color) {
        self.back[index] = color;
    }

    fn get(&self, index: usize) -> Self::Color {
        self.back[index]
    }

    fn nr_elements(&self) -> usize {
        self.back.len()
    }
}

fn main() {
    let mut fb = DoubleBufferedFramebuffer::new("/dev/fb0").expect("Failed to open framebuffer");
    let size = fb.size();

    eprintln!("Running video test at {}x{}", size.width, size.height);

    // Create font renderers
    let font_renderer = FontRenderer::new::<u8g2_fonts::fonts::u8g2_font_ncenB24_tr>();
    let small_font_renderer =
        FontRenderer::new::<u8g2_fonts::fonts::u8g2_font_haxrcorp4089_t_cyrillic>();

    // Animation state
    let mut x: f32 = 100.0;
    let mut y: f32 = 240.0;
    let mut dx: f32 = 2.0;
    let mut dy: f32 = 1.5;

    let text = "1FPGA";
    let info_text = format!("{}x{} 32-bit - Double Buffered", size.width, size.height);

    // Get approximate text dimensions for bouncing
    let text_width: i32 = 120;
    let text_height: i32 = 30;

    let target_frame_time = Duration::from_millis(16); // ~60 FPS

    // FPS tracking
    let mut frame_count: u32 = 0;
    let mut fps_timer = Instant::now();
    let mut fps_text = String::from("FPS: --");

    // Run for a limited time (20 seconds) since we don't have input handling
    let start_time = Instant::now();
    let run_duration = Duration::from_secs(20);

    while start_time.elapsed() < run_duration {
        let frame_start = Instant::now();

        // Update FPS counter
        frame_count += 1;
        let fps_elapsed = fps_timer.elapsed();
        if fps_elapsed >= Duration::from_secs(1) {
            let fps = frame_count as f32 / fps_elapsed.as_secs_f32();
            fps_text = format!("FPS: {:.1}", fps);
            frame_count = 0;
            fps_timer = Instant::now();
        }

        // Update position
        x += dx;
        y += dy;

        // Bounce off walls
        if x <= 0.0 || x + text_width as f32 >= size.width as f32 {
            dx = -dx;
            x = x.clamp(0.0, size.width as f32 - text_width as f32);
        }
        if y - text_height as f32 <= 0.0 || y >= size.height as f32 {
            dy = -dy;
            y = y.clamp(text_height as f32, size.height as f32);
        }

        // Clear back buffer to dark blue
        fb.clear(Rgb888::new(0, 0, 40));

        // Create a FrameBuf wrapper for drawing to back buffer
        let mut frame_buf = FrameBuf::new(&mut fb, size.width as usize, size.height as usize);

        // Draw a border rectangle
        Rectangle::new(
            Point::new(5, 5),
            Size::new(size.width - 10, size.height - 10),
        )
        .into_styled(PrimitiveStyle::with_stroke(Rgb888::new(100, 100, 200), 2))
        .draw(&mut frame_buf)
        .unwrap();

        // Draw the moving text
        font_renderer
            .render(
                text,
                Point::new(x as i32, y as i32),
                VerticalPosition::Baseline,
                FontColor::Transparent(Rgb888::new(255, 255, 0)),
                &mut frame_buf,
            )
            .unwrap();

        // Draw info text at bottom
        small_font_renderer
            .render(
                info_text.as_str(),
                Point::new(10, size.height as i32 - 15),
                VerticalPosition::Baseline,
                FontColor::Transparent(Rgb888::new(150, 150, 150)),
                &mut frame_buf,
            )
            .unwrap();

        // Draw FPS counter in top-right corner
        small_font_renderer
            .render(
                fps_text.as_str(),
                Point::new(size.width as i32 - 80, 20),
                VerticalPosition::Baseline,
                FontColor::Transparent(Rgb888::new(0, 255, 0)),
                &mut frame_buf,
            )
            .unwrap();

        // Copy back buffer to front buffer (display)
        fb.flip();

        // Cap frame rate
        let elapsed = frame_start.elapsed();
        if elapsed < target_frame_time {
            std::thread::sleep(target_frame_time - elapsed);
        }
    }

    // Clear to black before exiting
    fb.clear(Rgb888::BLACK);
    fb.flip();
    eprintln!("Video test complete.");
}
