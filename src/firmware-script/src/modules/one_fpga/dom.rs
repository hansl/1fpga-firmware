use crate::AppRef;
use boa_engine::interop::ContextData;
use boa_engine::value::TryFromJs;
use boa_engine::{js_error, js_string, Context, JsError, JsResult, JsString, JsValue, Module};
use boa_macros::{boa_class, boa_module, Finalize, JsData, Trace, TryIntoJs};
use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::prelude::*;
use firmware_ui::macguiver::buffer::OsdBuffer;
use std::cell::{Ref, RefCell, RefMut, UnsafeCell};
use std::collections::HashMap;
use std::fmt::Debug;
use std::ops::{Deref, DerefMut};
use std::rc::Rc;
use std::str::FromStr;
use taffy::{AvailableSpace, Layout, NodeId, Style, TaffyTree, TraversePartialTree};
use tracing::debug;
use u8g2_fonts::types::{FontColor, VerticalPosition};
use u8g2_fonts::FontRenderer;

type DefaultFont = u8g2_fonts::fonts::u8g2_font_haxrcorp4089_t_cyrillic;

#[derive(Default, Copy, Clone, Debug, Hash, Eq, PartialEq)]
enum FontType {
    #[default]
    None,
    Monospace,
    SansSerif,
}

impl FromStr for FontType {
    type Err = JsError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "default" => Ok(Self::None),
            "monospace" => Ok(FontType::Monospace),
            "sans" | "sans-serif" => Ok(FontType::SansSerif),
            _ => Err(js_error!(Error: "Unknown font type {}", s)),
        }
    }
}

#[derive(Default, Copy, Clone, Debug, Hash, Eq, PartialEq)]
enum FontSize {
    #[default]
    None,
    Small,
    Medium,
    Large,
}

impl FromStr for FontSize {
    type Err = JsError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "default" => Ok(FontSize::None),
            "small" => Ok(FontSize::Small),
            "medium" => Ok(FontSize::Medium),
            "large" => Ok(FontSize::Large),
            _ => Err(js_error!(Error: "Unknown font size {}", s)),
        }
    }
}

#[derive(Default, Copy, Clone, Debug, Hash, Eq, PartialEq)]
struct FontProp(FontType, FontSize);

impl FontProp {
    pub fn renderer(&self) -> FontRenderer {
        use u8g2_fonts::fonts::*;
        let font_type = if self.0 == FontType::None {
            FontType::SansSerif
        } else {
            self.0
        };
        let font_size = if self.1 == FontSize::None {
            FontSize::Medium
        } else {
            self.1
        };

        match (font_type, font_size) {
            (FontType::Monospace, FontSize::Small) => {
                FontRenderer::new::<u8g2_font_boutique_bitmap_7x7_t_all>()
            }
            (FontType::Monospace, FontSize::Medium) => {
                FontRenderer::new::<u8g2_font_boutique_bitmap_9x9_t_all>()
            }
            (FontType::Monospace, FontSize::Large) => {
                FontRenderer::new::<u8g2_font_unifont_t_chinese3>()
            }
            (FontType::SansSerif, FontSize::Small) => {
                FontRenderer::new::<u8g2_font_wqy12_t_chinese3>()
            }
            (FontType::SansSerif, FontSize::Medium) => {
                FontRenderer::new::<u8g2_font_wqy14_t_chinese3>()
            }
            (FontType::SansSerif, FontSize::Large) => {
                FontRenderer::new::<u8g2_font_wqy16_t_chinese3>()
            }
            (FontType::None, _) | (_, FontSize::None) => unsafe {
                // SAFETY: Verification done above.
                std::hint::unreachable_unchecked()
            },
        }
    }

    pub fn inherits(&self, other: FontProp) -> Self {
        match (self.0, self.1) {
            (FontType::None, FontSize::None) => other,
            (FontType::None, _) => Self(other.0, self.1),
            (_, FontSize::None) => Self(self.0, other.1),
            _ => *self,
        }
    }
}

impl TryFromJs for FontProp {
    fn try_from_js(value: &JsValue, context: &mut Context) -> JsResult<Self> {
        Self::from_str(value.to_string(context)?.to_std_string_lossy().as_str())
    }
}

impl FromStr for FontProp {
    type Err = JsError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Some((first, second)) = s.split_once(',') {
            Ok(Self(
                FontType::from_str(first)?,
                FontSize::from_str(second)?,
            ))
        } else if s.is_empty() || s == "default" {
            Ok(Self(FontType::None, FontSize::None))
        } else {
            if let Ok(ty) = FontType::from_str(s) {
                Ok(Self(ty, FontSize::None))
            } else if let Ok(sz) = FontSize::from_str(s) {
                Ok(Self(FontType::None, sz))
            } else {
                Err(js_error!(Error: "Unknown font specification {}", s))
            }
        }
    }
}

#[derive(Default, Clone, Debug)]
struct FontRendererCache(HashMap<FontProp, FontRenderer>);

impl FontRendererCache {
    pub fn get_or_create(&mut self, spec: FontProp) -> &FontRenderer {
        self.0.entry(spec).or_insert(spec.renderer())
    }

    pub fn get(&self, spec: &FontProp) -> Option<&FontRenderer> {
        self.0.get(spec)
    }
}

#[derive(Default, Debug)]
struct RenderingContext {
    fonts: FontRendererCache,
}

#[derive(Debug)]
struct TextContext {
    text: String,
    font: FontProp,
}

impl TextContext {
    pub(crate) fn measure(
        &self,
        _known_dimensions: taffy::geometry::Size<Option<f32>>,
        _available_space: taffy::geometry::Size<AvailableSpace>,
        context: &mut RenderingContext,
    ) -> taffy::geometry::Size<f32> {
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
struct TagContext {
    name: String,
    calculated_font: FontProp,
}

#[derive(Debug)]
enum NodeType {
    Root,
    Box { calculated_font: Option<FontProp> },
    Fragment(TextContext),
}

impl NodeType {
    fn font_spec(&self) -> FontProp {
        match self {
            NodeType::Root => FontProp(FontType::None, FontSize::None),
            NodeType::Box {
                calculated_font: Some(font_cache),
            } => *font_cache,
            NodeType::Box {
                calculated_font: None,
            } => FontProp(FontType::None, FontSize::None),
            NodeType::Fragment(TextContext { font, .. }) => *font,
        }
    }

    fn measure(
        &mut self,
        known_dimensions: taffy::geometry::Size<Option<f32>>,
        available_space: taffy::geometry::Size<AvailableSpace>,
        context: &mut RenderingContext,
    ) -> taffy::geometry::Size<f32> {
        match self {
            NodeType::Root => known_dimensions.unwrap_or(taffy::Size::zero()),
            NodeType::Box { .. } => taffy::Size::zero(),
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

#[derive(Debug)]
struct NodeInfo {
    node_type: NodeType,
    props: Option<NodeProps>,
}

impl NodeInfo {
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

    // Update font spec from the parent.
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
}

type Tree = TaffyTree<NodeInfo>;

#[derive(Trace, Finalize, Clone, JsData)]
struct TreeRef(#[unsafe_ignore_trace] Rc<UnsafeCell<Tree>>);

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

#[derive(Trace, Finalize, Debug, Clone, JsData)]
pub struct RenderingContextRef(#[unsafe_ignore_trace] Rc<RefCell<RenderingContext>>);

impl RenderingContextRef {
    pub fn new() -> Self {
        Self(Rc::new(RefCell::new(RenderingContext::default())))
    }

    pub fn borrow(&self) -> Ref<RenderingContext> {
        self.0.borrow()
    }

    pub fn borrow_mut(&self) -> RefMut<RenderingContext> {
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
        match tree.get_node_context(self.0) {
            Some(NodeInfo {
                node_type: NodeType::Fragment(TextContext { text, .. }),
                ..
            }) => Some(JsString::from(text.as_str())),
            _ => None,
        }
    }

    #[boa(setter)]
    #[boa(rename = "text")]
    pub fn set_text(
        &self,
        ContextData(mut tree): ContextData<TreeRef>,
        new_text: String,
    ) -> JsResult<()> {
        match tree.get_node_context_mut(self.0) {
            None => Err(js_error!(ReferenceError: "Node does not have context.")),
            Some(NodeInfo {
                node_type: NodeType::Fragment(old),
                ..
            }) => {
                old.text = new_text;
                tree.mark_dirty(self.0).map_err(JsError::from_rust)?;
                Ok(())
            }
            Some(_) => Err(js_error!(ReferenceError: "Node is not a Fragment.")),
        }
    }

    #[boa(getter)]
    pub fn tag_name(&self, ContextData(tree): ContextData<TreeRef>) -> Option<JsString> {
        match tree.get_node_context(self.0) {
            Some(NodeInfo {
                node_type: NodeType::Box { .. },
                ..
            }) => Some(js_string!("box")),
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
        context: &mut RenderingContext,
    ) -> JsResult<()> {
        let layout = tree.layout(node).unwrap();
        if let Some(n) = tree.get_node_context(node) {
            n.node_type.render_to_osd(layout, target, context)?;
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
                height: AvailableSpace::MinContent,
            },
            |known_dimensions, available_space, node_id, node_context, _style| {
                let parent = t.parent(node_id).and_then(|id| t.get_node_context(id));

                if let Some(mut n) = node_context {
                    if let Some(p) = parent {
                        n.calc_font(p);
                    }

                    n.node_type
                        .measure(known_dimensions, available_space, &mut rendering_context)
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

#[derive(Debug, TryFromJs)]
struct NodeProps {
    font: Option<FontProp>,
    location: Option<JsPoint>,
}

#[boa_module]
mod globals {
    type Root = super::Root;
    type Node = super::Node;
}

#[boa_module]
mod js {
    use boa_engine::interop::ContextData;
    use boa_engine::{Context, JsError, JsResult};

    type Node = super::Node;

    fn root(ContextData(mut root): ContextData<super::Root>) -> JsResult<super::Root> {
        Ok(root.clone())
    }

    fn create_node(
        ContextData(mut tree): ContextData<super::TreeRef>,
        name: String,
        props: super::NodeProps,
        context: &mut Context,
    ) -> JsResult<super::Node> {
        tracing::trace!(?name, ?props, "Creating node");

        let id = tree
            .new_leaf_with_context(super::Style::default(), super::NodeInfo::tag(name, props)?)
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
            .new_leaf_with_context(style, super::NodeInfo::fragment(text)?)
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

    let rendering = RenderingContext::default();
    context.insert_data(RenderingContextRef(Rc::new(RefCell::new(rendering))));

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
