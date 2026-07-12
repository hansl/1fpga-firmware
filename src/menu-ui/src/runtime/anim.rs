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
    ScaleX,
    ScaleY,
    Rotate,
    TranslateX,
    TranslateY,
    Top,
    Right,
    Bottom,
    Left,
}

impl TweenProp {
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "opacity" => Some(Self::Opacity),
            "scaleX" => Some(Self::ScaleX),
            "scaleY" => Some(Self::ScaleY),
            "rotate" => Some(Self::Rotate),
            "translateX" => Some(Self::TranslateX),
            "translateY" => Some(Self::TranslateY),
            "top" => Some(Self::Top),
            "right" => Some(Self::Right),
            "bottom" => Some(Self::Bottom),
            "left" => Some(Self::Left),
            _ => None,
        }
    }

    /// Read the property's current value off a `Style`. Falls back to
    /// the property's CSS default when unset, so a tween that starts
    /// before a style is committed still has a sensible `from`.
    fn read(self, style: &Style) -> f32 {
        match self {
            Self::Opacity => style.opacity.unwrap_or(1.0),
            Self::ScaleX => style.scale_x.unwrap_or(1.0),
            Self::ScaleY => style.scale_y.unwrap_or(1.0),
            Self::Rotate => style.rotate.unwrap_or(0.0),
            // Translation offsets default to 0 (no shift).
            Self::TranslateX => style.translate_x.unwrap_or(0.0),
            Self::TranslateY => style.translate_y.unwrap_or(0.0),
            // Position offsets: 0 is a sensible "no offset" default
            // for the from-value when the style hasn't committed yet.
            Self::Top => style.top.unwrap_or(0.0),
            Self::Right => style.right.unwrap_or(0.0),
            Self::Bottom => style.bottom.unwrap_or(0.0),
            Self::Left => style.left.unwrap_or(0.0),
        }
    }

    /// Build a partial `Style` that, when merged onto the node's
    /// current style, overrides only this property.
    fn write(self, value: f32) -> Style {
        let mut patch = Style::default();
        match self {
            Self::Opacity => patch.opacity = Some(value.clamp(0.0, 1.0)),
            // Scale clamped to a non-negative range; negative scales
            // would flip the image, which neither the FPGA renderer
            // nor the layout system handles today.
            Self::ScaleX => patch.scale_x = Some(value.max(0.0)),
            Self::ScaleY => patch.scale_y = Some(value.max(0.0)),
            // Rotation in degrees; any value is legal (it wraps).
            Self::Rotate => patch.rotate = Some(value),
            // Translation in pixels; any value is legal (signed offset).
            Self::TranslateX => patch.translate_x = Some(value),
            Self::TranslateY => patch.translate_y = Some(value),
            // Position offsets: pass through unchanged. Negative
            // values are CSS-legal (drag an element off-screen).
            Self::Top => patch.top = Some(value),
            Self::Right => patch.right = Some(value),
            Self::Bottom => patch.bottom = Some(value),
            Self::Left => patch.left = Some(value),
        }
        patch
    }

    /// Whether animating this prop affects LAYOUT (position offsets feed
    /// Taffy) vs only paint (opacity/scale/rotate/translate are applied
    /// at paint). Paint-only tweens let the runtime reuse the cached
    /// layout — `translateX/Y` is the cheap way to slide an element or
    /// subtree without reflowing, unlike `left/top` which do.
    fn is_layout(self) -> bool {
        matches!(self, Self::Top | Self::Right | Self::Bottom | Self::Left)
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
    /// Rate-based exponential follower, not a curve: the value chases
    /// the target with velocity proportional to the remaining
    /// distance (`duration` acts as the time constant), and a
    /// re-target UPDATES THE TARGET IN PLACE instead of restarting.
    /// This is the smooth-under-rapid-retarget mode: a duration-based
    /// ease re-launched every key repeat either lags unboundedly or
    /// (with snapBeyond) advances in visible jumps; a follower's
    /// velocity is a continuous function of lag, so held-scroll
    /// motion is steady and release settles without a discontinuity.
    Follow,
}

impl Easing {
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "linear" => Some(Self::Linear),
            "ease-in" | "easeIn" => Some(Self::EaseIn),
            "ease-out" | "easeOut" => Some(Self::EaseOut),
            "ease-in-out" | "easeInOut" => Some(Self::EaseInOut),
            "follow" => Some(Self::Follow),
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
            // Follow never samples a curve (Tween::advance handles it
            // statefully); linear is a harmless fallback if it does.
            Self::Follow => t,
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
    /// Follow-mode state: the value as of `last` (advanced per tick).
    /// Unused for timed easings (they are stateless in elapsed time).
    value: f32,
    last: Instant,
}

impl Tween {
    /// Advance to `now` and return `(current_value, done)`.
    /// `eps` is the per-property done threshold: below it the value
    /// snaps to the target and the tween retires. It must sit UNDER
    /// the property's paint quantum (1/255 for opacity, ~half a pixel
    /// for offsets) so the final snap is invisible — but not so far
    /// under that the follower's exponential tail churns the scene
    /// hash for a second producing no visible change.
    fn advance(&mut self, now: Instant, eps: f32) -> (f32, bool) {
        if matches!(self.easing, Easing::Follow) {
            // Exponential approach: value += (to - value) * (1 - e^(-dt/tau)).
            let dt = now.duration_since(self.last).as_secs_f32();
            self.last = now;
            let tau = self.duration.as_secs_f32().max(0.001);
            let k = 1.0 - (-dt / tau).exp();
            self.value += (self.to - self.value) * k;
            if (self.to - self.value).abs() < eps {
                self.value = self.to;
                return (self.to, true);
            }
            return (self.value, false);
        }
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
    /// Last interpolated value applied per (node, property). Persists
    /// across tween completions and React commits, so `start` can use
    /// it as the new tween's `from` instead of reading `style.x` —
    /// which by the time `useLayoutEffect` runs has already been
    /// overwritten with the new target by `commitUpdate`. Without this
    /// cache, every tween's `from` equals its `to` and no animation
    /// happens (the value just snaps).
    last_values: HashMap<(NodeId, TweenProp), f32>,
}

impl AnimationManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Start (or replace) a tween. The new tween's `from` is the
    /// last value we *applied* (cached in `last_values`), which
    /// survives React's `commitUpdate` clobbering `style.x` with the
    /// new target. First-ever tween for a (node, prop) reads
    /// `style.x` directly (good enough — there's nothing to animate
    /// from yet).
    #[allow(clippy::too_many_arguments)]
    pub fn start(
        &self,
        ui_state: &UiState,
        node_id: NodeId,
        prop: TweenProp,
        to: f32,
        duration: Duration,
        easing: Easing,
        snap_beyond: Option<f32>,
    ) {
        let mut inner = self.inner.borrow_mut();
        // Follow-mode re-target: update the target IN PLACE — the
        // follower keeps its current value and velocity profile, so
        // rapid re-targets (held key-repeat) produce continuous
        // motion instead of restarted glides.
        if matches!(easing, Easing::Follow)
            && let Some(t) = inner.tweens.get_mut(&(node_id, prop))
            && matches!(t.easing, Easing::Follow)
        {
            t.to = to;
            t.duration = duration;
            if let Some(sb) = snap_beyond
                && sb > 0.0
                && (to - t.value).abs() > sb
            {
                t.value = if to > t.value { to - sb } else { to + sb };
            }
            return;
        }
        let mut from = match inner.last_values.get(&(node_id, prop)).copied() {
            Some(v) => v,
            None => ui_state.with_tree(|t| {
                t.get(node_id).map(|n| prop.read(&n.style)).unwrap_or(0.0)
            }),
        };
        // Bounded re-target lag. A re-targeted ease glides FROM the
        // current value, so under held key-repeat the value covers
        // only a fraction of each step and falls arbitrarily far
        // behind the target — the carousel strip visibly trailed the
        // selection off-screen, then caught up in one big slide when
        // input stopped. `snapBeyond` caps the glide distance: when
        // the retarget leaves `from` further than this from `to`, snap
        // to that distance and glide the rest. The animation stays
        // smooth per step but can never lag more than the cap.
        // (Follow mode self-limits lag — velocity is proportional to
        // it — but the cap still bounds the worst case.)
        if let Some(sb) = snap_beyond
            && sb > 0.0
            && (to - from).abs() > sb
        {
            from = if to > from { to - sb } else { to + sb };
        }
        if duration.is_zero() || (from - to).abs() < f32::EPSILON {
            // Degenerate (zero duration) or no-op (same value): just
            // apply and skip the tween. Still record `to` as the
            // last_value so future `start`s see the right baseline.
            drop(inner);
            apply(ui_state, node_id, prop, to);
            let mut inner = self.inner.borrow_mut();
            inner.tweens.remove(&(node_id, prop));
            inner.last_values.insert((node_id, prop), to);
            return;
        }
        let now = Instant::now();
        let tween = Tween {
            from,
            to,
            start: now,
            duration,
            easing,
            value: from,
            last: now,
        };
        inner.tweens.insert((node_id, prop), tween);
    }

    /// Advance every active tween. Returns the number of tweens still
    /// running after this tick — caller can use that to short-circuit
    /// the "skip submit" fast path (a running tween means the scene
    /// changes every iteration even if nothing else moved).
    pub fn tick(&self, ui_state: &UiState) -> usize {
        let now = Instant::now();
        // Snapshot the tweens first so we can apply (which borrows
        // ui_state) outside the inner borrow. Each sample produces
        // both the value to write and the value to cache as
        // `last_values` for the next tween that targets the same key.
        let samples: Vec<((NodeId, TweenProp), f32, bool)> = {
            let mut inner = self.inner.borrow_mut();
            inner
                .tweens
                .iter_mut()
                .map(|(&key, tween)| {
                    // Done threshold under the property's paint
                    // quantum: opacity/scale quantise at ~1/255,
                    // offsets at whole pixels.
                    let eps = match key.1 {
                        TweenProp::Opacity | TweenProp::ScaleX | TweenProp::ScaleY => 0.003,
                        _ => 0.4,
                    };
                    let (value, done) = tween.advance(now, eps);
                    (key, value, done)
                })
                .collect()
        };
        for &(key, value, _done) in &samples {
            apply(ui_state, key.0, key.1, value);
        }
        let mut inner = self.inner.borrow_mut();
        for &(key, value, done) in &samples {
            inner.last_values.insert(key, value);
            if done {
                inner.tweens.remove(&key);
            }
        }
        inner.tweens.len()
    }
}

/// Apply a single property override to a node's style via the same
/// merge path JS's `gui.updateStyle` uses.
fn apply(ui_state: &UiState, node_id: NodeId, prop: TweenProp, value: f32) {
    let patch = prop.write(value);
    let layout = prop.is_layout();
    ui_state.with_tree_mut(|t| {
        if let Some(node) = t.get(node_id) {
            let mut merged = node.style.clone();
            merged.merge_from(&patch);
            // Position tweens reflow; opacity/scale/rotate keep the cached
            // layout (set_style_paint marks paint-dirty only).
            if layout {
                t.set_style(node_id, merged);
            } else {
                t.set_style_paint(node_id, merged);
            }
        }
    });
}
