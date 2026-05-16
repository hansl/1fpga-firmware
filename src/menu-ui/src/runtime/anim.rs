//! Host-driven tween manager.
//!
//! JS calls `1fpga:gui.startTween(nodeId, target, opts)` and the
//! manager records a [`Tween`] keyed by `(NodeId, TweenProp)`. Once
//! per loop iteration the runtime calls [`AnimationManager::tick`] —
//! which advances every active tween, applies the interpolated value
//! via the existing `Style::merge_from` fast path, and removes the
//! tween once `t >= 1`. The damage system then picks up the style
//! change naturally on the next `compute_scene`.
//!
//! Why a fast path: doing the interpolation in Rust avoids re-running
//! React + the reconciler per tween tick. Each step costs only a
//! tree lookup + a `Style::merge_from` + (later) a damage rect
//! invalidation — ~10s of microseconds per active tween.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use boa_macros::{Finalize, JsData, Trace};

use crate::host::UiState;
use crate::style::Style;
use crate::vdom::NodeId;

/// Properties that can be animated. The set is intentionally small —
/// each new entry requires (a) a way to read the current value off a
/// `Style`, (b) a way to write the interpolated value back via
/// `Style::merge_from`. Adding a property is mechanical; we only
/// expose what's been tested.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub enum TweenProp {
    Opacity,
}

impl TweenProp {
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "opacity" => Some(Self::Opacity),
            _ => None,
        }
    }

    /// Read the property's current value off a `Style`. Falls back to
    /// the property's CSS default when unset, so a tween that starts
    /// before a style is committed still has a sensible `from`.
    fn read(self, style: &Style) -> f32 {
        match self {
            Self::Opacity => style.opacity.unwrap_or(1.0),
        }
    }

    /// Build a partial `Style` that, when merged onto the node's
    /// current style, overrides only this property.
    fn write(self, value: f32) -> Style {
        let mut patch = Style::default();
        match self {
            Self::Opacity => patch.opacity = Some(value.clamp(0.0, 1.0)),
        }
        patch
    }
}

/// Easing functions. Linear is the default; the named curves are the
/// usual CSS-spec quadratic/cubic shapes. Adding new ones is a single
/// match arm in [`Easing::apply`].
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Easing {
    Linear,
    EaseIn,
    EaseOut,
    EaseInOut,
}

impl Easing {
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "linear" => Some(Self::Linear),
            "ease-in" | "easeIn" => Some(Self::EaseIn),
            "ease-out" | "easeOut" => Some(Self::EaseOut),
            "ease-in-out" | "easeInOut" => Some(Self::EaseInOut),
            _ => None,
        }
    }

    /// Map a linear `t ∈ [0, 1]` to the eased parameter.
    pub fn apply(self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Self::Linear => t,
            Self::EaseIn => t * t,
            Self::EaseOut => {
                let u = 1.0 - t;
                1.0 - u * u
            }
            Self::EaseInOut => {
                if t < 0.5 {
                    2.0 * t * t
                } else {
                    let u = -2.0 * t + 2.0;
                    1.0 - u * u / 2.0
                }
            }
        }
    }
}

/// One in-flight tween for a (node, property) pair.
#[derive(Debug)]
struct Tween {
    from: f32,
    to: f32,
    start: Instant,
    duration: Duration,
    easing: Easing,
}

impl Tween {
    /// Returns `(current_value, done)`. `done == true` means we've
    /// reached or passed the end of the duration and the tween can
    /// be retired.
    fn sample(&self, now: Instant) -> (f32, bool) {
        let elapsed = now.duration_since(self.start);
        if elapsed >= self.duration {
            (self.to, true)
        } else {
            let t = elapsed.as_secs_f32() / self.duration.as_secs_f32();
            let eased = self.easing.apply(t);
            (self.from + (self.to - self.from) * eased, false)
        }
    }
}

#[derive(Default, Clone, Trace, Finalize, JsData)]
pub struct AnimationManager {
    #[unsafe_ignore_trace]
    inner: Rc<RefCell<AnimInner>>,
}

#[derive(Default)]
struct AnimInner {
    tweens: HashMap<(NodeId, TweenProp), Tween>,
}

impl AnimationManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Start (or replace) a tween. The new tween's `from` is the
    /// node's *current* value for that property — including any
    /// in-progress tween's interpolated position — so retargeting
    /// mid-animation glides smoothly from wherever the tween was
    /// instead of snapping to the React-committed value.
    pub fn start(
        &self,
        ui_state: &UiState,
        node_id: NodeId,
        prop: TweenProp,
        to: f32,
        duration: Duration,
        easing: Easing,
    ) {
        // Look up the current visible value for `prop` on `node_id`.
        let from = ui_state.with_tree(|t| {
            t.get(node_id).map(|n| prop.read(&n.style)).unwrap_or(0.0)
        });
        if duration.is_zero() {
            // Degenerate — just apply the target and skip the tween.
            apply(ui_state, node_id, prop, to);
            self.inner.borrow_mut().tweens.remove(&(node_id, prop));
            return;
        }
        let tween = Tween {
            from,
            to,
            start: Instant::now(),
            duration,
            easing,
        };
        self.inner
            .borrow_mut()
            .tweens
            .insert((node_id, prop), tween);
    }

    /// Advance every active tween. Returns the number of tweens still
    /// running after this tick — caller can use that to short-circuit
    /// the "skip submit" fast path (a running tween means the scene
    /// changes every iteration even if nothing else moved).
    pub fn tick(&self, ui_state: &UiState) -> usize {
        let now = Instant::now();
        let mut inner = self.inner.borrow_mut();
        let mut to_remove: Vec<(NodeId, TweenProp)> = Vec::new();
        for (&(node_id, prop), tween) in inner.tweens.iter() {
            let (value, done) = tween.sample(now);
            apply(ui_state, node_id, prop, value);
            if done {
                to_remove.push((node_id, prop));
            }
        }
        for key in to_remove {
            inner.tweens.remove(&key);
        }
        inner.tweens.len()
    }
}

/// Apply a single property override to a node's style via the same
/// merge path JS's `gui.updateStyle` uses.
fn apply(ui_state: &UiState, node_id: NodeId, prop: TweenProp, value: f32) {
    let patch = prop.write(value);
    ui_state.with_tree_mut(|t| {
        if let Some(node) = t.get(node_id) {
            let mut merged = node.style.clone();
            merged.merge_from(&patch);
            t.set_style(node_id, merged);
        }
    });
}
