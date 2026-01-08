use crate::macguiver::buffer::DrawBuffer;
use embedded_graphics::geometry::OriginDimensions;
use embedded_graphics::pixelcolor::BinaryColor;
use mister_fpga::fpga::MisterFpga;
use mister_fpga::osd::OsdDisplay;

pub struct Osd {
    main: OsdDisplay,
    main_buffer: DrawBuffer<BinaryColor>,
    title: OsdDisplay,
    title_buffer: DrawBuffer<BinaryColor>,
}

impl Osd {
    pub fn new() -> Self {
        let main = OsdDisplay::main();
        let main_buffer = DrawBuffer::new(main.size());

        let title = OsdDisplay::title();
        let title_buffer = DrawBuffer::new(title.size());

        Self {
            main,
            main_buffer,
            title,
            title_buffer,
        }
    }

    pub fn main_mut(&mut self) -> &mut DrawBuffer<BinaryColor> {
        &mut self.main_buffer
    }

    pub fn title(&mut self) -> &mut DrawBuffer<BinaryColor> {
        &mut self.title_buffer
    }

    pub fn update(&self, fpga: &mut MisterFpga) {
        self.main.send(fpga, &self.main_buffer);
        self.title.send(fpga, &self.title_buffer);
    }
}
