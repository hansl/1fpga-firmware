//! Rolling FPS counter shared between the runtime (which records each
//! frame) and JS (which reads via `1fpga:gui.fps()`).
//!
//! Cloneable handle (`Rc<Cell<f32>>` inside) so the runtime keeps
//! one reference and the host module can grab another to expose to
//! JS. `Trace`/`Finalize`/`JsData` so it can ride in the Boa context.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Instant;

use boa_macros::{Finalize, JsData, Trace};

#[derive(Default, Clone, Trace, Finalize, JsData)]
pub struct FpsCounter {
    #[unsafe_ignore_trace]
    inner: Rc<FpsInner>,
}

struct FpsInner {
    fps: Cell<f32>,
    window_start: Cell<Option<Instant>>,
    frames_in_window: Cell<u32>,
}

impl Default for FpsInner {
    fn default() -> Self {
        Self {
            fps: Cell::new(0.0),
            window_start: Cell::new(None),
            frames_in_window: Cell::new(0),
        }
    }
}

impl FpsCounter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record one rendered frame. Updates the rolling average over a
    /// 1-second window and (when crossing a window boundary) emits a
    /// `tracing` info line.
    pub fn record_frame(&self) {
        let now = Instant::now();
        let inner = &*self.inner;
        let start = inner.window_start.get().unwrap_or(now);
        if inner.window_start.get().is_none() {
            inner.window_start.set(Some(now));
        }
        let frames = inner.frames_in_window.get() + 1;
        inner.frames_in_window.set(frames);
        let elapsed = now.duration_since(start).as_secs_f32();
        if elapsed >= 1.0 {
            let fps = frames as f32 / elapsed;
            inner.fps.set(fps);
            inner.window_start.set(Some(now));
            inner.frames_in_window.set(0);
            tracing::info!("fps: {fps:.1}");
        }
    }

    /// Read the latest rolling-average FPS. Zero until the first
    /// 1-second window completes.
    pub fn current(&self) -> f32 {
        self.inner.fps.get()
    }
}
