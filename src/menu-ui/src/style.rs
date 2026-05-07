//! Style struct (fixed property list) and parsers.
//!
//! N3 grows the surface to the full property list agreed in the plan:
//! display, flex-*, position, top/right/bottom/left, padding/margin,
//! min/max-width/height, gap, plus the visual fields
//! (`background-color`, `color`, `opacity`, `overflow`).
//!
//! The CSS `class` mechanism (registered class table + `class` prop)
//! is wired in N3 too; for now we only consume `style` props.

use menu_core_host::protocol::Rgba;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Display {
    #[default]
    Block,
    Flex,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Position {
    #[default]
    Relative,
    Absolute,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum FlexDirection {
    #[default]
    Row,
    Column,
    RowReverse,
    ColumnReverse,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum FlexWrap {
    #[default]
    NoWrap,
    Wrap,
    WrapReverse,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum JustifyContent {
    #[default]
    FlexStart,
    FlexEnd,
    Center,
    SpaceBetween,
    SpaceAround,
    SpaceEvenly,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum AlignItems {
    #[default]
    Stretch,
    FlexStart,
    FlexEnd,
    Center,
    Baseline,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Overflow {
    #[default]
    Visible,
    Hidden,
}

/// All style properties accepted by `1fpga:gui`. Every field is
/// `Option` so we can detect "set" vs "inherited / default" cleanly
/// when copying into Taffy.
///
/// Not `Copy` because `font_family` is a `String`. Cloning is cheap
/// on the hot paths (inline-string copy is small).
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Style {
    // ---- Layout ------------------------------------------------------
    pub display: Option<Display>,
    pub position: Option<Position>,
    pub flex_direction: Option<FlexDirection>,
    pub flex_wrap: Option<FlexWrap>,
    pub justify_content: Option<JustifyContent>,
    pub align_items: Option<AlignItems>,
    pub align_self: Option<AlignItems>,
    pub flex_grow: Option<f32>,
    pub flex_shrink: Option<f32>,
    pub flex_basis: Option<f32>,
    pub gap: Option<f32>,

    // ---- Position offsets (used when position: absolute, or as
    // ----- override hints in N1/N2 compatibility paths) ---------------
    pub top: Option<f32>,
    pub right: Option<f32>,
    pub bottom: Option<f32>,
    pub left: Option<f32>,

    // ---- Box ---------------------------------------------------------
    pub width: Option<f32>,
    pub height: Option<f32>,
    pub min_width: Option<f32>,
    pub max_width: Option<f32>,
    pub min_height: Option<f32>,
    pub max_height: Option<f32>,
    pub padding_top: Option<f32>,
    pub padding_right: Option<f32>,
    pub padding_bottom: Option<f32>,
    pub padding_left: Option<f32>,
    pub margin_top: Option<f32>,
    pub margin_right: Option<f32>,
    pub margin_bottom: Option<f32>,
    pub margin_left: Option<f32>,

    // ---- Visual ------------------------------------------------------
    pub background_color: Option<Rgba>,
    pub opacity: Option<f32>,
    pub overflow: Option<Overflow>,

    // ---- Text --------------------------------------------------------
    pub color: Option<Rgba>,
    /// Font family — looked up in `FontRegistry`. `None` means use
    /// the registry's default font.
    pub font_family: Option<FontFamily>,
    pub font_size: Option<f32>,
}

impl Style {
    /// Overlay every `Some` field of `patch` onto `self`, leaving
    /// `None` fields of `patch` untouched. Used by the
    /// `gui.updateStyle` fast-path so animated tweens (which only
    /// deliver the keys they animate) don't clobber the rest of a
    /// node's style.
    pub fn merge_from(&mut self, patch: &Style) {
        if patch.display.is_some()         { self.display = patch.display; }
        if patch.position.is_some()        { self.position = patch.position; }
        if patch.flex_direction.is_some()  { self.flex_direction = patch.flex_direction; }
        if patch.flex_wrap.is_some()       { self.flex_wrap = patch.flex_wrap; }
        if patch.justify_content.is_some() { self.justify_content = patch.justify_content; }
        if patch.align_items.is_some()     { self.align_items = patch.align_items; }
        if patch.align_self.is_some()      { self.align_self = patch.align_self; }
        if patch.flex_grow.is_some()       { self.flex_grow = patch.flex_grow; }
        if patch.flex_shrink.is_some()     { self.flex_shrink = patch.flex_shrink; }
        if patch.flex_basis.is_some()      { self.flex_basis = patch.flex_basis; }
        if patch.gap.is_some()             { self.gap = patch.gap; }
        if patch.top.is_some()             { self.top = patch.top; }
        if patch.right.is_some()           { self.right = patch.right; }
        if patch.bottom.is_some()          { self.bottom = patch.bottom; }
        if patch.left.is_some()            { self.left = patch.left; }
        if patch.width.is_some()           { self.width = patch.width; }
        if patch.height.is_some()          { self.height = patch.height; }
        if patch.min_width.is_some()       { self.min_width = patch.min_width; }
        if patch.max_width.is_some()       { self.max_width = patch.max_width; }
        if patch.min_height.is_some()      { self.min_height = patch.min_height; }
        if patch.max_height.is_some()      { self.max_height = patch.max_height; }
        if patch.padding_top.is_some()     { self.padding_top = patch.padding_top; }
        if patch.padding_right.is_some()   { self.padding_right = patch.padding_right; }
        if patch.padding_bottom.is_some()  { self.padding_bottom = patch.padding_bottom; }
        if patch.padding_left.is_some()    { self.padding_left = patch.padding_left; }
        if patch.margin_top.is_some()      { self.margin_top = patch.margin_top; }
        if patch.margin_right.is_some()    { self.margin_right = patch.margin_right; }
        if patch.margin_bottom.is_some()   { self.margin_bottom = patch.margin_bottom; }
        if patch.margin_left.is_some()     { self.margin_left = patch.margin_left; }
        if patch.background_color.is_some(){ self.background_color = patch.background_color; }
        if patch.opacity.is_some()         { self.opacity = patch.opacity; }
        if patch.overflow.is_some()        { self.overflow = patch.overflow; }
        if patch.color.is_some()           { self.color = patch.color; }
        if patch.font_family.is_some()     { self.font_family = patch.font_family.clone(); }
        if patch.font_size.is_some()       { self.font_size = patch.font_size; }
    }
}

/// Identifier for a registered font — kept as a small `String` since
/// it's compared by name on lookup. `Box<str>` would save a few bytes
/// but `String` makes parser ergonomics simpler.
pub type FontFamily = String;

/// Parse a CSS-style color string. Supports `#rgb`, `#rrggbb`,
/// `#rrggbbaa`. Returns `None` if `s` is not recognised.
pub fn parse_color(s: &str) -> Option<Rgba> {
    let s = s.strip_prefix('#')?;
    let bytes = s.as_bytes();
    let to_hex = |c: u8| -> Option<u8> {
        match c {
            b'0'..=b'9' => Some(c - b'0'),
            b'a'..=b'f' => Some(c - b'a' + 10),
            b'A'..=b'F' => Some(c - b'A' + 10),
            _ => None,
        }
    };
    match bytes.len() {
        3 => {
            let r = to_hex(bytes[0])?;
            let g = to_hex(bytes[1])?;
            let b = to_hex(bytes[2])?;
            Some(Rgba::new(r << 4 | r, g << 4 | g, b << 4 | b, 0xFF))
        }
        6 => {
            let r = (to_hex(bytes[0])? << 4) | to_hex(bytes[1])?;
            let g = (to_hex(bytes[2])? << 4) | to_hex(bytes[3])?;
            let b = (to_hex(bytes[4])? << 4) | to_hex(bytes[5])?;
            Some(Rgba::new(r, g, b, 0xFF))
        }
        8 => {
            let r = (to_hex(bytes[0])? << 4) | to_hex(bytes[1])?;
            let g = (to_hex(bytes[2])? << 4) | to_hex(bytes[3])?;
            let b = (to_hex(bytes[4])? << 4) | to_hex(bytes[5])?;
            let a = (to_hex(bytes[6])? << 4) | to_hex(bytes[7])?;
            Some(Rgba::new(r, g, b, a))
        }
        _ => None,
    }
}

/// Parse a `display` keyword.
pub fn parse_display(s: &str) -> Option<Display> {
    match s {
        "block" => Some(Display::Block),
        "flex" => Some(Display::Flex),
        _ => None,
    }
}

/// Parse a `position` keyword.
pub fn parse_position(s: &str) -> Option<Position> {
    match s {
        "relative" => Some(Position::Relative),
        "absolute" => Some(Position::Absolute),
        _ => None,
    }
}

pub fn parse_flex_direction(s: &str) -> Option<FlexDirection> {
    match s {
        "row" => Some(FlexDirection::Row),
        "column" => Some(FlexDirection::Column),
        "row-reverse" => Some(FlexDirection::RowReverse),
        "column-reverse" => Some(FlexDirection::ColumnReverse),
        _ => None,
    }
}

pub fn parse_flex_wrap(s: &str) -> Option<FlexWrap> {
    match s {
        "nowrap" => Some(FlexWrap::NoWrap),
        "wrap" => Some(FlexWrap::Wrap),
        "wrap-reverse" => Some(FlexWrap::WrapReverse),
        _ => None,
    }
}

pub fn parse_justify_content(s: &str) -> Option<JustifyContent> {
    match s {
        "flex-start" | "start" => Some(JustifyContent::FlexStart),
        "flex-end" | "end" => Some(JustifyContent::FlexEnd),
        "center" => Some(JustifyContent::Center),
        "space-between" => Some(JustifyContent::SpaceBetween),
        "space-around" => Some(JustifyContent::SpaceAround),
        "space-evenly" => Some(JustifyContent::SpaceEvenly),
        _ => None,
    }
}

pub fn parse_align_items(s: &str) -> Option<AlignItems> {
    match s {
        "stretch" => Some(AlignItems::Stretch),
        "flex-start" | "start" => Some(AlignItems::FlexStart),
        "flex-end" | "end" => Some(AlignItems::FlexEnd),
        "center" => Some(AlignItems::Center),
        "baseline" => Some(AlignItems::Baseline),
        _ => None,
    }
}

pub fn parse_overflow(s: &str) -> Option<Overflow> {
    match s {
        "visible" => Some(Overflow::Visible),
        "hidden" => Some(Overflow::Hidden),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_three_digit_hex() {
        assert_eq!(parse_color("#f00"), Some(Rgba::new(0xFF, 0x00, 0x00, 0xFF)));
        assert_eq!(parse_color("#0AF"), Some(Rgba::new(0x00, 0xAA, 0xFF, 0xFF)));
    }

    #[test]
    fn parse_six_digit_hex() {
        assert_eq!(parse_color("#123456"), Some(Rgba::new(0x12, 0x34, 0x56, 0xFF)));
    }

    #[test]
    fn parse_eight_digit_hex() {
        assert_eq!(
            parse_color("#12345680"),
            Some(Rgba::new(0x12, 0x34, 0x56, 0x80))
        );
    }

    #[test]
    fn parse_rejects_non_hex() {
        assert_eq!(parse_color("red"), None);
        assert_eq!(parse_color("#xyz"), None);
        assert_eq!(parse_color(""), None);
        assert_eq!(parse_color("#1234"), None);
    }

    #[test]
    fn parse_keyword_helpers() {
        assert_eq!(parse_display("flex"), Some(Display::Flex));
        assert_eq!(parse_position("absolute"), Some(Position::Absolute));
        assert_eq!(parse_flex_direction("column"), Some(FlexDirection::Column));
        assert_eq!(parse_justify_content("center"), Some(JustifyContent::Center));
        assert_eq!(parse_align_items("stretch"), Some(AlignItems::Stretch));
        assert_eq!(parse_overflow("hidden"), Some(Overflow::Hidden));
        assert_eq!(parse_display("oogabooga"), None);
    }
}
