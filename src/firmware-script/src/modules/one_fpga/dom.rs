use crate::AppRef;
use boa_engine::interop::ContextData;
use boa_engine::value::TryFromJs;
use boa_engine::{js_error, js_string, Context, JsError, JsResult, JsString, JsValue, Module};
use boa_macros::{boa_class, boa_module, Finalize, JsData, Trace};
use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::Rectangle;
use embedded_text::TextBox;
use firmware_ui::macguiver::buffer::OsdBuffer;
use std::cell::{Ref, RefCell, RefMut};
use std::fmt::Debug;
use std::ops::{Deref, DerefMut};
use std::rc::Rc;
use taffy::{AvailableSpace, NodeId, Style, TaffyTree, TraversePartialTree};

#[derive(Debug)]
pub enum NodeInfo {
    Root,
    Tag { name: String },
    Fragment(String),
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
            Some(NodeInfo::Fragment(text)) => Some(JsString::from(text.as_str())),
            _ => None,
        }
    }

    #[boa(getter)]
    pub fn set_text(
        &self,
        ContextData(tree): ContextData<TreeRef>,
        new_text: String,
    ) -> JsResult<()> {
        match tree.borrow_mut().get_node_context_mut(self.0) {
            None => Err(js_error!(ReferenceError: "Node does not have context.")),
            Some(NodeInfo::Fragment(old)) => {
                *old = new_text;
                Ok(())
            }
            Some(_) => Err(js_error!(ReferenceError: "Node is not a Fragment.")),
        }
    }

    #[boa(getter)]
    pub fn tag_name(&self, ContextData(tree): ContextData<TreeRef>) -> Option<JsString> {
        match tree.borrow().get_node_context(self.0) {
            Some(NodeInfo::Tag { name }) => Some(JsString::from(name.as_str())),
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
    fn render_to_osd(tree: &Tree, node: NodeId, target: &mut OsdBuffer) -> JsResult<()> {
        for child in tree.child_ids(node) {
            Root::render_to_osd(tree, child, target)?;
        }

        match tree.get_node_context(node) {
            Some(NodeInfo::Fragment(text)) => {
                let layout = tree.layout(node).unwrap();

                let character_style = u8g2_fonts::U8g2TextStyle::new(
                    u8g2_fonts::fonts::u8g2_font_haxrcorp4089_t_cyrillic,
                    BinaryColor::On,
                );

                TextBox::new(
                    &text,
                    Rectangle::new(
                        Point::new(layout.location.x as i32, layout.location.y as i32),
                        Size::new(layout.size.width as u32, layout.size.height as u32),
                    ),
                    character_style,
                )
                .draw(target)
                .map_err(JsError::from_rust)?;
            }
            _ => {}
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
        eprintln!("Root::layout");
        tree.borrow_mut()
            .compute_layout(
                self.root.0,
                taffy::Size {
                    width: AvailableSpace::Definite(self.buffer.borrow().size().width as f32),
                    height: AvailableSpace::MinContent,
                },
            )
            .map_err(JsError::from_rust)?;

        eprintln!("Root::render");
        let mut target = self.buffer.borrow_mut();
        target.clear(BinaryColor::Off).unwrap();
        Self::render_to_osd(tree.borrow().deref(), self.root.0, target.deref_mut())?;
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
    use std::collections::BTreeMap;

    type Node = super::Node;

    fn root(ContextData(mut root): ContextData<super::Root>) -> JsResult<super::Root> {
        Ok(root.clone())
    }

    fn create_node(
        ContextData(mut tree): ContextData<super::TreeRef>,
        ty: String,
        props: BTreeMap<String, JsValue>,
    ) -> JsResult<super::Node> {
        tracing::trace!(?ty, ?props, "Creating node");

        let id = tree
            .borrow_mut()
            .new_leaf_with_context(super::Style::default(), NodeInfo::Tag { name: ty })
            .map_err(JsError::from_rust)?;

        Ok(super::Node(id))
    }

    fn create_fragment(
        ContextData(mut tree): ContextData<super::TreeRef>,
        content: String,
    ) -> JsResult<super::Node> {
        tracing::trace!(?content, "Creating fragment");

        let id = tree
            .borrow_mut()
            .new_leaf_with_context(super::Style::default(), NodeInfo::Fragment(content))
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
