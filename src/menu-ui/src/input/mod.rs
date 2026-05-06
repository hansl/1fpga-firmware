//! Input pipeline.
//!
//! Three layers stack from low to high:
//!
//! 1. [`pump::Pump`] — opens Linux evdev devices (`/dev/input/event*`),
//!    translates kernel events to [`events::RawInputEvent`].
//! 2. [`router::IntentRouter`] — maps raw events to high-level intents
//!    (`confirm`, `back`, `navigate{direction}`, …) using a
//!    configurable keymap.
//! 3. JS dispatch (in `crate::host`) — fires `JsFunction` listeners
//!    registered via `1fpga:gui.addIntentListener` /
//!    `addRawInputListener`, scoped by focus where appropriate.
//!
//! N6a covers (1); (2) and (3) follow in N6b/N6c.

pub mod events;
pub mod pump;
pub mod router;
pub mod state;
