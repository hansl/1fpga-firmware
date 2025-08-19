use crate::modules::one_fpga::dom::node::{NodeInfo, NodeType};
use crate::modules::one_fpga::dom::render::{RenderingContext, RenderingContextRef};
use crate::AppRef;
use boa_engine::interop::ContextData;
use boa_engine::value::TryFromJs;
use boa_engine::{js_error, js_string, Context, JsError, JsResult, JsString, JsValue, Module};
use boa_macros::{boa_class, boa_module, Finalize, JsData, Trace};
use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::prelude::*;
use firmware_ui::macguiver::buffer::OsdBuffer;
use std::cell::{RefCell, UnsafeCell};
use std::fmt::Debug;
use std::ops::{Deref, DerefMut};
use std::rc::Rc;
use taffy::{AvailableSpace, NodeId, Style, TaffyTree, TraversePartialTree};
use tracing::debug;

mod node;
mod render;

type Tree = TaffyTree<NodeInfo>;

#[derive(Trace, Finalize, Clone, JsData)]
pub(crate) struct TreeRef(#[unsafe_ignore_trace] Rc<UnsafeCell<Tree>>);

impl Debug for TreeRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("TreeRef").field(&self.0).finish()
    }
}

impl TreeRef {
    pub fn new(tree: Tree) -> Self {
        Self(Rc::new(UnsafeCell::new(tree)))
    }
}

impl Deref for TreeRef {
    type Target = Tree;
    fn deref(&self) -> &Self::Target {
        unsafe { &*self.0.get() }
    }
}

impl DerefMut for TreeRef {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { &mut *self.0.get() }
    }
}

#[derive(Clone, Debug, Trace, Finalize, JsData)]
pub(crate) struct Node(#[unsafe_ignore_trace] NodeId);

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

    pub fn append(&self, ContextData(mut tree): ContextData<TreeRef>, child: Node) -> JsResult<()> {
        match tree.get_node_context(self.0) {
            None => Err(js_error!(ReferenceError:  "Node does not have context.")),
            Some(info) if !info.can_append() => {
                Err(js_error!(ReferenceError: "Node cannot contain children."))
            }
            _ => Ok(()),
        }?;

        let result = tree.add_child(self.0, child.0).map_err(JsError::from_rust);
        result
    }

    #[boa(getter)]
    pub fn text(&self, ContextData(tree): ContextData<TreeRef>) -> Option<JsString> {
        tree.get_node_context(self.0)
            .and_then(NodeInfo::text)
            .map(JsString::from)
    }

    #[boa(setter)]
    #[boa(rename = "text")]
    pub fn set_text(
        &self,
        ContextData(mut tree): ContextData<TreeRef>,
        new_text: String,
    ) -> JsResult<()> {
        tree.get_node_context_mut(self.0)
            .ok_or_else(|| js_error!(ReferenceError: "Node does not have context."))?
            .set_text(new_text)?;
        tree.mark_dirty(self.0).map_err(JsError::from_rust)?;
        Ok(())
    }

    #[boa(getter)]
    pub fn tag_name(&self, ContextData(tree): ContextData<TreeRef>) -> Option<JsString> {
        tree.get_node_context(self.0)
            .and_then(NodeInfo::tag_name)
            .map(JsString::from)
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
        context: &mut RenderingContext,
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
        ContextData(mut tree): ContextData<TreeRef>,
        ContextData(rendering_context): ContextData<RenderingContextRef>,
    ) -> JsResult<()> {
        debug!("layout");
        let start = std::time::Instant::now();
        let mut rendering_context = rendering_context.borrow_mut();
        let t = tree.clone();

        tree.compute_layout_with_measure(
            self.root.0,
            taffy::Size {
                width: AvailableSpace::Definite(self.buffer.borrow().size().width as f32),
                height: AvailableSpace::Definite(self.buffer.borrow().size().height as f32),
            },
            |known_dimensions, available_space, node_id, node_context, _style| {
                let parent = t.parent(node_id).and_then(|id| t.get_node_context(id));

                if let Some(n) = node_context {
                    if let Some(p) = parent {
                        n.calc_font(p);
                    }

                    n.measure(
                        &t,
                        known_dimensions,
                        available_space,
                        &mut rendering_context,
                    )
                } else {
                    taffy::Size::zero()
                }
            },
        )
        .map_err(JsError::from_rust)?;

        debug!("render");

        let mut target = self.buffer.borrow_mut();
        target.clear(BinaryColor::Off).unwrap();
        Self::render_to_osd(
            tree.deref(),
            self.root.0,
            target.deref_mut(),
            &mut rendering_context,
        )?;
        app.platform_mut().update_osd(target.deref_mut());

        let elapsed = start.elapsed();
        debug!(?elapsed, "Render complete");

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
    use crate::modules::one_fpga::dom::node::{NodeInfo, NodeProps};
    use boa_engine::interop::ContextData;
    use boa_engine::{Context, JsError, JsResult};

    type Node = super::Node;

    fn root(ContextData(mut root): ContextData<super::Root>) -> JsResult<super::Root> {
        Ok(root.clone())
    }

    fn create_node(
        ContextData(mut tree): ContextData<super::TreeRef>,
        name: String,
        props: NodeProps,
        context: &mut Context,
    ) -> JsResult<super::Node> {
        tracing::trace!(?name, ?props, "Creating node");

        let node = super::NodeInfo::tag(name, props)?;
        let style = node.style((taffy::Style::default())?;
        let id = tree
            .new_leaf_with_context(style, node)
            .map_err(JsError::from_rust)?;

        Ok(super::Node(id))
    }

    fn create_fragment(
        ContextData(mut tree): ContextData<super::TreeRef>,
        text: String,
    ) -> JsResult<super::Node> {
        tracing::trace!(?text, "Creating fragment");

        let style = super::Style::default();
        let id = tree
            .new_leaf_with_context(style, NodeInfo::fragment(text)?)
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
    let mut taffy = TreeRef::new(Tree::new());
    context.insert_data(taffy.clone());

    let rendering = RenderingContext::default().into_ref();
    context.insert_data(rendering);

    let root = taffy
        .new_leaf_with_context(Style::default(), NodeInfo::root()?)
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
