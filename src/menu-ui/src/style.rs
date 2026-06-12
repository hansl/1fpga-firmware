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

    // ---- Transform (axis-independent scale around layout-rect center) -
    // These are inherited multiplicatively by descendants — a parent
    // with scale_x=1.2 effectively scales every child by 1.2 too,
    // exactly like CSS's stacking-context behaviour. The layout box
    // itself is *not* affected; Taffy still places neighbours as
    // though the element were at unit scale. Default for both axes
    // is 1.0 (no scaling).
    pub scale_x: Option<f32>,
    pub scale_y: Option<f32>,

    // Rotation in degrees (clockwise), about the element's centre.
    // Applied only to <img> nodes today (rendered via the affine blit);
    // ignored for div/text, which paint axis-aligned. Inherited
    // additively by descendants, like scale is multiplicatively.
    pub rotate: Option<f32>,

    // Translation in pixels, applied to the paint rect *after* scaling.
    // Inherited additively by descendants — a parent with translate_x=40
    // shifts every child by 40 too — exactly like CSS `transform:
    // translate`. The layout box is NOT affected: Taffy still places
    // neighbours as though the element were at the origin, so animating
    // translate is paint-only (no reflow). Default 0 on both axes.
    pub translate_x: Option<f32>,
    pub translate_y: Option<f32>,

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
        if patch.scale_x.is_some()         { self.scale_x = patch.scale_x; }
        if patch.scale_y.is_some()         { self.scale_y = patch.scale_y; }
        if patch.rotate.is_some()          { self.rotate = patch.rotate; }
        if patch.translate_x.is_some()     { self.translate_x = patch.translate_x; }
        if patch.translate_y.is_some()     { self.translate_y = patch.translate_y; }
        if patch.color.is_some()           { self.color = patch.color; }
        if patch.font_family.is_some()     { self.font_family = patch.font_family.clone(); }
        if patch.font_size.is_some()       { self.font_size = patch.font_size; }
    }
}

/// Identifier for a registered font — kept as a small `String` since
/// it's compared by name on lookup. `Box<str>` would save a few bytes
/// but `String` makes parser ergonomics simpler.
pub type FontFamily = String;

/// Walk the tree from `root`, computing each node's *effective*
/// opacity = ancestor_effective × self_opacity (CSS-like multiplicative
/// inheritance). Single pass over the tree, used once per frame by
/// both paint and damage so each node sees the same value.
///
/// Returns `1.0` for any node whose own `style.opacity` is `None`.
/// True CSS opacity is a stacking-context primitive (subtree
/// composited at full alpha then painted onto canvas at opacity);
/// we approximate it by multiplying down. For non-overlapping
/// children — the menu UI's common case — the two are equivalent.
pub fn resolve_opacity(
    tree: &crate::vdom::Tree,
    root: crate::vdom::NodeId,
) -> std::collections::HashMap<crate::vdom::NodeId, f32> {
    let mut out = std::collections::HashMap::new();
    walk_opacity(tree, root, 1.0, &mut out);
    out
}

fn walk_opacity(
    tree: &crate::vdom::Tree,
    id: crate::vdom::NodeId,
    parent: f32,
    out: &mut std::collections::HashMap<crate::vdom::NodeId, f32>,
) {
    let Some(node) = tree.get(id) else {
        return;
    };
    let here = parent * node.style.opacity.unwrap_or(1.0);
    out.insert(id, here.clamp(0.0, 1.0));
    for &child in &node.children {
        walk_opacity(tree, child, here, out);
    }
}

/// Effective 2D transform for a node, accumulated from ancestors.
/// `scale_*` is the multiplicative scale (1.0 = identity). The
/// effective transform is the same shape CSS's stacking-context
/// model produces: each node's own scale multiplied by every
/// ancestor's. For non-overlapping subtrees — the menu-UI default —
/// this matches CSS exactly; overlapping subtrees with partial
/// opacity & scale combinations may differ.
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct Transform {
    pub scale_x: f32,
    pub scale_y: f32,
    /// Accumulated rotation in degrees (clockwise), about the rect
    /// centre. Only <img> nodes act on it (via the affine blit).
    pub rotation: f32,
    /// Accumulated translation in pixels, added to the dst rect *after*
    /// the scale-about-centre step. Additive down the subtree, with no
    /// scale interaction (the menu's translated nodes have no scaled
    /// ancestors, so a plain sum matches CSS for that case). Paint-only.
    pub tx: f32,
    pub ty: f32,
}

impl Transform {
    pub const IDENTITY: Self =
        Self { scale_x: 1.0, scale_y: 1.0, rotation: 0.0, tx: 0.0, ty: 0.0 };

    #[inline]
    pub fn is_identity(self) -> bool {
        // Use a small tolerance — scale values come from f32 tweens
        // and may end at 1.0 + epsilon after interpolation. Rotation is
        // intentionally excluded: it doesn't change the axis-aligned
        // scale box (it's handled separately by the img paint path), so
        // a pure rotation must not flip div/text onto the scaled path.
        (self.scale_x - 1.0).abs() < 1e-4 && (self.scale_y - 1.0).abs() < 1e-4
    }

    /// True when this transform has a non-trivial rotation.
    #[inline]
    pub fn has_rotation(self) -> bool {
        self.rotation.abs() > 1e-3
    }

    /// Apply this transform to a layout rect, scaling around its
    /// centre (transform-origin: 50% 50% in CSS terms). Returns the
    /// new dst rect in floating-point form so callers can perform
    /// their own integer-rounding / alignment. Negative scales are
    /// clamped to zero (we don't support flips).
    #[inline]
    pub fn apply_to_rect(self, x: f32, y: f32, w: f32, h: f32) -> (f32, f32, f32, f32) {
        let sx = self.scale_x.max(0.0);
        let sy = self.scale_y.max(0.0);
        let cx = x + w * 0.5;
        let cy = y + h * 0.5;
        let new_w = w * sx;
        let new_h = h * sy;
        // Scale about the centre, then apply the accumulated translate.
        let new_x = cx - new_w * 0.5 + self.tx;
        let new_y = cy - new_h * 0.5 + self.ty;
        (new_x, new_y, new_w, new_h)
    }

    /// Like [`apply_to_rect`](Self::apply_to_rect) but returns the
    /// axis-aligned bounding box of the scaled-then-rotated rect (about
    /// its centre). Used by damage tracking and the paint cull so a
    /// rotated image's full footprint — which extends beyond the scale
    /// box (up to ~1.41× at 45°) — is covered. Collapses to
    /// `apply_to_rect` when there is no rotation.
    #[inline]
    pub fn apply_to_aabb(self, x: f32, y: f32, w: f32, h: f32) -> (f32, f32, f32, f32) {
        let (sx, sy, sw, sh) = self.apply_to_rect(x, y, w, h);
        if !self.has_rotation() {
            return (sx, sy, sw, sh);
        }
        let rad = self.rotation.to_radians();
        let c = rad.cos().abs();
        let s = rad.sin().abs();
        let aw = sw * c + sh * s;
        let ah = sw * s + sh * c;
        let cx = sx + sw * 0.5;
        let cy = sy + sh * 0.5;
        (cx - aw * 0.5, cy - ah * 0.5, aw, ah)
    }
}

/// Walk the tree from `root`, computing each node's effective
/// transform = ancestor × self (multiplicative). Same shape as
/// [`resolve_opacity`]; used by paint to size draw rects and by
/// damage to include the transform in each item's content hash.
pub fn resolve_transforms(
    tree: &crate::vdom::Tree,
    root: crate::vdom::NodeId,
) -> std::collections::HashMap<crate::vdom::NodeId, Transform> {
    let mut out = std::collections::HashMap::new();
    walk_transform(tree, root, Transform::IDENTITY, &mut out);
    out
}

fn walk_transform(
    tree: &crate::vdom::Tree,
    id: crate::vdom::NodeId,
    parent: Transform,
    out: &mut std::collections::HashMap<crate::vdom::NodeId, Transform>,
) {
    let Some(node) = tree.get(id) else {
        return;
    };
    let sx = node.style.scale_x.unwrap_or(1.0);
    let sy = node.style.scale_y.unwrap_or(1.0);
    let here = Transform {
        scale_x: parent.scale_x * sx,
        scale_y: parent.scale_y * sy,
        rotation: parent.rotation + node.style.rotate.unwrap_or(0.0),
        tx: parent.tx + node.style.translate_x.unwrap_or(0.0),
        ty: parent.ty + node.style.translate_y.unwrap_or(0.0),
    };
    out.insert(id, here);
    for &child in &node.children {
        walk_transform(tree, child, here, out);
    }
}

/// Parse a CSS-style color string. Supports `#rgb`, `#rrggbb`,
/// `#rrggbbaa`, plus the `rgb(R, G, B)` / `rgba(R, G, B, A)` forms
/// (the latter is what react-spring's string interpolator emits when
/// tweening between hex/named colours). Returns `None` if `s` is not
/// recognised.
pub fn parse_color(s: &str) -> Option<Rgba> {
    let s = s.trim();
    if let Some(hex) = s.strip_prefix('#') {
        return parse_hex_color(hex);
    }
    if let Some(args) = s.strip_prefix("rgba(").and_then(|x| x.strip_suffix(')')) {
        return parse_rgb_args(args, /* with_alpha */ true);
    }
    if let Some(args) = s.strip_prefix("rgb(").and_then(|x| x.strip_suffix(')')) {
        return parse_rgb_args(args, /* with_alpha */ false);
    }
    None
}

fn parse_hex_color(hex: &str) -> Option<Rgba> {
    let bytes = hex.as_bytes();
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

/// Parse a comma-separated `R, G, B[, A]` argument list. RGB values
/// are 0-255 integers (CSS allows percentages too; we don't yet).
/// Alpha is 0..1 floating point per CSS — we round*255 to map to our
/// 8-bit channel.
fn parse_rgb_args(args: &str, with_alpha: bool) -> Option<Rgba> {
    let mut parts = args.split(',').map(|p| p.trim());
    let r: u32 = parts.next()?.parse().ok()?;
    let g: u32 = parts.next()?.parse().ok()?;
    let b: u32 = parts.next()?.parse().ok()?;
    let a: u8 = if with_alpha {
        let a_str = parts.next()?;
        let af: f32 = a_str.parse().ok()?;
        (af.clamp(0.0, 1.0) * 255.0).round() as u8
    } else {
        0xFF
    };
    if parts.next().is_some() {
        return None; // extra args
    }
    if r > 255 || g > 255 || b > 255 {
        return None;
    }
    Some(Rgba::new(r as u8, g as u8, b as u8, a))
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
    fn parse_rgb_function() {
        assert_eq!(
            parse_color("rgb(18, 52, 86)"),
            Some(Rgba::new(0x12, 0x34, 0x56, 0xFF))
        );
        // Whitespace tolerance — spring's interpolator emits with one
        // space after each comma, but we also strip leading/trailing.
        assert_eq!(
            parse_color("  rgb(255,0,0)  "),
            Some(Rgba::new(0xFF, 0, 0, 0xFF))
        );
    }

    #[test]
    fn parse_rgba_function() {
        // react-spring's stringInterpolation outputs this exact shape:
        // "rgba(R, G, B, A)" with float alpha.
        assert_eq!(
            parse_color("rgba(255, 80, 96, 1)"),
            Some(Rgba::new(0xFF, 0x50, 0x60, 0xFF))
        );
        assert_eq!(
            parse_color("rgba(0, 0, 0, 0.5)"),
            Some(Rgba::new(0, 0, 0, 0x80)) // 0.5 * 255 ≈ 128
        );
        assert_eq!(
            parse_color("rgba(0, 0, 0, 0)"),
            Some(Rgba::new(0, 0, 0, 0))
        );
    }

    #[test]
    fn parse_rgb_rejects_bad_input() {
        assert_eq!(parse_color("rgb(256, 0, 0)"), None); // out of range
        assert_eq!(parse_color("rgb(0, 0)"), None); // too few args
        assert_eq!(parse_color("rgb(0, 0, 0, 0)"), None); // too many for rgb
        assert_eq!(parse_color("rgba(0, 0, 0)"), None); // too few for rgba
        assert_eq!(parse_color("rgb(a, b, c)"), None); // not numbers
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
