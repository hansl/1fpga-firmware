use crate::modules::one_fpga::dom::render::FontProp;
use boa_engine::value::{TryFromJs, TryIntoJs};
use embedded_graphics::geometry::Point;

#[derive(Copy, Clone, Debug, TryFromJs, TryIntoJs)]
pub(crate) struct Location {
    pub x: i32,
    pub y: i32,
}

impl Into<Point> for Location {
    fn into(self) -> Point {
        Point::new(self.x, self.y)
    }
}

#[derive(Default, Copy, Clone, Debug, TryFromJs)]
pub(crate) struct BoxStyle {
    pub font: Option<FontProp>,
    pub location: Option<Location>,
}

#[derive(Default, Clone, Debug, TryFromJs)]
pub(crate) struct TStyle {
    pub font: Option<FontProp>,
}
