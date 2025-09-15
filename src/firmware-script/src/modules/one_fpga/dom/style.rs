use crate::modules::one_fpga::dom::render::FontProp;
use boa_engine::value::{TryFromJs, TryIntoJs};

#[derive(Clone, Debug, TryFromJs, TryIntoJs)]
pub(crate) struct Location {
    x: i32,
    y: i32,
}

#[derive(Clone, Debug, TryFromJs, TryIntoJs)]
pub(crate) struct NodeStyle {
    font: Option<FontProp>,
    location: Option<Location>,
}
