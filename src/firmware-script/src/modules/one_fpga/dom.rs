use crate::AppRef;
use boa_engine::interop::ContextData;
use boa_engine::value::TryFromJs;
use boa_engine::{js_error, js_string, Context, JsError, JsResult, JsString, JsValue, Module};
use boa_macros::{boa_class, boa_module, Finalize, JsData, Trace};
use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::Rectangle;
use embedded_graphics::text::renderer::TextRenderer;
use embedded_text::TextBox;
use firmware_ui::macguiver::buffer::OsdBuffer;
use std::cell::{Ref, RefCell, RefMut};
use std::fmt::Debug;
use std::ops::{Deref, DerefMut};
use std::rc::Rc;
use taffy::{AvailableSpace, Layout, NodeId, Style, TaffyTree, TraversePartialTree};
use u8g2_fonts::U8g2TextStyle;

#[derive(Debug)]
struct RenderingContext {
    font: U8g2TextStyle<BinaryColor>,
}

#[derive(Debug)]
struct TextContext {
    text: String,
}

impl TextContext {
    pub(crate) fn measure(
        &self,
        known_dimensions: taffy::geometry::Size<Option<f32>>,
        available_space: taffy::geometry::Size<AvailableSpace>,
        _context: &RenderingContext,
    ) -> taffy::geometry::Size<f32> {
        eprintln!(
            "layout::fragment\n  text: {}\n  dimensions: {known_dimensions:?}\n  available: {available_space:?}",
            self.text,
        );

        let inline_axis = taffy::AbsoluteAxis::Horizontal;
        let block_axis = inline_axis.other_axis();

        if self.text.is_empty() {
            return taffy::geometry::Size::zero();
        }

        let char_width = 12.0f32;
        let char_height = 14.0f32;
        let words: Vec<&str> = self.text.split_whitespace().collect();

        let min_line_length: usize = words.iter().map(|line| line.len()).max().unwrap_or(0);
        let max_line_length: usize = words.iter().map(|line| line.len()).sum();
        let inline_size = known_dimensions.get_abs(inline_axis).unwrap_or_else(|| {
            match available_space.get_abs(inline_axis) {
                AvailableSpace::MinContent => min_line_length as f32 * char_width,
                AvailableSpace::MaxContent => max_line_length as f32 * char_width,
                AvailableSpace::Definite(inline_size) => inline_size
                    .min(max_line_length as f32 * char_width)
                    .max(min_line_length as f32 * char_width),
            }
        });
        let block_size = known_dimensions.get_abs(block_axis).unwrap_or_else(|| {
            let inline_line_length = (inline_size / char_width).floor() as usize;
            let mut line_count = 1;
            let mut current_line_length = 0;
            for word in &words {
                if current_line_length == 0 {
                    // first word
                    current_line_length = word.len();
                } else if current_line_length + word.len() + 1 > inline_line_length {
                    // every word past the first needs to check for line length including the space between words
                    // note: a real implementation of this should handle whitespace characters other than ' '
                    // and do something more sophisticated for long words
                    line_count += 1;
                    current_line_length = word.len();
                } else {
                    // add the word and a space
                    current_line_length += word.len() + 1;
                };
            }
            (line_count as f32) * char_height
        });

        // match text_context.writing_mode {
        //     WritingMode::Horizontal => Size {
        //         width: inline_size,
        //         height: block_size,
        //     },
        //     WritingMode::Vertical => Size {
        //         width: block_size,
        //         height: inline_size,
        //     },
        // }

        taffy::Size {
            width: inline_size,
            height: block_size,
        }
    }
}

#[derive(Debug)]
struct TagContext {
    name: String,
    font: U8g2TextStyle<BinaryColor>,
}

#[derive(Debug)]
enum NodeInfo {
    Root,
    Tag(TagContext),
    Fragment(TextContext),
}

impl NodeInfo {
    fn measure(
        &self,
        known_dimensions: taffy::geometry::Size<Option<f32>>,
        available_space: taffy::geometry::Size<taffy::style::AvailableSpace>,
        context: &RenderingContext,
    ) -> taffy::geometry::Size<f32> {
        match self {
            NodeInfo::Root => known_dimensions.unwrap_or(taffy::Size::zero()),
            NodeInfo::Tag(_) => taffy::Size::zero(),
            NodeInfo::Fragment(fragment) => {
                fragment.measure(known_dimensions, available_space, context)
            }
        }
    }

    pub(crate) fn render_to_osd(
        &self,
        layout: &Layout,
        target: &mut OsdBuffer,
        context: &RenderingContext,
    ) -> JsResult<()> {
        match self {
            NodeInfo::Fragment(TextContext { text, .. }) => {
                eprintln!("render fragment: {text:?} {layout:?}");

                TextBox::new(
                    &text,
                    Rectangle::new(
                        Point::new(layout.location.x as i32, layout.location.y as i32),
                        Size::new(layout.size.width as u32, layout.size.height as u32),
                    ),
                    context.font.clone(),
                )
                .draw(target)
                .unwrap();
                Ok(())
            }
            _ => Ok(()),
        }
    }
}

pub type Tree = TaffyTree<NodeInfo>;

#[derive(Trace, Finalize, Debug, Clone, JsData)]
pub struct TreeRef(#[unsafe_ignore_trace] Rc<RefCell<Tree>>);

impl TreeRef {
    pub fn new(tree: Tree) -> Self {
        Self(Rc::new(RefCell::new(tree)))
    }

    pub fn borrow(&self) -> Ref<Tree> {
        self.0.borrow()
    }

    pub fn borrow_mut(&self) -> RefMut<Tree> {
        self.0.borrow_mut()
    }
}

#[derive(Clone, Debug, Trace, Finalize, JsData)]
pub struct Node(#[unsafe_ignore_trace] NodeId);

impl TryFromJs for Node {
    fn try_from_js(value: &JsValue, _context: &mut Context) -> JsResult<Self> {
        value
            .as_object()
            .and_then(|o| o.downcast_ref::<Node>().map(|n| n.clone()))
            .ok_or_else(|| js_error!("Value is not a Node"))
    }
}

#[boa_class]
impl Node {
    #[boa(constructor)]
    pub fn constructor() -> JsResult<Self> {
        Err(js_error!(TypeError: "Cannot construct a Node directly."))
    }

    pub fn append(&self, ContextData(tree): ContextData<TreeRef>, child: Node) -> JsResult<()> {
        match tree.borrow().get_node_context(self.0) {
            None => Err(js_error!(ReferenceError:  "Node does not have context.")),
            Some(NodeInfo::Fragment(_)) => Err(js_error!(ReferenceError: "Node is a fragment.")),
            _ => Ok(()),
        }?;

        let result = tree
            .borrow_mut()
            .add_child(self.0, child.0)
            .map_err(JsError::from_rust);
        result
    }

    #[boa(getter)]
    pub fn text(&self, ContextData(tree): ContextData<TreeRef>) -> Option<JsString> {
        match tree.borrow().get_node_context(self.0) {
            Some(NodeInfo::Fragment(TextContext { text })) => Some(JsString::from(text.as_str())),
            _ => None,
        }
    }

    #[boa(setter)]
    #[boa(rename = "text")]
    pub fn set_text(
        &self,
        ContextData(tree): ContextData<TreeRef>,
        new_text: String,
    ) -> JsResult<()> {
        let mut tree = tree.borrow_mut();

        match tree.get_node_context_mut(self.0) {
            None => Err(js_error!(ReferenceError: "Node does not have context.")),
            Some(NodeInfo::Fragment(old)) => {
                old.text = new_text;
                tree.mark_dirty(self.0).map_err(JsError::from_rust)?;
                Ok(())
            }
            Some(_) => Err(js_error!(ReferenceError: "Node is not a Fragment.")),
        }
    }

    #[boa(getter)]
    pub fn tag_name(&self, ContextData(tree): ContextData<TreeRef>) -> Option<JsString> {
        match tree.borrow().get_node_context(self.0) {
            Some(NodeInfo::Tag(TagContext { name, .. })) => Some(JsString::from(name.as_str())),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Trace, Finalize, JsData)]
pub struct Root {
    root: Node,

    #[unsafe_ignore_trace]
    buffer: Rc<RefCell<OsdBuffer>>,
}

impl Root {
    fn render_to_osd(
        tree: &Tree,
        node: NodeId,
        target: &mut OsdBuffer,
        context: &RenderingContext,
    ) -> JsResult<()> {
        let layout = tree.layout(node).unwrap();
        if let Some(n) = tree.get_node_context(node) {
            n.render_to_osd(layout, target, context)?;
        }

        for child in tree.child_ids(node) {
            Root::render_to_osd(tree, child, target, context)?;
        }

        Ok(())
    }
}

#[boa_class]
impl Root {
    #[boa(constructor)]
    pub fn constructor() -> JsResult<Self> {
        Err(js_error!(TypeError: "Cannot construct a Root directly."))
    }

    fn render(
        &self,
        ContextData(mut app): ContextData<AppRef>,
        ContextData(tree): ContextData<TreeRef>,
    ) -> JsResult<()> {
        let font = u8g2_fonts::U8g2TextStyle::new(
            u8g2_fonts::fonts::u8g2_font_unifont_t_chinese3,
            BinaryColor::On,
        );

        let context = RenderingContext { font };
        eprintln!("Root::layout");
        tree.borrow_mut()
            .compute_layout_with_measure(
                self.root.0,
                taffy::Size {
                    width: AvailableSpace::Definite(self.buffer.borrow().size().width as f32),
                    height: AvailableSpace::MinContent,
                },
                |known_dimensions, available_space, _node_id, node_context, _style| {
                    if let Some(n) = node_context {
                        n.measure(known_dimensions, available_space, &context)
                    } else {
                        taffy::Size::zero()
                    }
                },
            )
            .map_err(JsError::from_rust)?;

        eprintln!("Root::render");
        let mut target = self.buffer.borrow_mut();
        target.clear(BinaryColor::Off).unwrap();
        Self::render_to_osd(
            tree.borrow().deref(),
            self.root.0,
            target.deref_mut(),
            &context,
        )?;
        app.platform_mut().update_osd(target.deref_mut());

        Ok(())
    }

    pub fn append(&self, tree_data: ContextData<TreeRef>, child: Node) -> JsResult<()> {
        self.root.append(tree_data, child)
    }
}

#[boa_module]
mod globals {
    type Root = super::Root;
    type Node = super::Node;
}

#[boa_module]
mod js {
    use super::NodeInfo;
    use boa_engine::interop::ContextData;
    use boa_engine::{JsError, JsResult, JsValue};
    use embedded_graphics::pixelcolor::BinaryColor;
    use std::collections::BTreeMap;

    type Node = super::Node;

    fn root(ContextData(mut root): ContextData<super::Root>) -> JsResult<super::Root> {
        Ok(root.clone())
    }

    fn create_node(
        ContextData(mut tree): ContextData<super::TreeRef>,
        name: String,
        props: BTreeMap<String, JsValue>,
    ) -> JsResult<super::Node> {
        tracing::trace!(?name, ?props, "Creating node");
        let font = u8g2_fonts::U8g2TextStyle::new(
            u8g2_fonts::fonts::u8g2_font_haxrcorp4089_t_cyrillic,
            BinaryColor::On,
        );

        let id = tree
            .borrow_mut()
            .new_leaf_with_context(
                super::Style::default(),
                NodeInfo::Tag(super::TagContext { name, font }),
            )
            .map_err(JsError::from_rust)?;

        Ok(super::Node(id))
    }

    fn create_fragment(
        ContextData(mut tree): ContextData<super::TreeRef>,
        text: String,
    ) -> JsResult<super::Node> {
        tracing::trace!(?text, "Creating fragment");

        let id = tree
            .borrow_mut()
            .new_leaf_with_context(
                super::Style::default(),
                NodeInfo::Fragment(super::TextContext { text }),
            )
            .map_err(JsError::from_rust)?;

        Ok(super::Node(id))
    }
}

pub fn create_module(context: &mut Context) -> JsResult<(JsString, Module)> {
    globals::boa_register(None, context)?;

    let mut app_ref = context
        .get_data::<AppRef>()
        .expect("No app registered")
        .clone();
    let taffy = TreeRef::new(Tree::new());
    context.insert_data(taffy.clone());

    let root = taffy
        .borrow_mut()
        .new_leaf_with_context(Style::default(), NodeInfo::Root)
        .expect("Could not create the DOM root.");

    let root = Root {
        root: Node(root),
        buffer: Rc::new(RefCell::new(OsdBuffer::new(
            app_ref.deref_mut().platform_mut().osd_dimensions(),
        ))),
    };
    context.insert_data(root);

    Ok((js_string!("dom"), js::boa_module(None, context)))
}
