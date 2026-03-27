use embedded_graphics::pixelcolor::Rgb888;
use embedded_graphics::prelude::*;
use embedded_graphics_framebuf::backends::FrameBufferBackend;
use linuxfb::TerminalMode;

/// Software double-buffered framebuffer.
/// Draws to a RAM back buffer, then copies to the hardware framebuffer on flip.
pub struct DoubleBufferedFramebuffer {
    _fb: linuxfb::Framebuffer,
    _mmap: memmap::MmapMut,
    front: &'static mut [Rgb888],
    back: Vec<Rgb888>,
    width: u32,
    height: u32,
}

impl DoubleBufferedFramebuffer {
    pub fn new(path: impl AsRef<std::path::Path>) -> Result<Self, String> {
        let tty = std::fs::File::open("/dev/tty1")
            .map_err(|e| format!("Failed to open /dev/tty1: {e}"))?;
        linuxfb::set_terminal_mode(&tty, TerminalMode::Graphics)
            .map_err(|e| format!("{e:?}"))?;
        drop(tty);

        let fb = linuxfb::Framebuffer::new(path).map_err(|err| format!("{err:?}"))?;
        let (width, height) = fb.get_size();
        let pixel_count = (width * height) as usize;

        let mut mmap = fb.map().map_err(|e| format!("{e:?}"))?;

        let front = unsafe {
            let (prefix, pixels, suffix) = mmap.align_to_mut::<Rgb888>();
            assert_eq!(prefix.len(), 0, "Framebuffer not aligned properly");
            assert_eq!(suffix.len(), 0, "Framebuffer has trailing bytes");
            std::slice::from_raw_parts_mut(pixels.as_mut_ptr(), pixel_count)
        };

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

    pub fn size(&self) -> Size {
        Size::new(self.width, self.height)
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn clear(&mut self, color: Rgb888) {
        self.back.fill(color);
    }

    /// Copy back buffer to front buffer, making the frame visible.
    pub fn flip(&mut self) {
        self.front.copy_from_slice(&self.back);
    }

    /// Direct access to the back buffer for pixel-level operations.
    pub fn back_buffer(&self) -> &[Rgb888] {
        &self.back
    }

    /// Direct mutable access to the back buffer.
    pub fn back_buffer_mut(&mut self) -> &mut [Rgb888] {
        &mut self.back
    }

    /// Set a single pixel in the back buffer.
    #[inline]
    pub fn set_pixel(&mut self, x: u32, y: u32, color: Rgb888) {
        if x < self.width && y < self.height {
            self.back[(y * self.width + x) as usize] = color;
        }
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
