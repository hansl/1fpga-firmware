use boa_engine::{js_string, Context, JsResult, JsString, Module};
use boa_macros::boa_module;

use std::cell::RefCell;
use std::marker::PhantomData;
use std::rc::Rc;

use boa_macros::{Finalize, JsData, Trace};
use ui_fb::{AnimationController, DomTree, LayoutEngine, Renderer};

/// Shared DOM state, inserted into the Boa context as ContextData.
#[derive(Clone, Trace, Finalize, JsData)]
pub struct DomState {
    #[unsafe_ignore_trace]
    inner: Rc<RefCell<DomStateInner>>,

    _marker: PhantomData<*mut ()>,
}

struct DomStateInner {
    tree: DomTree,
    layout: LayoutEngine,
    renderer: Renderer,
    animations: AnimationController,
    render_requested: bool,
    should_exit: bool,
    exit_value: Option<String>,
}

impl DomState {
    pub fn new() -> Self {
        Self {
            inner: Rc::new(RefCell::new(DomStateInner {
                tree: DomTree::new(),
                layout: LayoutEngine::new(),
                renderer: Renderer::new(),
                animations: AnimationController::new(),
                render_requested: false,
                should_exit: false,
                exit_value: None,
            })),
            _marker: PhantomData,
        }
    }
}

impl std::fmt::Debug for DomState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DomState").finish()
    }
}

#[boa_module]
#[boa(rename_all = "camelCase")]
mod js {
    use boa_engine::interop::ContextData;
    use boa_engine::object::builtins::{JsFunction, JsPromise};
    use boa_engine::{js_error, Context, JsResult, JsString, JsValue};
    use std::time::{Duration, Instant};
    use tracing::{debug, trace};
    use ui_fb::dom::{
        AlignItems, FlexDirection, JustifyContent, NodeType, PositionType,
    };
    use ui_fb::{AnimatedProperty, EasingFn, Tween};

    use super::DomState;
    use crate::AppRef;

    fn create_element(
        node_type: JsString,
        ContextData(state): ContextData<DomState>,
    ) -> JsResult<u32> {
        let ty = match node_type.to_std_string_lossy().as_str() {
            "view" => NodeType::View,
            "text" => NodeType::Text,
            "image" => NodeType::Image,
            other => return Err(js_error!("Unknown element type: {other}")),
        };
        let id = state.inner.borrow_mut().tree.create_node(ty);
        Ok(id)
    }

    fn create_text(text: JsString, ContextData(state): ContextData<DomState>) -> u32 {
        let mut inner = state.inner.borrow_mut();
        let id = inner.tree.create_node(NodeType::Text);
        inner.tree.set_text(id, text.to_std_string_lossy());
        id
    }

    fn append_child(parent: u32, child: u32, ContextData(state): ContextData<DomState>) {
        state.inner.borrow_mut().tree.append_child(parent, child);
    }

    fn insert_before(
        parent: u32,
        child: u32,
        before: u32,
        ContextData(state): ContextData<DomState>,
    ) {
        state
            .inner
            .borrow_mut()
            .tree
            .insert_before(parent, child, before);
    }

    fn remove_child(parent: u32, child: u32, ContextData(state): ContextData<DomState>) {
        state.inner.borrow_mut().tree.remove_child(parent, child);
    }

    fn remove_node(node_id: u32, ContextData(state): ContextData<DomState>) {
        state.inner.borrow_mut().tree.remove_node(node_id);
    }

    fn set_text_content(
        node_id: u32,
        text: JsString,
        ContextData(state): ContextData<DomState>,
    ) {
        state
            .inner
            .borrow_mut()
            .tree
            .set_text(node_id, text.to_std_string_lossy());
    }

    fn get_root_node(ContextData(state): ContextData<DomState>) -> u32 {
        state.inner.borrow().tree.root()
    }

    fn request_render(ContextData(state): ContextData<DomState>) {
        state.inner.borrow_mut().render_requested = true;
    }

    fn exit_render_loop(value: Option<JsString>, ContextData(state): ContextData<DomState>) {
        let mut inner = state.inner.borrow_mut();
        inner.should_exit = true;
        inner.exit_value = value.map(|s| s.to_std_string_lossy());
    }

    fn set_prop(
        node_id: u32,
        key: JsString,
        value: JsValue,
        ContextData(state): ContextData<DomState>,
        context: &mut Context,
    ) -> JsResult<()> {
        let key_str = key.to_std_string_lossy();
        let mut inner = state.inner.borrow_mut();

        inner.tree.update_style(node_id, |style| {
            match key_str.as_str() {
                "backgroundColor" => {
                    style.background_color = parse_color(&value, context);
                }
                "color" => {
                    style.color = parse_color(&value, context);
                }
                "width" => {
                    style.width = value.to_number(context).ok().map(|n| n as f32);
                }
                "height" => {
                    style.height = value.to_number(context).ok().map(|n| n as f32);
                }
                "flexDirection" => {
                    if let Some(s) = value.as_string() {
                        style.flex_direction = Some(match s.to_std_string_lossy().as_str() {
                            "row" => FlexDirection::Row,
                            _ => FlexDirection::Column,
                        });
                    }
                }
                "justifyContent" => {
                    if let Some(s) = value.as_string() {
                        style.justify_content = Some(match s.to_std_string_lossy().as_str() {
                            "center" => JustifyContent::Center,
                            "flex-end" => JustifyContent::FlexEnd,
                            "space-between" => JustifyContent::SpaceBetween,
                            "space-around" => JustifyContent::SpaceAround,
                            "space-evenly" => JustifyContent::SpaceEvenly,
                            _ => JustifyContent::FlexStart,
                        });
                    }
                }
                "alignItems" => {
                    if let Some(s) = value.as_string() {
                        style.align_items = Some(match s.to_std_string_lossy().as_str() {
                            "center" => AlignItems::Center,
                            "flex-end" => AlignItems::FlexEnd,
                            "flex-start" => AlignItems::FlexStart,
                            "baseline" => AlignItems::Baseline,
                            _ => AlignItems::Stretch,
                        });
                    }
                }
                "padding" => {
                    if let Ok(n) = value.to_number(context) {
                        let v = n as f32;
                        style.padding = [v, v, v, v];
                    }
                }
                "margin" => {
                    if let Ok(n) = value.to_number(context) {
                        let v = n as f32;
                        style.margin = [v, v, v, v];
                    }
                }
                "gap" => {
                    style.gap = value.to_number(context).ok().map(|n| n as f32);
                }
                "flexGrow" => {
                    style.flex_grow = value.to_number(context).ok().map(|n| n as f32);
                }
                "flexShrink" => {
                    style.flex_shrink = value.to_number(context).ok().map(|n| n as f32);
                }
                "opacity" => {
                    style.opacity = value.to_number(context).ok().map(|n| n as f32);
                }
                "fontSize" => {
                    style.font_size = value.to_number(context).ok().map(|n| n as u8);
                }
                "borderRadius" => {
                    style.border_radius = value.to_number(context).ok().map(|n| n as f32);
                }
                "position" => {
                    if let Some(s) = value.as_string() {
                        style.position_type = Some(match s.to_std_string_lossy().as_str() {
                            "absolute" => PositionType::Absolute,
                            _ => PositionType::Relative,
                        });
                    }
                }
                "top" => style.top = value.to_number(context).ok().map(|n| n as f32),
                "left" => style.left = value.to_number(context).ok().map(|n| n as f32),
                "right" => style.right = value.to_number(context).ok().map(|n| n as f32),
                "bottom" => style.bottom = value.to_number(context).ok().map(|n| n as f32),
                "src" => {
                    // Image source - set on node directly, not style
                }
                _ => {
                    trace!("Unknown style property: {key_str}");
                }
            }
        });

        // Handle non-style props
        if key_str == "src" {
            if let Some(s) = value.as_string() {
                inner
                    .tree
                    .set_image_src(node_id, s.to_std_string_lossy());
            }
        }

        Ok(())
    }

    fn animate(
        node_id: u32,
        options: JsValue,
        ContextData(state): ContextData<DomState>,
        context: &mut Context,
    ) -> JsResult<()> {
        let obj = options
            .as_object()
            .ok_or_else(|| js_error!("animate options must be an object"))?;

        let property_str = obj
            .get(boa_macros::js_str!("property"), context)?
            .as_string()
            .ok_or_else(|| js_error!("property must be a string"))?
            .to_std_string_lossy();
        let from = obj
            .get(boa_macros::js_str!("from"), context)?
            .to_number(context)? as f32;
        let to = obj
            .get(boa_macros::js_str!("to"), context)?
            .to_number(context)? as f32;
        let duration = obj
            .get(boa_macros::js_str!("duration"), context)?
            .to_number(context)? as u32;
        let easing_str = obj
            .get(boa_macros::js_str!("easing"), context)?
            .as_string()
            .map(|s| s.to_std_string_lossy())
            .unwrap_or_else(|| "linear".to_string());

        let property = match property_str.as_str() {
            "opacity" => AnimatedProperty::Opacity,
            "translateX" => AnimatedProperty::TranslateX,
            "translateY" => AnimatedProperty::TranslateY,
            "scale" => AnimatedProperty::Scale,
            _ => return Err(js_error!("Unknown animated property: {property_str}")),
        };

        let easing = match easing_str.as_str() {
            "easeIn" => EasingFn::EaseIn,
            "easeOut" => EasingFn::EaseOut,
            "easeInOut" => EasingFn::EaseInOut,
            _ => EasingFn::Linear,
        };

        let tween = Tween {
            property,
            from,
            to,
            duration_ms: duration,
            easing,
            start_time: Instant::now(),
        };

        state.inner.borrow_mut().animations.animate(node_id, tween);
        Ok(())
    }

    /// Enter the Rust-owned render loop. Blocks until exitRenderLoop is called.
    /// The `on_input` callback is called with input events (e.g., "up", "down", "select", "back").
    fn start_render_loop(
        on_input: JsFunction,
        ContextData(state): ContextData<DomState>,
        ContextData(mut app): ContextData<AppRef>,
        context: &mut Context,
    ) -> JsResult<JsPromise> {
        use embedded_graphics_framebuf::FrameBuf;

        let target_frame_time = Duration::from_millis(16); // ~60fps

        debug!("Starting DOM render loop");

        // Open the framebuffer
        let mut fb = ui_fb::DoubleBufferedFramebuffer::new("/dev/fb0")
            .map_err(|e| js_error!("Failed to open framebuffer: {e}"))?;
        let fb_size = fb.size();
        let vw = fb_size.width;
        let vh = fb_size.height;

        debug!(?vw, ?vh, "Framebuffer opened");

        // Debug: count nodes in the tree
        {
            let inner = state.inner.borrow();
            let root = inner.tree.root();
            let root_node = inner.tree.get(root);
            let child_count = root_node.map(|n| n.children.len()).unwrap_or(0);
            eprintln!("[DOM] Root has {child_count} children");
            if let Some(root_node) = root_node {
                for (i, &child_id) in root_node.children.iter().enumerate() {
                    if let Some(child) = inner.tree.get(child_id) {
                        eprintln!("[DOM]   child {i}: {:?} children={} text={:?} src={:?}",
                            child.node_type, child.children.len(), child.text_content, child.image_src);
                    }
                }
            }
        }

        // Initial layout
        {
            let mut inner = state.inner.borrow_mut();
            let tree = &inner.tree as *const ui_fb::DomTree;
            // SAFETY: layout.compute only reads the tree.
            inner.layout.compute(unsafe { &*tree }, vw, vh);
            inner.tree.mark_clean();
            inner.render_requested = true;
        }

        eprintln!("[DOM] Entering render loop, render_requested=true");

        loop {
            let frame_start = Instant::now();

            // 1. Poll input events from SDL
            let events = app.platform_mut().events();
            for event in &events {
                if let Some(input) = translate_event(event) {
                    let input_str = JsString::from(input);
                    let _ = on_input.call(
                        &JsValue::undefined(),
                        &[JsValue::from(input_str)],
                        context,
                    );
                }
            }

            // 2. Run JS microtasks (React batched updates)
            let _ = context.run_jobs();

            // 3. Tick animations
            let animating = state.inner.borrow_mut().animations.tick();

            // 4. Re-layout if dirty
            {
                let mut inner = state.inner.borrow_mut();
                if inner.tree.is_dirty() || animating {
                    let tree = &inner.tree as *const ui_fb::DomTree;
                    // SAFETY: layout.compute only reads the tree.
                    inner.layout.compute(unsafe { &*tree }, vw, vh);
                    inner.tree.mark_clean();
                    inner.render_requested = true;
                }
            }

            // 5. Render to framebuffer if needed
            {
                let mut inner = state.inner.borrow_mut();
                if inner.render_requested {
                    eprintln!("[DOM] Rendering frame to {}x{} framebuffer", vw, vh);
                    let tree = &inner.tree as *const ui_fb::DomTree;
                    let layout = &inner.layout as *const ui_fb::LayoutEngine;
                    let animations = &inner.animations as *const ui_fb::AnimationController;
                    // SAFETY: renderer.render only reads tree/layout/animations,
                    // and writes to fb (not part of inner).
                    let mut frame_buf = FrameBuf::new(
                        &mut fb,
                        vw as usize,
                        vh as usize,
                    );
                    inner.renderer.render(
                        unsafe { &*tree },
                        unsafe { &*layout },
                        unsafe { &*animations },
                        &mut frame_buf,
                    );
                    drop(frame_buf);
                    fb.flip();
                    inner.render_requested = false;
                }
            }

            // 6. Check exit condition
            if state.inner.borrow().should_exit {
                break;
            }

            // 7. Frame timing
            let elapsed = frame_start.elapsed();
            if elapsed < target_frame_time {
                std::thread::sleep(target_frame_time - elapsed);
            }
        }

        let exit_value = state
            .inner
            .borrow()
            .exit_value
            .clone()
            .unwrap_or_default();

        debug!(?exit_value, "DOM render loop exited");

        Ok(JsPromise::resolve(
            JsValue::from(JsString::from(exit_value.as_str())),
            context,
        ))
    }

    #[boa(skip)]
    fn translate_event(event: &sdl3::event::Event) -> Option<&'static str> {
        use sdl3::keyboard::Scancode;
        match event {
            sdl3::event::Event::KeyDown {
                scancode: Some(sc), ..
            } => match *sc {
                Scancode::Up => Some("up"),
                Scancode::Down => Some("down"),
                Scancode::Return => Some("select"),
                Scancode::Escape => Some("back"),
                Scancode::Left => Some("left"),
                Scancode::Right => Some("right"),
                _ => None,
            },
            _ => None,
        }
    }

    #[boa(skip)]
    fn parse_color(value: &JsValue, context: &mut Context) -> Option<ui_fb::dom::Rgb888> {
        use ui_fb::dom::Rgb888;
        // Accept [r, g, b] array or "#RRGGBB" string
        if let Some(s) = value.as_string() {
            let s = s.to_std_string_lossy();
            if s.starts_with('#') && s.len() == 7 {
                let r = u8::from_str_radix(&s[1..3], 16).ok()?;
                let g = u8::from_str_radix(&s[3..5], 16).ok()?;
                let b = u8::from_str_radix(&s[5..7], 16).ok()?;
                return Some(Rgb888::new(r, g, b));
            }
        }
        if let Some(obj) = value.as_object() {
            if let Ok(r) = obj.get(0, context) {
                if let Ok(g) = obj.get(1, context) {
                    if let Ok(b) = obj.get(2, context) {
                        let r = r.to_number(context).unwrap_or(0.0) as u8;
                        let g = g.to_number(context).unwrap_or(0.0) as u8;
                        let b = b.to_number(context).unwrap_or(0.0) as u8;
                        return Some(Rgb888::new(r, g, b));
                    }
                }
            }
        }
        None
    }
}

pub fn create_module(context: &mut Context) -> JsResult<(JsString, Module)> {
    Ok((js_string!("dom"), js::boa_module(None, context)))
}
