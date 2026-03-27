pub use embedded_graphics::pixelcolor::Rgb888;

/// Unique identifier for a node in the DOM tree.
pub type NodeId = u32;

/// The type of a DOM node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeType {
    /// The root node of the tree. There is exactly one per DomTree.
    Root,
    /// A generic container element (like HTML `<div>`).
    View,
    /// A text leaf node.
    Text,
    /// An image leaf node.
    Image,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FlexDirection {
    Row,
    #[default]
    Column,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum JustifyContent {
    #[default]
    FlexStart,
    FlexEnd,
    Center,
    SpaceBetween,
    SpaceAround,
    SpaceEvenly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AlignItems {
    FlexStart,
    FlexEnd,
    Center,
    #[default]
    Stretch,
    Baseline,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PositionType {
    #[default]
    Relative,
    Absolute,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ObjectFit {
    #[default]
    Cover,
    Contain,
    Fill,
}

/// Style properties for a DOM node. Controls both layout (via taffy) and visual rendering.
#[derive(Debug, Clone, Default)]
pub struct StyleProps {
    // Layout properties (fed to taffy)
    pub width: Option<f32>,
    pub height: Option<f32>,
    pub min_width: Option<f32>,
    pub min_height: Option<f32>,
    pub max_width: Option<f32>,
    pub max_height: Option<f32>,
    pub flex_direction: Option<FlexDirection>,
    pub justify_content: Option<JustifyContent>,
    pub align_items: Option<AlignItems>,
    pub padding: [f32; 4],  // top, right, bottom, left
    pub margin: [f32; 4],   // top, right, bottom, left
    pub gap: Option<f32>,
    pub flex_grow: Option<f32>,
    pub flex_shrink: Option<f32>,
    pub flex_basis: Option<f32>,
    pub position_type: Option<PositionType>,
    pub top: Option<f32>,
    pub left: Option<f32>,
    pub right: Option<f32>,
    pub bottom: Option<f32>,

    // Visual properties
    pub background_color: Option<Rgb888>,
    pub color: Option<Rgb888>,
    pub font_size: Option<u8>,
    pub border_radius: Option<f32>,
    pub opacity: Option<f32>,

    // Image properties
    pub object_fit: Option<ObjectFit>,
}

/// Percentage-based dimension. `100.0` means 100%.
#[derive(Debug, Clone, Copy)]
pub enum Dimension {
    Points(f32),
    Percent(f32),
}

/// A single node in the DOM tree.
#[derive(Debug, Clone)]
pub struct Node {
    pub id: NodeId,
    pub node_type: NodeType,
    pub parent: Option<NodeId>,
    pub children: Vec<NodeId>,
    pub style: StyleProps,
    pub text_content: Option<String>,
    pub image_src: Option<String>,
}
