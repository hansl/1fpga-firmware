use crate::modules::one_fpga::dom::render::{FontProp, RenderingContext};
use crate::modules::one_fpga::dom::style::Location;
use crate::modules::one_fpga::dom::{style, Tree};
use boa_engine::value::{Convert, TryFromJs, TryIntoJs};
use boa_engine::{js_error, Context, JsError, JsResult, JsValue};
use boa_macros::js_str;
use either::Either;
use embedded_graphics::geometry::Point;
use embedded_graphics::pixelcolor::BinaryColor;
use firmware_ui::macguiver::buffer::OsdBuffer;
use taffy::{AvailableSpace, Layout, Size as TaffySize};
use tracing::debug;
use u8g2_fonts::types::{FontColor, HorizontalAlignment, VerticalPosition};

fn measure_text(
    text: &str,
    font: FontProp,
    _known_dimensions: TaffySize<Option<f32>>,
    _available_space: TaffySize<AvailableSpace>,
    context: &mut RenderingContext,
) -> TaffySize<f32> {
    let renderer = context.fonts.get_or_create(font);

    let dimensions = renderer
        .get_rendered_dimensions(text, Point::zero(), VerticalPosition::Baseline)
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

#[derive(Debug, TryFromJs)]
pub(crate) struct BoxNodeProps {
    style: Option<style::BoxStyle>,
}

#[derive(Debug, TryFromJs)]
pub(crate) struct TextNodeProps {
    children: Option<Either<Vec<Convert<String>>, Convert<String>>>,
    style: Option<style::TStyle>,
}

impl TextNodeProps {
    pub(crate) fn inner_text(&self) -> Option<String> {
        if let Some(children) = &self.children {
            Some(children.as_ref().either(
                |l| {
                    l.iter().fold("".to_string(), |mut acc, el| {
                        acc.push_str(el.0.as_str());
                        acc
                    })
                },
                |r| r.0.clone(),
            ))
        } else {
            None
        }
    }
}

#[derive(Debug)]
pub(crate) enum NodeInfo {
    Root,
    Box {
        props: BoxNodeProps,
        calculated_font: Option<FontProp>,
        calculated_location: Option<Point>,
    },
    Text {
        props: TextNodeProps,
        text: String,
        calculated_font: Option<FontProp>,
        calculated_location: Option<Point>,
    },
    Fragment {
        text: String,
        font: FontProp,
        calculated_location: Option<Point>,
    },
}

impl NodeInfo {
    pub(crate) fn measure(
        &self,
        _tree: &Tree,
        known_dimensions: TaffySize<Option<f32>>,
        available_space: TaffySize<AvailableSpace>,
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
            NodeInfo::Box { .. } | NodeInfo::Root => taffy::Size::zero(),
            NodeInfo::Text {
                text,
                calculated_font,
                ..
            } => measure_text(
                text.as_str(),
                calculated_font.unwrap_or_default(),
                known_dimensions,
                available_space,
                context,
            ),
            NodeInfo::Fragment { text, font, .. } => measure_text(
                text.as_str(),
                *font,
                known_dimensions,
                available_space,
                context,
            ),
        }
    }

    pub fn render_to_osd(
        &self,
        layout: &Layout,
        target: &mut OsdBuffer,
        context: &mut RenderingContext,
    ) -> JsResult<()> {
        match self {
            NodeInfo::Text {
                text,
                calculated_font,
                ..
            } => {
                debug!(?text, ?calculated_font, "Rendering text");

                let renderer = context
                    .fonts
                    .get_or_create(calculated_font.unwrap_or_default());

                // Need to add height as `FontRenderer` renders "up".
                let position = if let Some(p) = self.calculated_location() {
                    p.into()
                } else {
                    Point::new(
                        layout.location.x as i32,
                        layout.location.y as i32 + layout.size.height as i32,
                    )
                };

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
            NodeInfo::Fragment { text, font, .. } => {
                debug!(?text, ?font, "Rendering fragment");
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

    pub(crate) fn text(&self) -> Option<&'_ str> {
        match self {
            NodeInfo::Fragment { text, .. } => Some(text.as_str()),
            _ => None,
        }
    }

    pub fn tag_name(&self) -> Option<&'_ str> {
        match self {
            NodeInfo::Box { .. } => Some("box"),
            NodeInfo::Text { .. } => Some("t"),
            _ => None,
        }
    }

    pub fn set_text(&mut self, new_text: String) -> JsResult<()> {
        match self {
            NodeInfo::Text { text, .. } => {
                *text = new_text;
                Ok(())
            }
            NodeInfo::Fragment { text, .. } => {
                *text = new_text;
                Ok(())
            }
            _ => Err(js_error!(ReferenceError: "Node cannot set text directly.")),
        }
    }

    pub fn update(&mut self, new_props: JsValue, context: &mut Context) -> JsResult<()> {
        match self {
            NodeInfo::Root => Err(js_error!(ReferenceError: "Cannot update props on Root")),
            NodeInfo::Box { props, .. } => {
                *props = BoxNodeProps::try_from_js(&new_props, context)?;
                Ok(())
            }
            NodeInfo::Text { props, text, .. } => {
                *props = TextNodeProps::try_from_js(&new_props, context)?;
                *text = props.inner_text().unwrap_or_default();
                Ok(())
            }
            NodeInfo::Fragment { .. } => {
                Err(js_error!(ReferenceError: "Cannot update props on Text Fragment"))
            }
        }
    }

    pub fn can_append(&self) -> bool {
        match self {
            NodeInfo::Root => true,
            NodeInfo::Box { .. } => true,
            NodeInfo::Text { .. } => true,
            NodeInfo::Fragment { .. } => false,
        }
    }

    pub fn tag(name: String, props: JsValue, context: &mut Context) -> JsResult<Self> {
        match name.as_str() {
            "box" => {
                eprintln!("box {}", props.display());
                let props = BoxNodeProps::try_from_js(&props, context)?;
                eprintln!("box2");
                Ok(NodeInfo::Box {
                    props,
                    calculated_font: None,
                    calculated_location: None,
                })
            }
            "t" => {
                eprintln!("t {}", props.display());
                let props = TextNodeProps::try_from_js(&props, context)?;
                eprintln!("t2");
                let text = props.inner_text().unwrap_or_default();
                eprintln!("t3");
                Ok(NodeInfo::Text {
                    props,
                    text,
                    calculated_font: None,
                    calculated_location: None,
                })
            }
            _ => Err(js_error!("Unknown node type {}", name)),
        }
    }

    pub fn style(&self, mut style: taffy::Style) -> taffy::Style {
        match self {
            NodeInfo::Root => {}
            NodeInfo::Box { props, .. } => {
                if let Some(s) = props.style
                    && s.location.is_some()
                {
                    style.position = taffy::Position::Absolute;
                }
                style.size = taffy::Size {
                    width: taffy::Dimension::auto(),
                    height: taffy::Dimension::auto(),
                };
                style.display = taffy::Display::Block;
            }
            NodeInfo::Text { .. } | NodeInfo::Fragment { .. } => {
                style.display = taffy::Display::Flex;
            }
        }

        style
    }

    pub fn fragment(text: String) -> JsResult<Self> {
        Ok(Self::Fragment {
            text,
            font: FontProp::default(),
            calculated_location: None,
        })
    }

    pub fn root() -> JsResult<Self> {
        Ok(Self::Root)
    }

    pub fn calculated_font(&self) -> Option<FontProp> {
        match self {
            NodeInfo::Root => None,
            NodeInfo::Box {
                calculated_font, ..
            } => *calculated_font,
            NodeInfo::Text {
                calculated_font, ..
            } => *calculated_font,
            NodeInfo::Fragment { font, .. } => Some(*font),
        }
    }

    pub fn calculated_location(&self) -> Option<Point> {
        match self {
            NodeInfo::Root => None,
            NodeInfo::Box {
                calculated_location: Some(l),
                ..
            } => Some(*l),
            NodeInfo::Text {
                calculated_location: Some(l),
                ..
            } => Some(*l),
            NodeInfo::Fragment {
                calculated_location: Some(l),
                ..
            } => Some(*l),
            NodeInfo::Box {
                props: BoxNodeProps { style: Some(s) },
                ..
            } => s.location.map(Into::into),
            _ => None,
        }
    }

    /// Update all the cache from the node's props and parent.
    pub fn update_cache(&mut self, parent: &NodeInfo) {
        match self {
            NodeInfo::Root => {}
            NodeInfo::Box {
                calculated_font,
                props,
                calculated_location,
                ..
            } => {
                if calculated_font.is_none() {
                    *calculated_font = Some(
                        props
                            .style
                            .clone()
                            .unwrap_or_default()
                            .font
                            .unwrap_or_default()
                            .inherits(parent.calculated_font().unwrap_or_default()),
                    );
                }

                if calculated_location.is_none() {
                    let new_location = parent.calculated_location();
                    *calculated_location = new_location;
                }
            }
            NodeInfo::Fragment { font, .. } => {
                // Always update it, but it's always equal to the parent.
                *font = parent.calculated_font().unwrap_or_default()
            }
            NodeInfo::Text {
                calculated_font,
                props,
                ..
            } => {
                if calculated_font.is_none() {
                    *calculated_font = Some(
                        props
                            .style
                            .clone()
                            .unwrap_or_default()
                            .font
                            .unwrap_or_default()
                            .inherits(parent.calculated_font().unwrap_or_default()),
                    )
                }
            }
        }
    }
}
