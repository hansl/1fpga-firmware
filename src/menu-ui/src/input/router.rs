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
///
/// Two kinds of intent live here:
///
/// - **Semantic intents** (`confirm`, `back`, `menu`, …) are
///   abstract actions the UI uses to drive navigation. The default
///   keymap binds them to platform-appropriate keys/buttons.
/// - **Raw button intents** (`face_south`, `shoulder_l1`, …) fire
///   alongside the semantic ones so the bottom-bar / shortcuts
///   system can inspect "which physical input fired this".
pub mod intent {
    // Semantic / navigation actions.
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

    // Gamepad face buttons (compass naming — standard across
    // libraries that don't pick a brand convention). On a typical
    // Xbox-layout pad: south=A, east=B, west=X, north=Y. On a
    // Nintendo-layout pad: south=B, east=A, west=Y, north=X.
    pub const FACE_SOUTH: &str = "face_south";
    pub const FACE_EAST: &str = "face_east";
    pub const FACE_WEST: &str = "face_west";
    pub const FACE_NORTH: &str = "face_north";

    // Shoulders + triggers. L1/R1 are digital bumpers, L2/R2 are
    // typically analog triggers reported as buttons here (full pull
    // = pressed); analog values arrive as axis events elsewhere.
    pub const SHOULDER_L1: &str = "shoulder_l1";
    pub const SHOULDER_R1: &str = "shoulder_r1";
    pub const SHOULDER_L2: &str = "shoulder_l2";
    pub const SHOULDER_R2: &str = "shoulder_r2";

    // Centre cluster.
    pub const START: &str = "start";
    pub const SELECT: &str = "select";
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
///
/// Each physical input can fire **multiple** intent dispatches:
///
/// - The **semantic** intent (`confirm`, `back`, …) that
///   navigation code listens for.
/// - One or more **raw** intents (`face_south`, `shoulder_l1`, …)
///   that the bottom action bar / shortcut system inspects.
///
/// Listing both lets `<ActionBar>` show "Ⓐ Select" when the user is
/// holding a gamepad and "↵ Select" when they're on a keyboard,
/// while navigation handlers stay device-agnostic.
pub struct IntentRouter {
    keymap: HashMap<u16, Vec<&'static str>>,
    gamepad_map: HashMap<u16, Vec<&'static str>>,
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
    /// Most inputs map to one semantic intent plus zero-or-one raw
    /// button intent (e.g., gamepad south = `confirm` +
    /// `face_south`). Unmapped inputs yield an empty vec.
    pub fn translate(&self, ev: &RawInputEvent) -> Vec<IntentDispatch> {
        match ev {
            RawInputEvent::Keyboard(k) => {
                let kind = kind_from(k.kind, k.pressed);
                self.keymap
                    .get(&k.code)
                    .map(|names| {
                        names
                            .iter()
                            .map(|n| IntentDispatch {
                                name: n.to_string(),
                                kind,
                            })
                            .collect()
                    })
                    .unwrap_or_default()
            }
            RawInputEvent::Gamepad(g) => match g.kind {
                GamepadKind::Button { code, pressed } => {
                    let kind = if pressed {
                        IntentKind::Pressed
                    } else {
                        IntentKind::Released
                    };
                    self.gamepad_map
                        .get(&code)
                        .map(|names| {
                            names
                                .iter()
                                .map(|n| IntentDispatch {
                                    name: n.to_string(),
                                    kind,
                                })
                                .collect()
                        })
                        .unwrap_or_default()
                }
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

fn default_keyboard_map() -> HashMap<u16, Vec<&'static str>> {
    use intent::*;
    // Navigation / actions.
    let mut m: HashMap<u16, Vec<&'static str>> = HashMap::from([
        (keycode::ENTER, vec![CONFIRM]),
        (keycode::SPACE, vec![CONFIRM]),
        (keycode::ESC, vec![BACK]),
        (keycode::BACKSPACE, vec![BACK]),
        (keycode::UP, vec![NAVIGATE_UP]),
        (keycode::DOWN, vec![NAVIGATE_DOWN]),
        (keycode::LEFT, vec![NAVIGATE_LEFT]),
        (keycode::RIGHT, vec![NAVIGATE_RIGHT]),
        (keycode::TAB, vec![TAB]),
        (keycode::PAGE_UP, vec![PAGE_UP]),
        (keycode::PAGE_DOWN, vec![PAGE_DOWN]),
    ]);
    // Keyboard alternatives for the named gamepad buttons. Useful
    // for testing without a controller plugged in, and gives users
    // without a pad a way to reach face_north / shoulder_* shortcuts.
    // The Z/X/A/S layout mirrors the SNES emulator convention; the
    // shoulder/menu bindings are arbitrary but consistent.
    m.insert(keycode::Z, vec![FACE_SOUTH]);
    m.insert(keycode::X, vec![FACE_EAST]);
    m.insert(keycode::A, vec![FACE_WEST]);
    m.insert(keycode::S, vec![FACE_NORTH]);
    m.insert(keycode::Q, vec![SHOULDER_L1]);
    m.insert(keycode::W, vec![SHOULDER_R1]);
    m.insert(keycode::ONE, vec![SHOULDER_L2]);
    m.insert(keycode::TWO, vec![SHOULDER_R2]);
    m.insert(keycode::F1, vec![MENU]);
    m.insert(keycode::F11, vec![START]);
    m.insert(keycode::F12, vec![SELECT]);
    m
}

fn default_gamepad_map() -> HashMap<u16, Vec<&'static str>> {
    use intent::*;
    // evdev BTN_* codes. Each face/shoulder button fires both its
    // raw name (face_south / shoulder_l1 / …) and a semantic alias
    // (confirm / back / menu / …) so a single press drives both
    // the navigation system and the bottom-bar / shortcut overlay
    // without each having to translate the other.
    const BTN_SOUTH:      u16 = 0x130;
    const BTN_EAST:       u16 = 0x131;
    // 0x132 is BTN_C — unused on most modern pads.
    const BTN_NORTH:      u16 = 0x133;
    const BTN_WEST:       u16 = 0x134;
    const BTN_TL:         u16 = 0x136;  // L1
    const BTN_TR:         u16 = 0x137;  // R1
    const BTN_TL2:        u16 = 0x138;  // L2 (digital-side)
    const BTN_TR2:        u16 = 0x139;  // R2 (digital-side)
    const BTN_SELECT:     u16 = 0x13A;
    const BTN_START:      u16 = 0x13B;
    const BTN_MODE:       u16 = 0x13C;  // "guide" / "home" / "PS" — used for MENU
    const BTN_DPAD_UP:    u16 = 0x220;
    const BTN_DPAD_DOWN:  u16 = 0x221;
    const BTN_DPAD_LEFT:  u16 = 0x222;
    const BTN_DPAD_RIGHT: u16 = 0x223;
    HashMap::from([
        (BTN_SOUTH,      vec![FACE_SOUTH, CONFIRM]),
        (BTN_EAST,       vec![FACE_EAST, BACK]),
        (BTN_NORTH,      vec![FACE_NORTH]),
        (BTN_WEST,       vec![FACE_WEST]),
        (BTN_TL,         vec![SHOULDER_L1]),
        (BTN_TR,         vec![SHOULDER_R1]),
        (BTN_TL2,        vec![SHOULDER_L2]),
        (BTN_TR2,        vec![SHOULDER_R2]),
        (BTN_SELECT,     vec![SELECT]),
        (BTN_START,      vec![START]),
        (BTN_MODE,       vec![MENU]),
        (BTN_DPAD_UP,    vec![NAVIGATE_UP]),
        (BTN_DPAD_DOWN,  vec![NAVIGATE_DOWN]),
        (BTN_DPAD_LEFT,  vec![NAVIGATE_LEFT]),
        (BTN_DPAD_RIGHT, vec![NAVIGATE_RIGHT]),
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
        // KEY_B = 48 — not in the default keymap. (KEY_A, this test's
        // original probe, has been deliberately mapped to face_west
        // since the SNES-convention Z/X/A/S row was added — the test
        // was stale, not the router.)
        let d = r.translate(&key_event(48, true));
        assert!(d.is_empty());
    }
}
