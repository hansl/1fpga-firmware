//! Console module based on BOA's own implementation (in `boa_runtime`) but uses
//! `tracing` for logging instead of `println!`.

use boa_engine::{Context, JsResult, JsString};
use boa_gc::Trace;
use boa_macros::Finalize;
use boa_runtime::{ConsoleState, Logger};
use tracing::{debug, error, info, trace, warn};

fn stack(context: &mut Context) -> String {
    context
        .stack_trace()
        .map(|frame| {
            format!(
                "    {}",
                JsString::to_std_string_lossy(frame.code_block().name())
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[derive(Debug, Trace, Finalize)]
pub struct TracingLogger;

impl Logger for TracingLogger {
    fn trace(&self, msg: String, state: &ConsoleState, context: &mut Context) -> JsResult<()> {
        let indent = state.indent();
        let stack = stack(context);
        trace!("{msg:>indent$}\n{stack}");
        Ok(())
    }

    fn debug(&self, msg: String, state: &ConsoleState, context: &mut Context) -> JsResult<()> {
        let indent = state.indent();
        if tracing::enabled!(tracing::Level::TRACE) {
            let stack = stack(context);
            debug!("{msg:>indent$}\n{stack}");
        } else {
            debug!("{msg:>indent$}");
        }
        Ok(())
    }

    fn log(&self, msg: String, state: &ConsoleState, context: &mut Context) -> JsResult<()> {
        let indent = state.indent();
        if tracing::enabled!(tracing::Level::TRACE) {
            let stack = stack(context);
            info!("{msg:>indent$}\n{stack}");
        } else {
            info!("{msg:>indent$}");
        }
        Ok(())
    }

    fn info(&self, msg: String, state: &ConsoleState, context: &mut Context) -> JsResult<()> {
        self.log(msg, state, context)
    }

    fn warn(&self, msg: String, state: &ConsoleState, context: &mut Context) -> JsResult<()> {
        let indent = state.indent();
        if tracing::enabled!(tracing::Level::TRACE) {
            let stack = stack(context);
            warn!("{msg:>indent$}\n{stack}");
        } else {
            warn!("{msg:>indent$}");
        }
        Ok(())
    }

    fn error(&self, msg: String, state: &ConsoleState, context: &mut Context) -> JsResult<()> {
        let indent = state.indent();
        let stack = stack(context);
        error!("{msg:>indent$}\n{stack}");
        Ok(())
    }
}
