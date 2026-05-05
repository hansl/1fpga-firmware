//! `1fpga:gui` host module exposed to Boa.
//!
//! N1 surface: `createInstance`, `appendChild`, `setStyle`, `run`. All
//! mutate or read [`UiState`], a `Trace`/`Finalize`/`JsData` handle to
//! the Rust-side host tree that sits in the Boa context.
//!
//! Subsequent milestones add `commitUpdate`, `commitTextUpdate`,
//! reconciler-shaped helpers, asset registration, and input.

use std::cell::RefCell;
use std::rc::Rc;

use boa_engine::module::{IntoJsModule, MapModuleLoader};
use boa_engine::{
    Context, JsArgs, JsError, JsNativeError, JsObject, JsResult, JsString, JsValue, NativeFunction,
    js_string,
};
use boa_macros::{Finalize, JsData, Trace};

use crate::style::{Style, parse_color};
use crate::vdom::{NodeId, NodeKind, Tree};

/// JS-visible mutable handle to the host tree, the registered root,
/// and any future runtime state. Cheap to clone (`Rc<RefCell<...>>`
/// inside) and `JsData`-trackable so the Boa GC keeps callbacks
/// holding it alive.
#[derive(Default, Clone, Trace, Finalize, JsData)]
pub struct UiState {
    #[unsafe_ignore_trace]
    inner: Rc<RefCell<UiStateInner>>,
}

#[derive(Default)]
struct UiStateInner {
    tree: Tree,
    root: NodeId,
}

impl UiState {
    pub fn root(&self) -> NodeId {
        self.inner.borrow().root
    }

    pub fn with_tree<R>(&self, f: impl FnOnce(&Tree) -> R) -> R {
        f(&self.inner.borrow().tree)
    }

    pub fn with_tree_mut<R>(&self, f: impl FnOnce(&mut Tree) -> R) -> R {
        f(&mut self.inner.borrow_mut().tree)
    }

    fn set_root(&self, root: NodeId) {
        self.inner.borrow_mut().root = root;
    }
}

/// Build the `1fpga:gui` synthetic module and register it on `loader`.
/// Call once per Boa context, before evaluating the bundle.
pub fn register(loader: &MapModuleLoader, context: &mut Context) -> JsResult<()> {
    let exports = vec![
        (
            js_string!("createInstance"),
            NativeFunction::from_fn_ptr(create_instance),
        ),
        (
            js_string!("appendChild"),
            NativeFunction::from_fn_ptr(append_child),
        ),
        (
            js_string!("insertBefore"),
            NativeFunction::from_fn_ptr(insert_before),
        ),
        (
            js_string!("removeChild"),
            NativeFunction::from_fn_ptr(remove_child),
        ),
        (
            js_string!("commitUpdate"),
            NativeFunction::from_fn_ptr(commit_update),
        ),
        (
            js_string!("setStyle"),
            NativeFunction::from_fn_ptr(set_style),
        ),
        (
            js_string!("run"),
            NativeFunction::from_fn_ptr(run_app),
        ),
    ];
    let module = exports.into_js_module(context);
    loader.insert("1fpga:gui", module);
    Ok(())
}

// ===== Native functions =====

fn ui_state(context: &mut Context) -> JsResult<UiState> {
    context.get_data::<UiState>().cloned().ok_or_else(|| {
        JsError::from_native(JsNativeError::error().with_message("UiState not installed in Boa context"))
    })
}

fn create_instance(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let kind_str = args
        .get_or_undefined(0)
        .to_string(context)?
        .to_std_string_escaped();
    let kind = match kind_str.as_str() {
        "div" => NodeKind::Div,
        other => {
            return Err(JsError::from_native(
                JsNativeError::error().with_message(format!("unknown node type: {other}")),
            ));
        }
    };
    let style = parse_style_arg(args.get_or_undefined(1), context)?;
    let state = ui_state(context)?;
    let id = state.with_tree_mut(|t| t.create(kind, style));
    tracing::info!("gui.createInstance({kind_str}, style={:?}) -> {}", style, id.0);
    Ok(JsValue::from(id.0))
}

fn append_child(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let parent = NodeId(args.get_or_undefined(0).to_u32(context)?);
    let child = NodeId(args.get_or_undefined(1).to_u32(context)?);
    tracing::info!("gui.appendChild(parent={}, child={})", parent.0, child.0);
    let state = ui_state(context)?;
    state.with_tree_mut(|t| t.append_child(parent, child));
    Ok(JsValue::undefined())
}

fn insert_before(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let parent = NodeId(args.get_or_undefined(0).to_u32(context)?);
    let child = NodeId(args.get_or_undefined(1).to_u32(context)?);
    let before = NodeId(args.get_or_undefined(2).to_u32(context)?);
    let state = ui_state(context)?;
    state.with_tree_mut(|t| t.insert_before(parent, child, before));
    Ok(JsValue::undefined())
}

fn remove_child(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let parent = NodeId(args.get_or_undefined(0).to_u32(context)?);
    let child = NodeId(args.get_or_undefined(1).to_u32(context)?);
    let state = ui_state(context)?;
    // react-reconciler expects the subtree to be detached and its
    // resources released. We fully drop the node + descendants to
    // free vdom slots; if React later remounts via a fresh
    // createInstance, that's a new id.
    state.with_tree_mut(|t| {
        t.detach_child(parent, child);
        t.remove(child);
    });
    Ok(JsValue::undefined())
}

/// Replace the entire props of a node. Currently equivalent to
/// `setStyle` for the supported property surface; will diverge once
/// non-style props (event handlers, refs) land.
fn commit_update(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let id = NodeId(args.get_or_undefined(0).to_u32(context)?);
    let style = parse_style_arg(args.get_or_undefined(1), context)?;
    let state = ui_state(context)?;
    state.with_tree_mut(|t| t.set_style(id, style));
    Ok(JsValue::undefined())
}

fn set_style(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let id = NodeId(args.get_or_undefined(0).to_u32(context)?);
    // The second arg may be either a `{style: {...}}` props object
    // (matches createInstance) or a flat style object. Support flat
    // here for ergonomic `setStyle(id, { width: 100 })` calls.
    let style = parse_style_value(args.get_or_undefined(1), context)?;
    let state = ui_state(context)?;
    state.with_tree_mut(|t| t.set_style(id, style));
    Ok(JsValue::undefined())
}

fn run_app(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let id = NodeId(args.get_or_undefined(0).to_u32(context)?);
    tracing::info!("1fpga:gui.run(id={})", id.0);
    let state = ui_state(context)?;
    state.set_root(id);
    Ok(JsValue::undefined())
}

// ===== Style argument parsers =====

/// Decode a `props` object (`{ style: {...} }`) into a [`Style`].
fn parse_style_arg(value: &JsValue, context: &mut Context) -> JsResult<Style> {
    let Some(props) = value.as_object() else {
        return Ok(Style::default());
    };
    let style_v = props.get(js_string!("style"), context)?;
    parse_style_value(&style_v, context)
}

/// Decode a flat style object into a [`Style`].
fn parse_style_value(value: &JsValue, context: &mut Context) -> JsResult<Style> {
    let mut out = Style::default();
    let Some(style_obj) = value.as_object() else {
        return Ok(out);
    };

    let bg = style_obj.get(js_string!("backgroundColor"), context)?;
    if !bg.is_undefined() {
        let s = bg.to_string(context)?.to_std_string_escaped();
        out.background_color = parse_color(&s);
    }
    out.width = read_u16(&style_obj, "width", context)?;
    out.height = read_u16(&style_obj, "height", context)?;
    out.top = read_u16(&style_obj, "top", context)?;
    out.left = read_u16(&style_obj, "left", context)?;
    Ok(out)
}

fn read_u16(obj: &JsObject, key: &str, context: &mut Context) -> JsResult<Option<u16>> {
    let v = obj.get(JsString::from(key), context)?;
    if v.is_undefined() || v.is_null() {
        return Ok(None);
    }
    let n = v.to_u32(context)?;
    Ok(Some(n.min(u16::MAX as u32) as u16))
}
