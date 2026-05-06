//! Intent router: translates raw input events to high-level intent
//! names (`confirm`, `back`, `navigate_up`, …).
//!
//! N6 ships a fixed default keymap for keyboard + the most common
//! gamepad button codes. Future work adds a `1fpga:gui.setIntentMap`
//! host call so JS can override per-app (e.g. for IME-style
//! typing or game-specific bindings).

use std::collections::HashMap;

use crate::input::events::{
    GamepadKind, KeyKind, MouseKind, RawInputEvent, keycode,
};

/// Common intent names emitted by the default keymap. JS uses these
/// strings with `useIntent(name, handler)`. Custom intents are just
/// other strings — there's nothing magic about these constants.
pub mod intent {
    pub const CONFIRM: &str = "confirm";
    pub const BACK: &str = "back";
    pub const NAVIGATE_UP: &str = "navigate_up";
    pub const NAVIGATE_DOWN: &str = "navigate_down";
    pub const NAVIGATE_LEFT: &str = "navigate_left";
    pub const NAVIGATE_RIGHT: &str = "navigate_right";
    pub const MENU: &str = "menu";
    pub const TAB: &str = "tab";
    pub const PAGE_UP: &str = "page_up";
    pub const PAGE_DOWN: &str = "page_down";
}

/// One translated intent. `Pressed` fires on key-down, `Released` on
/// key-up; the JS-side hooks subscribe to whichever they care about.
/// `Repeat` fires while a key is held (kernel auto-repeat).
#[derive(Debug, Clone)]
pub struct IntentDispatch {
    pub name: String,
    pub kind: IntentKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntentKind {
    Pressed,
    Released,
    Repeat,
}

/// The router. Stateless for v1 — `translate` is a pure function of
/// the keymap + the raw event. State lives outside (focus stack,
/// listener registry).
pub struct IntentRouter {
    keymap: HashMap<u16, &'static str>,
    gamepad_map: HashMap<u16, &'static str>,
}

impl Default for IntentRouter {
    fn default() -> Self {
        Self {
            keymap: default_keyboard_map(),
            gamepad_map: default_gamepad_map(),
        }
    }
}

impl IntentRouter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Translate a raw event into zero-or-more intent dispatches.
    /// Most keys map to a single intent; some (e.g. modifiers) map
    /// to none.
    pub fn translate(&self, ev: &RawInputEvent) -> Vec<IntentDispatch> {
        match ev {
            RawInputEvent::Keyboard(k) => self
                .keymap
                .get(&k.code)
                .copied()
                .map(|name| {
                    vec![IntentDispatch {
                        name: name.to_string(),
                        kind: kind_from(k.kind, k.pressed),
                    }]
                })
                .unwrap_or_default(),
            RawInputEvent::Gamepad(g) => match g.kind {
                GamepadKind::Button { code, pressed } => self
                    .gamepad_map
                    .get(&code)
                    .copied()
                    .map(|name| {
                        vec![IntentDispatch {
                            name: name.to_string(),
                            kind: if pressed {
                                IntentKind::Pressed
                            } else {
                                IntentKind::Released
                            },
                        }]
                    })
                    .unwrap_or_default(),
                GamepadKind::Axis { .. } => Vec::new(),
            },
            RawInputEvent::Mouse(m) => match m.kind {
                MouseKind::Button { code: _, pressed } => vec![IntentDispatch {
                    name: intent::CONFIRM.to_string(),
                    kind: if pressed {
                        IntentKind::Pressed
                    } else {
                        IntentKind::Released
                    },
                }],
                _ => Vec::new(),
            },
        }
    }
}

fn kind_from(key_kind: KeyKind, pressed: bool) -> IntentKind {
    match key_kind {
        KeyKind::Repeat => IntentKind::Repeat,
        KeyKind::Key => {
            if pressed {
                IntentKind::Pressed
            } else {
                IntentKind::Released
            }
        }
    }
}

fn default_keyboard_map() -> HashMap<u16, &'static str> {
    use intent::*;
    HashMap::from([
        (keycode::ENTER, CONFIRM),
        (keycode::SPACE, CONFIRM),
        (keycode::ESC, BACK),
        (keycode::BACKSPACE, BACK),
        (keycode::UP, NAVIGATE_UP),
        (keycode::DOWN, NAVIGATE_DOWN),
        (keycode::LEFT, NAVIGATE_LEFT),
        (keycode::RIGHT, NAVIGATE_RIGHT),
        (keycode::TAB, TAB),
        (keycode::PAGE_UP, PAGE_UP),
        (keycode::PAGE_DOWN, PAGE_DOWN),
    ])
}

fn default_gamepad_map() -> HashMap<u16, &'static str> {
    use intent::*;
    // evdev BTN_* codes (subset). The mapping mirrors typical
    // console face-button conventions (south=A=confirm, east=B=back).
    const BTN_SOUTH: u16 = 0x130;
    const BTN_EAST: u16 = 0x131;
    const BTN_NORTH: u16 = 0x133;
    const BTN_WEST: u16 = 0x134;
    const BTN_START: u16 = 0x13B;
    const BTN_DPAD_UP: u16 = 0x220;
    const BTN_DPAD_DOWN: u16 = 0x221;
    const BTN_DPAD_LEFT: u16 = 0x222;
    const BTN_DPAD_RIGHT: u16 = 0x223;
    HashMap::from([
        (BTN_SOUTH, CONFIRM),
        (BTN_EAST, BACK),
        (BTN_NORTH, MENU),
        (BTN_WEST, TAB),
        (BTN_START, MENU),
        (BTN_DPAD_UP, NAVIGATE_UP),
        (BTN_DPAD_DOWN, NAVIGATE_DOWN),
        (BTN_DPAD_LEFT, NAVIGATE_LEFT),
        (BTN_DPAD_RIGHT, NAVIGATE_RIGHT),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::events::{KeyEvent, KeyKind, KeyMods};

    fn key_event(code: u16, pressed: bool) -> RawInputEvent {
        RawInputEvent::Keyboard(KeyEvent {
            kind: KeyKind::Key,
            code,
            pressed,
            mods: KeyMods::default(),
        })
    }

    #[test]
    fn enter_translates_to_confirm() {
        let r = IntentRouter::new();
        let dispatches = r.translate(&key_event(keycode::ENTER, true));
        assert_eq!(dispatches.len(), 1);
        assert_eq!(dispatches[0].name, intent::CONFIRM);
        assert_eq!(dispatches[0].kind, IntentKind::Pressed);
    }

    #[test]
    fn arrow_keys_translate_to_navigate() {
        let r = IntentRouter::new();
        for (code, expected) in [
            (keycode::UP, intent::NAVIGATE_UP),
            (keycode::DOWN, intent::NAVIGATE_DOWN),
            (keycode::LEFT, intent::NAVIGATE_LEFT),
            (keycode::RIGHT, intent::NAVIGATE_RIGHT),
        ] {
            let d = r.translate(&key_event(code, true));
            assert_eq!(d.len(), 1, "{expected:?}");
            assert_eq!(d[0].name, expected);
        }
    }

    #[test]
    fn unmapped_key_yields_empty() {
        let r = IntentRouter::new();
        // KEY_A = 30 — not in the default keymap.
        let d = r.translate(&key_event(30, true));
        assert!(d.is_empty());
    }
}
