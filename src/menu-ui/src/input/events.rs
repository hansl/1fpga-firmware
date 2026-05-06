//! Raw input event types and small helpers.
//!
//! These types are the lowest-rung "what just happened" surface: a
//! key going down/up with a code, a gamepad button, a mouse motion.
//! Translation to high-level intents (`confirm`, `back`, …) happens
//! in `crate::input::router`.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputSource {
    Keyboard,
    Gamepad,
    Mouse,
}

#[derive(Debug, Clone)]
pub enum RawInputEvent {
    Keyboard(KeyEvent),
    Gamepad(GamepadEvent),
    Mouse(MouseEvent),
}

impl RawInputEvent {
    pub fn source(&self) -> InputSource {
        match self {
            RawInputEvent::Keyboard(_) => InputSource::Keyboard,
            RawInputEvent::Gamepad(_) => InputSource::Gamepad,
            RawInputEvent::Mouse(_) => InputSource::Mouse,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct KeyEvent {
    pub kind: KeyKind,
    /// Linux evdev keycode (`KEY_ENTER` = 28, `KEY_ESC` = 1, …).
    pub code: u16,
    /// True iff this is a key press (false = release).
    pub pressed: bool,
    /// Modifier state at the time of the event.
    pub mods: KeyMods,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyKind {
    Key,
    /// Auto-repeat fired (event value == 2 in evdev).
    Repeat,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct KeyMods {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub meta: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct GamepadEvent {
    pub kind: GamepadKind,
}

#[derive(Debug, Clone, Copy)]
pub enum GamepadKind {
    Button { code: u16, pressed: bool },
    Axis { axis: u8, value: i16 },
}

#[derive(Debug, Clone, Copy)]
pub struct MouseEvent {
    pub kind: MouseKind,
}

#[derive(Debug, Clone, Copy)]
pub enum MouseKind {
    Move { dx: i32, dy: i32 },
    Button { code: u16, pressed: bool },
    Wheel { delta: i32 },
}

// ---- Common Linux evdev keycodes (subset) -----------------------
// These match `<linux/input-event-codes.h>` and let our default
// keymap reference keys symbolically without pulling in a giant
// constant table.

pub mod keycode {
    pub const ESC: u16 = 1;
    pub const ENTER: u16 = 28;
    pub const SPACE: u16 = 57;
    pub const BACKSPACE: u16 = 14;
    pub const TAB: u16 = 15;
    pub const UP: u16 = 103;
    pub const LEFT: u16 = 105;
    pub const RIGHT: u16 = 106;
    pub const DOWN: u16 = 108;
    pub const HOME: u16 = 102;
    pub const END: u16 = 107;
    pub const PAGE_UP: u16 = 104;
    pub const PAGE_DOWN: u16 = 109;
}
