use crate::modules::one_fpga::dom::render::{FontProp, RenderingContext};
use crate::modules::one_fpga::dom::Tree;
use boa_engine::{js_error, JsResult};
use boa_macros::{TryFromJs, TryIntoJs};
use embedded_graphics::geometry::Point;
use embedded_graphics::pixelcolor::BinaryColor;
use firmware_ui::macguiver::buffer::OsdBuffer;
use taffy::{AvailableSpace, Layout, Size as TaffySize};
use tracing::debug;
use u8g2_fonts::types::{FontColor, VerticalPosition};

#[derive(Debug)]
pub(crate) struct TextContext {
    text: String,
    font: FontProp,
}

impl TextContext {
    pub(crate) fn measure(
        &self,
        _known_dimensions: TaffySize<Option<f32>>,
        _available_space: TaffySize<AvailableSpace>,
        context: &mut RenderingContext,
    ) -> TaffySize<f32> {
        let renderer = context.fonts.get_or_create(self.font);

        let dimensions = renderer
            .get_rendered_dimensions(
                self.text.as_str(),
                Point::zero(),
                VerticalPosition::Baseline,
            )
            .expect("Failed to get rendered dimensions");

        if let Some(bb) = dimensions.bounding_box {
            // Use dimensions.advance.x to account for white spaces.
            taffy::Size {
                width: dimensions.advance.x as _,
                height: bb.size.height as _,
            }
        } else {
            taffy::Size {
                width: dimensions.advance.x as _,
                height: dimensions.advance.y as _,
            }
        }
    }
}

#[derive(Debug)]
pub(crate) enum NodeType {
    Root,
    Box { calculated_font: Option<FontProp> },
    Fragment(TextContext),
}

impl NodeType {
    fn measure(
        &mut self,
        tree: &Tree,
        known_dimensions: taffy::geometry::Size<Option<f32>>,
        available_space: taffy::geometry::Size<AvailableSpace>,
        context: &mut RenderingContext,
    ) -> taffy::geometry::Size<f32> {
        if let taffy::geometry::Size {
            width: Some(width),
            height: Some(height),
        } = known_dimensions
        {
            return taffy::geometry::Size { width, height };
        }

        match self {
            NodeType::Box { .. } | NodeType::Root => taffy::Size::zero(),
            NodeType::Fragment(fragment) => {
                fragment.measure(known_dimensions, available_space, context)
            }
        }
    }

    pub(crate) fn render_to_osd(
        &self,
        layout: &Layout,
        target: &mut OsdBuffer,
        context: &mut RenderingContext,
    ) -> JsResult<()> {
        match self {
            NodeType::Fragment(TextContext { text, font }) => {
                debug!(?text, ?font, "Rendering text");
                let renderer = context.fonts.get_or_create(*font);

                // Need to add height as `FontRenderer` renders "up".
                let position = Point::new(
                    layout.location.x as i32,
                    layout.location.y as i32 + layout.size.height as i32,
                );

                renderer
                    .render(
                        text.as_str(),
                        position,
                        VerticalPosition::Baseline,
                        FontColor::Transparent(BinaryColor::On),
                        target,
                    )
                    .expect("Failed to render text");

                Ok(())
            }
            _ => Ok(()),
        }
    }
}

#[derive(Debug, TryFromJs, TryIntoJs)]
struct JsPoint {
    x: i32,
    y: i32,
}

#[derive(Debug, TryFromJs)]
pub(crate) struct NodeProps {
    font: Option<FontProp>,
    location: Option<JsPoint>,
}

#[derive(Debug)]
pub(crate) struct NodeInfo {
    node_type: NodeType,
    props: Option<NodeProps>,
}

impl NodeInfo {
    pub(crate) fn text(&self) -> Option<&'_ str> {
        match &self.node_type {
            NodeType::Fragment(TextContext { text, .. }) => Some(text.as_str()),
            _ => None,
        }
    }

    pub fn tag_name(&self) -> Option<&'_ str> {
        match &self.node_type {
            NodeType::Box { .. } => Some("box"),
            _ => None,
        }
    }

    pub fn set_text(&mut self, text: String) -> JsResult<()> {
        match self {
            NodeInfo {
                node_type: NodeType::Fragment(old),
                ..
            } => {
                old.text = text;
                Ok(())
            }
            _ => Err(js_error!(ReferenceError: "Node is not a Fragment.")),
        }
    }

    pub fn measure(
        &mut self,
        tree: &Tree,
        known_dimensions: TaffySize<Option<f32>>,
        available_space: TaffySize<AvailableSpace>,
        rendering_context: &mut RenderingContext,
    ) -> TaffySize<f32> {
        self.node_type
            .measure(tree, known_dimensions, available_space, rendering_context)
    }

    pub fn can_append(&self) -> bool {
        match &self.node_type {
            NodeType::Root => true,
            NodeType::Box { .. } => true,
            NodeType::Fragment(_) => false,
        }
    }

    pub fn tag(name: String, props: NodeProps) -> JsResult<Self> {
        let node_type = match name.as_str() {
            "box" => NodeType::Box {
                calculated_font: None,
            },
            _ => return Err(js_error!("Unknown node type {}", name)),
        };

        Ok(NodeInfo {
            node_type,
            props: Some(props),
        })
    }

    pub fn style(&self, mut style: taffy::Style) -> taffy::Style {
        if let Some(props) = self.props {
            if let Some(location) = props {
                style.position = taffy::Position::Absolute;
            }
        }

        style
    }

    pub fn fragment(text: String) -> JsResult<Self> {
        let context = TextContext {
            text,
            font: FontProp::default(),
        };

        Ok(Self {
            node_type: NodeType::Fragment(context),
            props: None,
        })
    }

    pub fn root() -> JsResult<Self> {
        Ok(Self {
            node_type: NodeType::Root,
            props: None,
        })
    }

    pub fn font_cache(&self) -> Option<FontProp> {
        match self.node_type {
            NodeType::Root => None,
            NodeType::Box {
                calculated_font: font_cache,
            } => font_cache,
            NodeType::Fragment(TextContext { font, .. }) => Some(font),
        }
    }

    /// Update font spec from the parent.
    pub fn calc_font(&mut self, parent: &NodeInfo) {
        match &mut self.node_type {
            NodeType::Root => {}
            NodeType::Box {
                calculated_font: font_cache,
            } => {
                if font_cache.is_none()
                    && let Some(ref props) = self.props
                    && let Some(ref parent) = parent.font_cache()
                {
                    *font_cache = Some(props.font.unwrap_or_default().inherits(*parent))
                }
            }
            NodeType::Fragment(f) => {
                // Always update it, but it's always equal to the parent.
                f.font = parent.font_cache().unwrap_or_default()
            }
        }
    }

    pub fn render_to_osd(
        &self,
        layout: &Layout,
        target: &mut OsdBuffer,
        context: &mut RenderingContext,
    ) -> JsResult<()> {
        self.node_type.render_to_osd(layout, target, context)
    }
}
