//! Linux evdev pump.
//!
//! Opens every `/dev/input/event*` device that looks like a keyboard,
//! gamepad, or mouse, and drains pending events into a flat
//! [`RawInputEvent`] queue each frame. Cross-platform abstractions
//! (SDL, gilrs) are overkill for a single Linux target — evdev is a
//! pure-Rust crate that talks to the kernel's event interface
//! directly, with no SDL build dependencies.

use std::path::PathBuf;

use evdev::{Device, EventSummary, KeyCode};
use tracing::{info, warn};

use crate::input::events::{
    GamepadEvent, GamepadKind, InputSource, KeyEvent, KeyKind, KeyMods, MouseEvent, MouseKind,
    RawInputEvent,
};

const INPUT_DIR: &str = "/dev/input";

/// One opened input device + its derived classification.
struct OpenDevice {
    dev: Device,
    source: InputSource,
    path: PathBuf,
}

pub struct Pump {
    devices: Vec<OpenDevice>,
    mods: KeyMods,
}

impl Pump {
    /// Discover and open all input devices the kernel exposes that
    /// match a keyboard / gamepad / mouse profile. Errors opening a
    /// specific device are logged and skipped — the runtime keeps
    /// going with whatever devices it could open.
    pub fn open_all() -> Self {
        let mut devices = Vec::new();
        let entries = match std::fs::read_dir(INPUT_DIR) {
            Ok(it) => it,
            Err(e) => {
                warn!("input: cannot enumerate {INPUT_DIR}: {e}");
                return Self {
                    devices,
                    mods: KeyMods::default(),
                };
            }
        };
        for entry in entries.flatten() {
            let path = entry.path();
            // Filter to event* nodes (avoid mice/, mouse, js* aliases).
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if !name.starts_with("event") {
                continue;
            }
            match Device::open(&path) {
                Ok(mut dev) => {
                    if let Err(e) = dev.grab() {
                        // Non-fatal: another process may have it. We
                        // can still read events without exclusive
                        // access (just won't suppress duplicate
                        // delivery to the kernel's tty).
                        tracing::debug!("input: grab {} failed: {e}", path.display());
                    }
                    if let Err(e) = dev.set_nonblocking(true) {
                        warn!(
                            "input: set_nonblocking failed for {}: {e}; pump may block",
                            path.display()
                        );
                    }
                    let source = classify(&dev);
                    info!(
                        "input: opened {} ({:?}, name={:?})",
                        path.display(),
                        source,
                        dev.name().unwrap_or("?")
                    );
                    devices.push(OpenDevice { dev, source, path });
                }
                Err(e) => {
                    tracing::debug!("input: open {} failed: {e}", path.display());
                }
            }
        }
        Self {
            devices,
            mods: KeyMods::default(),
        }
    }

    /// Drain every pending event from every device. Non-blocking; the
    /// runtime calls this once per frame before paint.
    pub fn drain(&mut self, out: &mut Vec<RawInputEvent>) {
        // Iterate by index so we can borrow `self.mods` mutably while
        // pulling events from each device. evdev's fetch_events is
        // blocking by default; we set O_NONBLOCK on the fd so it
        // returns WouldBlock immediately when no events are pending.
        let n = self.devices.len();
        for i in 0..n {
            let source = self.devices[i].source;
            let path = self.devices[i].path.clone();
            // `set_nonblocking(true)` was applied at open time; this
            // returns `WouldBlock` immediately when the kernel queue
            // is empty.
            let events: Vec<evdev::InputEvent> = match self.devices[i].dev.fetch_events() {
                Ok(it) => it.collect(),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => Vec::new(),
                Err(e) => {
                    tracing::debug!("input: read {}: {e}", path.display());
                    Vec::new()
                }
            };
            if !events.is_empty() {
                tracing::info!(
                    "input: drained {} events from {} ({:?})",
                    events.len(),
                    path.display(),
                    source
                );
            }
            for ev in events {
                if let Some(raw) = translate_event(ev, source, &mut self.mods) {
                    out.push(raw);
                }
            }
        }
    }
}

fn classify(dev: &Device) -> InputSource {
    let keys = dev.supported_keys();
    let abs = dev.supported_absolute_axes();
    let rel = dev.supported_relative_axes();
    // Heuristic: if the device reports any gamepad-style button,
    // call it a gamepad. Else if it has REL_X/REL_Y, mouse. Else
    // keyboard.
    if let Some(keys) = keys {
        // Common gamepad face buttons. Cheap heuristic that catches
        // any modern controller advertised as such.
        if keys.contains(KeyCode::BTN_SOUTH)
            || keys.contains(KeyCode::BTN_NORTH)
            || keys.contains(KeyCode::BTN_EAST)
            || keys.contains(KeyCode::BTN_WEST)
            || keys.contains(KeyCode::BTN_START)
        {
            return InputSource::Gamepad;
        }
    }
    let _ = abs;
    if rel.is_some_and(|r| r.contains(evdev::RelativeAxisCode::REL_X)) {
        return InputSource::Mouse;
    }
    InputSource::Keyboard
}

fn translate_event(
    ev: evdev::InputEvent,
    source: InputSource,
    mods: &mut KeyMods,
) -> Option<RawInputEvent> {
    match ev.destructure() {
        EventSummary::Key(_, code, value) => {
            let pressed = value > 0;
            // Track modifier state on keyboards.
            update_mods(code, pressed, mods);
            let kind = if value == 2 {
                KeyKind::Repeat
            } else {
                KeyKind::Key
            };
            match source {
                InputSource::Keyboard => Some(RawInputEvent::Keyboard(KeyEvent {
                    kind,
                    code: code.0,
                    pressed,
                    mods: *mods,
                })),
                InputSource::Gamepad => Some(RawInputEvent::Gamepad(GamepadEvent {
                    kind: GamepadKind::Button {
                        code: code.0,
                        pressed,
                    },
                })),
                InputSource::Mouse => Some(RawInputEvent::Mouse(MouseEvent {
                    kind: MouseKind::Button {
                        code: code.0,
                        pressed,
                    },
                })),
            }
        }
        EventSummary::AbsoluteAxis(_, axis, value) => match source {
            InputSource::Gamepad => Some(RawInputEvent::Gamepad(GamepadEvent {
                kind: GamepadKind::Axis {
                    axis: axis.0 as u8,
                    value: value.clamp(i16::MIN as i32, i16::MAX as i32) as i16,
                },
            })),
            _ => None,
        },
        EventSummary::RelativeAxis(_, axis, value) => match source {
            InputSource::Mouse => match axis {
                evdev::RelativeAxisCode::REL_X => Some(RawInputEvent::Mouse(MouseEvent {
                    kind: MouseKind::Move { dx: value, dy: 0 },
                })),
                evdev::RelativeAxisCode::REL_Y => Some(RawInputEvent::Mouse(MouseEvent {
                    kind: MouseKind::Move { dx: 0, dy: value },
                })),
                evdev::RelativeAxisCode::REL_WHEEL => Some(RawInputEvent::Mouse(MouseEvent {
                    kind: MouseKind::Wheel { delta: value },
                })),
                _ => None,
            },
            _ => None,
        },
        // Synchronization, MSC, LED — ignore.
        _ => None,
    }
}

fn update_mods(code: KeyCode, pressed: bool, mods: &mut KeyMods) {
    match code {
        KeyCode::KEY_LEFTSHIFT | KeyCode::KEY_RIGHTSHIFT => mods.shift = pressed,
        KeyCode::KEY_LEFTCTRL | KeyCode::KEY_RIGHTCTRL => mods.ctrl = pressed,
        KeyCode::KEY_LEFTALT | KeyCode::KEY_RIGHTALT => mods.alt = pressed,
        KeyCode::KEY_LEFTMETA | KeyCode::KEY_RIGHTMETA => mods.meta = pressed,
        _ => {}
    }
}

