use std::collections::HashMap;
use std::time::Instant;

use crate::dom::NodeId;

/// Which property is being animated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnimatedProperty {
    Opacity,
    TranslateX,
    TranslateY,
    Scale,
}

/// Easing function for animation interpolation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EasingFn {
    #[default]
    Linear,
    EaseIn,
    EaseOut,
    EaseInOut,
}

impl EasingFn {
    /// Apply the easing function to a progress value (0.0 to 1.0).
    pub fn apply(self, t: f32) -> f32 {
        match self {
            EasingFn::Linear => t,
            EasingFn::EaseIn => t * t,
            EasingFn::EaseOut => 1.0 - (1.0 - t) * (1.0 - t),
            EasingFn::EaseInOut => {
                if t < 0.5 {
                    2.0 * t * t
                } else {
                    1.0 - (-2.0 * t + 2.0).powi(2) / 2.0
                }
            }
        }
    }
}

/// A single property animation (tween).
#[derive(Debug, Clone)]
pub struct Tween {
    pub property: AnimatedProperty,
    pub from: f32,
    pub to: f32,
    pub duration_ms: u32,
    pub easing: EasingFn,
    pub start_time: Instant,
}

impl Tween {
    /// Get the current value of this tween, or None if it has completed.
    pub fn current_value(&self, now: Instant) -> Option<f32> {
        let elapsed = now.duration_since(self.start_time).as_millis() as f32;
        let progress = (elapsed / self.duration_ms as f32).min(1.0);
        let eased = self.easing.apply(progress);
        let value = self.from + (self.to - self.from) * eased;

        if progress >= 1.0 {
            None // Animation complete
        } else {
            Some(value)
        }
    }

    /// Get the final value of this tween.
    pub fn final_value(&self) -> f32 {
        self.to
    }
}

/// The current animated state of a node, computed from active tweens.
#[derive(Debug, Clone, Default)]
pub struct AnimatedState {
    pub opacity: Option<f32>,
    pub translate_x: Option<f32>,
    pub translate_y: Option<f32>,
    pub scale: Option<f32>,
}

/// Manages all active animations. Runs independently of JS.
pub struct AnimationController {
    animations: HashMap<NodeId, Vec<Tween>>,
    states: HashMap<NodeId, AnimatedState>,
}

impl AnimationController {
    pub fn new() -> Self {
        Self {
            animations: HashMap::new(),
            states: HashMap::new(),
        }
    }

    /// Start an animation on a node.
    pub fn animate(&mut self, node_id: NodeId, tween: Tween) {
        // Remove any existing tween for the same property
        let tweens = self.animations.entry(node_id).or_default();
        tweens.retain(|t| t.property != tween.property);
        tweens.push(tween);
    }

    /// Tick all animations. Returns true if any animations are still active.
    pub fn tick(&mut self) -> bool {
        let now = Instant::now();
        let mut any_active = false;
        let mut completed_nodes = Vec::new();

        for (&node_id, tweens) in &mut self.animations {
            let mut state = AnimatedState::default();
            let mut all_done = true;

            tweens.retain(|tween| {
                if let Some(value) = tween.current_value(now) {
                    match tween.property {
                        AnimatedProperty::Opacity => state.opacity = Some(value),
                        AnimatedProperty::TranslateX => state.translate_x = Some(value),
                        AnimatedProperty::TranslateY => state.translate_y = Some(value),
                        AnimatedProperty::Scale => state.scale = Some(value),
                    }
                    all_done = false;
                    any_active = true;
                    true // Keep tween
                } else {
                    // Animation completed, apply final value to state
                    match tween.property {
                        AnimatedProperty::Opacity => state.opacity = Some(tween.final_value()),
                        AnimatedProperty::TranslateX => {
                            state.translate_x = Some(tween.final_value())
                        }
                        AnimatedProperty::TranslateY => {
                            state.translate_y = Some(tween.final_value())
                        }
                        AnimatedProperty::Scale => state.scale = Some(tween.final_value()),
                    }
                    false // Remove tween
                }
            });

            self.states.insert(node_id, state);

            if all_done {
                completed_nodes.push(node_id);
            }
        }

        // Clean up nodes with no active animations
        for node_id in completed_nodes {
            self.animations.remove(&node_id);
        }

        any_active
    }

    /// Get the current animated state for a node.
    pub fn get_state(&self, node_id: NodeId) -> Option<&AnimatedState> {
        self.states.get(&node_id)
    }

    /// Whether any animations are currently running.
    pub fn has_active(&self) -> bool {
        !self.animations.is_empty()
    }
}

impl Default for AnimationController {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_easing_linear() {
        assert_eq!(EasingFn::Linear.apply(0.0), 0.0);
        assert_eq!(EasingFn::Linear.apply(0.5), 0.5);
        assert_eq!(EasingFn::Linear.apply(1.0), 1.0);
    }

    #[test]
    fn test_easing_ease_out() {
        let v = EasingFn::EaseOut.apply(0.5);
        assert!(v > 0.5, "EaseOut should be ahead of linear at midpoint");
    }

    #[test]
    fn test_tween_completion() {
        let start = Instant::now();
        let tween = Tween {
            property: AnimatedProperty::Opacity,
            from: 0.0,
            to: 1.0,
            duration_ms: 100,
            easing: EasingFn::Linear,
            start_time: start,
        };

        // At start
        assert!(tween.current_value(start).is_some());

        // Well past completion
        let later = start + std::time::Duration::from_millis(200);
        assert!(tween.current_value(later).is_none());
    }
}
