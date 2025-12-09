//! Video test binary - displays moving text using SDL3 at 640x480 32-bit color.

use embedded_graphics::draw_target::DrawTarget;
use embedded_graphics::geometry::Size;
use embedded_graphics::pixelcolor::Rgb888;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::{PrimitiveStyle, Rectangle};
use sdl3::pixels::PixelFormat;
use sdl3::render::{Canvas, Texture, TextureCreator};
use sdl3::sys::pixels::SDL_PIXELFORMAT_RGB24;
use sdl3::video::WindowContext;
use u8g2_fonts::types::{FontColor, VerticalPosition};
use u8g2_fonts::FontRenderer;

const SCREEN_WIDTH: u32 = 640;
const SCREEN_HEIGHT: u32 = 480;

/// Simple framebuffer that implements DrawTarget for embedded-graphics.
struct Framebuffer {
    data: Vec<u8>,
    size: Size,
}

impl Framebuffer {
    fn new(width: u32, height: u32) -> Self {
        let size = Size::new(width, height);
        let data = vec![0u8; (width * height * 3) as usize];
        Self { data, size }
    }

    fn clear(&mut self, color: Rgb888) {
        for chunk in self.data.chunks_exact_mut(3) {
            chunk[0] = color.r();
            chunk[1] = color.g();
            chunk[2] = color.b();
        }
    }

    fn as_slice(&self) -> &[u8] {
        &self.data
    }
}

impl DrawTarget for Framebuffer {
    type Color = Rgb888;
    type Error = core::convert::Infallible;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        let width = self.size.width as i32;
        let height = self.size.height as i32;

        for Pixel(point, color) in pixels {
            if point.x >= 0 && point.x < width && point.y >= 0 && point.y < height {
                let index = ((point.y as u32 * self.size.width + point.x as u32) * 3) as usize;
                self.data[index] = color.r();
                self.data[index + 1] = color.g();
                self.data[index + 2] = color.b();
            }
        }
        Ok(())
    }
}

impl OriginDimensions for Framebuffer {
    fn size(&self) -> Size {
        self.size
    }
}

fn main() {
    unsafe {
        const SDL_VIDEO_DRIVER_VARNAME: &str = "SDL_VIDEO_DRIVER";
        const SDL_VIDEO_DRIVER_DEFAULT: &str = "evdev";
        std::env::set_var(SDL_VIDEO_DRIVER_VARNAME, SDL_VIDEO_DRIVER_DEFAULT);
    }

    // Initialize SDL3
    let sdl_context = sdl3::init().expect("Failed to initialize SDL3");
    let video_subsystem = sdl_context
        .video()
        .expect("Failed to initialize video subsystem");

    // Create window at 640x480
    let window = video_subsystem
        .window("Video Test - Moving Text", SCREEN_WIDTH, SCREEN_HEIGHT)
        .position_centered()
        .fullscreen()
        .build()
        .expect("Failed to create window");

    // Create canvas (renderer)
    let mut canvas: Canvas<sdl3::video::Window> = window.into_canvas();

    // Create texture for streaming pixel data
    let texture_creator: TextureCreator<WindowContext> = canvas.texture_creator();
    let mut texture: Texture = texture_creator
        .create_texture_streaming(
            unsafe { PixelFormat::from_ll(SDL_PIXELFORMAT_RGB24) },
            SCREEN_WIDTH,
            SCREEN_HEIGHT,
        )
        .expect("Failed to create texture");

    // Create our framebuffer
    let mut framebuffer = Framebuffer::new(SCREEN_WIDTH, SCREEN_HEIGHT);

    // Create font renderer using a built-in u8g2 font
    let font_renderer = FontRenderer::new::<u8g2_fonts::fonts::u8g2_font_ncenB24_tr>();
    let small_font_renderer =
        FontRenderer::new::<u8g2_fonts::fonts::u8g2_font_haxrcorp4089_t_cyrillic>();

    // Event pump for input handling
    let mut event_pump = sdl_context.event_pump().expect("Failed to get event pump");

    // Animation state
    let mut x: f32 = 100.0;
    let mut y: f32 = 240.0;
    let mut dx: f32 = 2.0;
    let mut dy: f32 = 1.5;

    let text = "1FPGA";
    let info_text = "640x480 32-bit - Press ESC to quit";

    // Get approximate text dimensions for bouncing
    let text_width: i32 = 120; // Approximate width for "1FPGA" with this font
    let text_height: i32 = 30; // Approximate height

    // Main loop
    'running: loop {
        // Handle events
        event_pump.pump_events();
        for event in event_pump.poll_iter() {
            match event {
                sdl3::event::Event::Quit { .. } => break 'running,
                sdl3::event::Event::KeyDown {
                    scancode: Some(sdl3::keyboard::Scancode::Escape),
                    ..
                } => break 'running,
                _ => {}
            }
        }

        // Update position
        x += dx;
        y += dy;

        // Bounce off walls
        if x <= 0.0 || x + text_width as f32 >= SCREEN_WIDTH as f32 {
            dx = -dx;
            x = x.clamp(0.0, SCREEN_WIDTH as f32 - text_width as f32);
        }
        if y - text_height as f32 <= 0.0 || y >= SCREEN_HEIGHT as f32 {
            dy = -dy;
            y = y.clamp(text_height as f32, SCREEN_HEIGHT as f32);
        }

        // Clear framebuffer to dark blue
        framebuffer.clear(Rgb888::new(0, 0, 40));

        // Draw a border rectangle
        Rectangle::new(
            Point::new(5, 5),
            Size::new(SCREEN_WIDTH - 10, SCREEN_HEIGHT - 10),
        )
        .into_styled(PrimitiveStyle::with_stroke(Rgb888::new(100, 100, 200), 2))
        .draw(&mut framebuffer)
        .unwrap();

        // Draw the moving text
        font_renderer
            .render(
                text,
                Point::new(x as i32, y as i32),
                VerticalPosition::Baseline,
                FontColor::Transparent(Rgb888::new(255, 255, 0)),
                &mut framebuffer,
            )
            .unwrap();

        // Draw info text at bottom
        small_font_renderer
            .render(
                info_text,
                Point::new(10, SCREEN_HEIGHT as i32 - 15),
                VerticalPosition::Baseline,
                FontColor::Transparent(Rgb888::new(150, 150, 150)),
                &mut framebuffer,
            )
            .unwrap();

        // Update texture with framebuffer data
        texture
            .update(None, framebuffer.as_slice(), (SCREEN_WIDTH * 3) as usize)
            .expect("Failed to update texture");

        // Copy texture to canvas and present
        canvas
            .copy(&texture, None, None)
            .expect("Failed to copy texture");
        canvas.present();

        // Cap frame rate (~60 FPS)
        std::thread::sleep(std::time::Duration::from_millis(16));
    }
}
