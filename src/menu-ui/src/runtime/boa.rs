//! Boa context bootstrap, owned by `menu-ui` (not via firmware-script).
//!
//! N1 keeps this minimal: no module loader (the bundle is a single
//! self-contained ESM file passed in directly), `console.*` from
//! `boa_runtime` routed through `tracing`, and our [`crate::host`]
//! module registered as a synthetic module so JS code can
//! `import * as gui from '1fpga:gui'`.

use std::rc::Rc;

use boa_engine::module::MapModuleLoader;
use boa_engine::{Context, JsResult, JsString};
use boa_gc::{Finalize, Trace};
use boa_runtime::extensions::ConsoleExtension;
use boa_runtime::{ConsoleState, Logger};
use tracing::{debug, error, info, warn};

/// Build a fresh Boa context with the runtime extensions and our
/// `1fpga:gui` host module registered.
pub fn build_context() -> JsResult<(Context, Rc<MapModuleLoader>)> {
    let loader = Rc::new(MapModuleLoader::new());
    let mut context = Context::builder().module_loader(loader.clone()).build()?;
    boa_runtime::register(ConsoleExtension(TracingLogger), None, &mut context)?;
    crate::host::register(&loader, &mut context)?;
    Ok((context, loader))
}

/// Bridges `console.*` calls from JS to the host's `tracing`
/// subscriber so logs route through the same sink as the rest of the
/// firmware. Mirrors `firmware_script::console::TracingLogger`.
#[derive(Debug, Trace, Finalize)]
pub(crate) struct TracingLogger;

fn stack(context: &mut Context) -> Vec<String> {
    context
        .stack_trace()
        .map(|frame| frame.code_block().name())
        .map(JsString::to_std_string_escaped)
        .collect::<Vec<_>>()
}

impl Logger for TracingLogger {
    fn debug(&self, msg: String, state: &ConsoleState, context: &mut Context) -> JsResult<()> {
        let indent = state.indent();
        if tracing::enabled!(tracing::Level::TRACE) {
            let stack = stack(context);
            debug!(target: "menu_ui::js", ?stack, "{msg:>indent$}");
        } else {
            debug!(target: "menu_ui::js", "{msg:>indent$}");
        }
        Ok(())
    }

    fn log(&self, msg: String, state: &ConsoleState, context: &mut Context) -> JsResult<()> {
        let indent = state.indent();
        if tracing::enabled!(tracing::Level::TRACE) {
            let stack = stack(context);
            info!(target: "menu_ui::js", ?stack, "{msg:>indent$}");
        } else {
            info!(target: "menu_ui::js", "{msg:>indent$}");
        }
        Ok(())
    }

    fn info(&self, msg: String, state: &ConsoleState, context: &mut Context) -> JsResult<()> {
        self.log(msg, state, context)
    }

    fn warn(&self, msg: String, state: &ConsoleState, _context: &mut Context) -> JsResult<()> {
        let indent = state.indent();
        warn!(target: "menu_ui::js", "{msg:>indent$}");
        Ok(())
    }

    fn error(&self, msg: String, state: &ConsoleState, context: &mut Context) -> JsResult<()> {
        let indent = state.indent();
        let stack = stack(context);
        error!(target: "menu_ui::js", ?stack, "{msg:>indent$}");
        Ok(())
    }
}
