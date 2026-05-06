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
use boa_engine::object::builtins::JsFunction;
use boa_engine::{
    Context, JsArgs, JsError, JsNativeError, JsObject, JsResult, JsString, JsValue, NativeFunction,
    js_string,
};
use boa_macros::{Finalize, JsData, Trace};

use crate::input::events::InputSource;
use crate::input::state::{InputState, ListenerId, ListenerKind, ListenerScope};
use crate::style::{
    Style, parse_align_items, parse_color, parse_display, parse_flex_direction, parse_flex_wrap,
    parse_justify_content, parse_overflow, parse_position,
};
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
            js_string!("createTextInstance"),
            NativeFunction::from_fn_ptr(create_text_instance),
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
            js_string!("commitTextUpdate"),
            NativeFunction::from_fn_ptr(commit_text_update),
        ),
        (
            js_string!("setStyle"),
            NativeFunction::from_fn_ptr(set_style),
        ),
        (
            js_string!("addIntentListener"),
            NativeFunction::from_fn_ptr(add_intent_listener),
        ),
        (
            js_string!("addRawInputListener"),
            NativeFunction::from_fn_ptr(add_raw_input_listener),
        ),
        (
            js_string!("removeListener"),
            NativeFunction::from_fn_ptr(remove_listener),
        ),
        (
            js_string!("pushFocus"),
            NativeFunction::from_fn_ptr(push_focus),
        ),
        (
            js_string!("popFocus"),
            NativeFunction::from_fn_ptr(pop_focus),
        ),
        (
            js_string!("setFocus"),
            NativeFunction::from_fn_ptr(set_focus_host),
        ),
        (
            js_string!("getFocus"),
            NativeFunction::from_fn_ptr(get_focus),
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
    let props = args.get_or_undefined(1);
    let style = parse_style_arg(props, context)?;
    let kind = match kind_str.as_str() {
        "div" => NodeKind::Div,
        "img" => {
            let src = read_str_prop(props, "src", context)?.unwrap_or_default();
            NodeKind::Img { src }
        }
        other => {
            return Err(JsError::from_native(
                JsNativeError::error().with_message(format!("unknown node type: {other}")),
            ));
        }
    };
    let state = ui_state(context)?;
    let id = state.with_tree_mut(|t| t.create(kind, style.clone()));
    tracing::debug!(?style, "gui.createInstance({kind_str}) -> {}", id.0);
    Ok(JsValue::from(id.0))
}

/// Read a top-level string-typed prop (e.g. `src` on `<img>`) from a
/// `props` object. `None` if missing / undefined.
fn read_str_prop(value: &JsValue, key: &str, context: &mut Context) -> JsResult<Option<String>> {
    let Some(props) = value.as_object() else {
        return Ok(None);
    };
    let v = props.get(JsString::from(key), context)?;
    if v.is_undefined() || v.is_null() {
        return Ok(None);
    }
    Ok(Some(v.to_string(context)?.to_std_string_escaped()))
}

fn create_text_instance(
    _this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let text = args
        .get_or_undefined(0)
        .to_string(context)?
        .to_std_string_escaped();
    let state = ui_state(context)?;
    let id = state.with_tree_mut(|t| t.create(NodeKind::Text { content: text.clone() }, Style::default()));
    tracing::debug!("gui.createTextInstance({text:?}) -> {}", id.0);
    Ok(JsValue::from(id.0))
}

fn commit_text_update(
    _this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let id = NodeId(args.get_or_undefined(0).to_u32(context)?);
    let text = args
        .get_or_undefined(1)
        .to_string(context)?
        .to_std_string_escaped();
    let state = ui_state(context)?;
    state.with_tree_mut(|t| t.set_text(id, text));
    Ok(JsValue::undefined())
}

fn append_child(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let parent = NodeId(args.get_or_undefined(0).to_u32(context)?);
    let child = NodeId(args.get_or_undefined(1).to_u32(context)?);
    tracing::debug!("gui.appendChild(parent={}, child={})", parent.0, child.0);
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

/// Replace the entire props of a node. Updates style + element-
/// specific props (e.g. `<img src=...>`).
fn commit_update(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let id = NodeId(args.get_or_undefined(0).to_u32(context)?);
    let props = args.get_or_undefined(1);
    let style = parse_style_arg(props, context)?;
    let new_src = read_str_prop(props, "src", context)?;
    let state = ui_state(context)?;
    state.with_tree_mut(|t| {
        t.set_style(id, style);
        if let Some(src) = new_src {
            t.set_img_src(id, src);
        }
    });
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
    tracing::debug!("1fpga:gui.run(id={})", id.0);
    let state = ui_state(context)?;
    state.set_root(id);
    Ok(JsValue::undefined())
}

// ===== Input listeners + focus =====

fn input_state(context: &mut Context) -> JsResult<InputState> {
    context.get_data::<InputState>().cloned().ok_or_else(|| {
        JsError::from_native(
            JsNativeError::error().with_message("InputState not installed in Boa context"),
        )
    })
}

/// `addIntentListener(name, handler, opts?) -> id`. `opts` may
/// contain `{ global: true }` (default false) and `{ nodeId: u32 }`
/// (defaults to the current focus, or "any focus" if none set).
fn add_intent_listener(
    _this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let name = args
        .get_or_undefined(0)
        .to_string(context)?
        .to_std_string_escaped();
    let handler = args
        .get_or_undefined(1)
        .as_object()
        .and_then(|o| JsFunction::from_object(o.clone()))
        .ok_or_else(|| {
            JsError::from_native(JsNativeError::typ().with_message("expected handler function"))
        })?;
    let scope = parse_scope(args.get_or_undefined(2), context)?;
    let state = input_state(context)?;
    let id = state.add(ListenerKind::Intent { name }, scope, handler);
    Ok(JsValue::from(id.0))
}

/// `addRawInputListener(source, handler, opts?) -> id`. `source` is
/// `'keyboard' | 'gamepad' | 'mouse'`.
fn add_raw_input_listener(
    _this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let source_str = args
        .get_or_undefined(0)
        .to_string(context)?
        .to_std_string_escaped();
    let source = match source_str.as_str() {
        "keyboard" => InputSource::Keyboard,
        "gamepad" => InputSource::Gamepad,
        "mouse" => InputSource::Mouse,
        other => {
            return Err(JsError::from_native(
                JsNativeError::error()
                    .with_message(format!("unknown raw input source: {other}")),
            ));
        }
    };
    let handler = args
        .get_or_undefined(1)
        .as_object()
        .and_then(|o| JsFunction::from_object(o.clone()))
        .ok_or_else(|| {
            JsError::from_native(JsNativeError::typ().with_message("expected handler function"))
        })?;
    let scope = parse_scope(args.get_or_undefined(2), context)?;
    let state = input_state(context)?;
    let id = state.add(ListenerKind::Raw { source }, scope, handler);
    Ok(JsValue::from(id.0))
}

fn remove_listener(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let id = ListenerId(args.get_or_undefined(0).to_u32(context)?);
    let state = input_state(context)?;
    Ok(JsValue::from(state.remove(id)))
}

fn push_focus(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let id = args.get_or_undefined(0).to_u32(context)?;
    input_state(context)?.push_focus(id);
    Ok(JsValue::undefined())
}

fn pop_focus(_this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let popped = input_state(context)?.pop_focus();
    Ok(popped.map(JsValue::from).unwrap_or(JsValue::null()))
}

fn set_focus_host(
    _this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let id = args.get_or_undefined(0).to_u32(context)?;
    input_state(context)?.set_focus(id);
    Ok(JsValue::undefined())
}

fn get_focus(_this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let f = input_state(context)?.focus();
    Ok(f.map(JsValue::from).unwrap_or(JsValue::null()))
}

/// Parse the optional `opts` object passed to `add*Listener`. Default
/// scope is `Global` (so handlers fire regardless of focus); pass
/// `{ global: false, nodeId: <id> }` to scope to a specific subtree.
fn parse_scope(value: &JsValue, context: &mut Context) -> JsResult<ListenerScope> {
    let Some(opts) = value.as_object() else {
        return Ok(ListenerScope::Global);
    };
    let global_v = opts.get(js_string!("global"), context)?;
    let global = global_v.is_undefined() || global_v.to_boolean();
    if global {
        return Ok(ListenerScope::Global);
    }
    let node_v = opts.get(js_string!("nodeId"), context)?;
    if node_v.is_undefined() || node_v.is_null() {
        return Ok(ListenerScope::Global);
    }
    Ok(ListenerScope::Focused {
        node_id: node_v.to_u32(context)?,
    })
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

/// Decode a flat style object into a [`Style`]. Tolerant: any field
/// absent / `undefined` / unknown-keyword leaves the corresponding
/// `Style` slot as `None`.
fn parse_style_value(value: &JsValue, context: &mut Context) -> JsResult<Style> {
    let mut out = Style::default();
    let Some(o) = value.as_object() else {
        return Ok(out);
    };

    out.display = read_keyword(&o, "display", context, parse_display)?;
    out.position = read_keyword(&o, "position", context, parse_position)?;
    out.flex_direction = read_keyword(&o, "flexDirection", context, parse_flex_direction)?;
    out.flex_wrap = read_keyword(&o, "flexWrap", context, parse_flex_wrap)?;
    out.justify_content = read_keyword(&o, "justifyContent", context, parse_justify_content)?;
    out.align_items = read_keyword(&o, "alignItems", context, parse_align_items)?;
    out.align_self = read_keyword(&o, "alignSelf", context, parse_align_items)?;
    out.overflow = read_keyword(&o, "overflow", context, parse_overflow)?;

    out.flex_grow = read_f32(&o, "flexGrow", context)?;
    out.flex_shrink = read_f32(&o, "flexShrink", context)?;
    out.flex_basis = read_f32(&o, "flexBasis", context)?;
    out.gap = read_f32(&o, "gap", context)?;

    out.top = read_f32(&o, "top", context)?;
    out.right = read_f32(&o, "right", context)?;
    out.bottom = read_f32(&o, "bottom", context)?;
    out.left = read_f32(&o, "left", context)?;

    out.width = read_f32(&o, "width", context)?;
    out.height = read_f32(&o, "height", context)?;
    out.min_width = read_f32(&o, "minWidth", context)?;
    out.max_width = read_f32(&o, "maxWidth", context)?;
    out.min_height = read_f32(&o, "minHeight", context)?;
    out.max_height = read_f32(&o, "maxHeight", context)?;

    out.padding_top = read_f32(&o, "paddingTop", context)?;
    out.padding_right = read_f32(&o, "paddingRight", context)?;
    out.padding_bottom = read_f32(&o, "paddingBottom", context)?;
    out.padding_left = read_f32(&o, "paddingLeft", context)?;
    if let Some(p) = read_f32(&o, "padding", context)? {
        out.padding_top.get_or_insert(p);
        out.padding_right.get_or_insert(p);
        out.padding_bottom.get_or_insert(p);
        out.padding_left.get_or_insert(p);
    }
    out.margin_top = read_f32(&o, "marginTop", context)?;
    out.margin_right = read_f32(&o, "marginRight", context)?;
    out.margin_bottom = read_f32(&o, "marginBottom", context)?;
    out.margin_left = read_f32(&o, "marginLeft", context)?;
    if let Some(m) = read_f32(&o, "margin", context)? {
        out.margin_top.get_or_insert(m);
        out.margin_right.get_or_insert(m);
        out.margin_bottom.get_or_insert(m);
        out.margin_left.get_or_insert(m);
    }

    let bg = o.get(js_string!("backgroundColor"), context)?;
    if !bg.is_undefined() {
        out.background_color = parse_color(&bg.to_string(context)?.to_std_string_escaped());
    }
    let color = o.get(js_string!("color"), context)?;
    if !color.is_undefined() {
        out.color = parse_color(&color.to_string(context)?.to_std_string_escaped());
    }
    out.opacity = read_f32(&o, "opacity", context)?;

    let ff = o.get(js_string!("fontFamily"), context)?;
    if !ff.is_undefined() {
        out.font_family = Some(ff.to_string(context)?.to_std_string_escaped());
    }
    out.font_size = read_f32(&o, "fontSize", context)?;

    Ok(out)
}

fn read_f32(obj: &JsObject, key: &str, context: &mut Context) -> JsResult<Option<f32>> {
    let v = obj.get(JsString::from(key), context)?;
    if v.is_undefined() || v.is_null() {
        return Ok(None);
    }
    Ok(Some(v.to_number(context)? as f32))
}

fn read_keyword<T>(
    obj: &JsObject,
    key: &str,
    context: &mut Context,
    parse: fn(&str) -> Option<T>,
) -> JsResult<Option<T>> {
    let v = obj.get(JsString::from(key), context)?;
    if v.is_undefined() || v.is_null() {
        return Ok(None);
    }
    let s = v.to_string(context)?.to_std_string_escaped();
    Ok(parse(&s))
}
