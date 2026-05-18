//! Render-target dimensions exposed to JS so layouts can size relative
//! to the actual framebuffer instead of hardcoding 1920×1080.
//!
//! Cloneable handle (`Rc<Cell<...>>` inside) so the runtime stores one
//! reference at startup and the `1fpga:gui.viewport()` host function
//! reads from it on demand. `Trace`/`Finalize`/`JsData` so it can ride
//! in the Boa context next to the other shared state.

use std::cell::Cell;
use std::rc::Rc;

use boa_macros::{Finalize, JsData, Trace};

#[derive(Default, Clone, Trace, Finalize, JsData)]
pub struct Viewport {
    #[unsafe_ignore_trace]
    inner: Rc<Cell<(u16, u16)>>,
}

impl Viewport {
    pub fn new(width: u16, height: u16) -> Self {
        Self {
            inner: Rc::new(Cell::new((width, height))),
        }
    }

    pub fn set(&self, width: u16, height: u16) {
        self.inner.set((width, height));
    }

    pub fn get(&self) -> (u16, u16) {
        self.inner.get()
    }
}
