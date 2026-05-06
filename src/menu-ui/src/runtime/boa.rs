//! Boa context bootstrap, owned by `menu-ui` (not via firmware-script).
//!
//! N1 keeps this minimal: no module loader (the bundle is a single
//! self-contained ESM file passed in directly), `console.*` from
//! `boa_runtime` routed through `tracing`, and our [`crate::host`]
//! module registered as a synthetic module so JS code can
//! `import * as gui from '1fpga:gui'`.

use std::cell::RefCell;
use std::rc::Rc;

use boa_engine::job::{JobExecutor, SimpleJobExecutor};
use boa_engine::module::MapModuleLoader;
use boa_engine::{Context, JsResult, JsString};
use boa_gc::{Finalize, Trace};
use boa_runtime::extensions::ConsoleExtension;
use boa_runtime::{ConsoleState, Logger};
use tracing::{debug, error, info, warn};

/// Build a fresh Boa context with the runtime extensions and our
/// `1fpga:gui` host module registered.
///
/// Returns the executor too so the caller can drive it tick-style
/// (see [`tick_jobs`]) instead of via the blocking `Context::run_jobs`,
/// which spins forever once `setInterval` is in play.
pub fn build_context() -> JsResult<(Context, Rc<SimpleJobExecutor>, Rc<MapModuleLoader>)> {
    let loader = Rc::new(MapModuleLoader::new());
    let executor = Rc::new(SimpleJobExecutor::new());
    let mut context = Context::builder()
        .module_loader(loader.clone())
        .job_executor(executor.clone())
        .build()?;
    boa_runtime::register(ConsoleExtension(TracingLogger), None, &mut context)?;
    boa_runtime::interval::register(&mut context)?;
    crate::host::register(&loader, &mut context)?;
    Ok((context, executor, loader))
}

/// Drive `SimpleJobExecutor::run_jobs_async` for a bounded number of
/// iterations, abandoning the future if it doesn't terminate.
///
/// Boa's `Context::run_jobs` (and the executor's `run_jobs` / blocking
/// `block_on(run_jobs_async)`) loops until *every* queued job —
/// including future-scheduled timeouts — drains. A recurring
/// `setInterval` keeps the queue non-empty forever, so the blocking
/// call never returns and the frame loop hangs.
///
/// `run_jobs_async`'s body is, however, a forward-progressing state
/// machine: each iteration drains async jobs into a `FutureGroup`,
/// runs every past-due timeout, drains promise + generic queues, polls
/// one async future, then `yield_now().await`s. By driving the future
/// via `poll_once` we get exactly one iteration of work per poll, then
/// control returns to us — no blocking on future timeouts.
///
/// We poll up to `MAX_ITERATIONS` times per call to let chained
/// promise resolutions settle (React's commit cycle typically needs
/// 2-3 iterations to quiesce).
///
/// Tradeoff: in-flight async jobs (NativeAsyncJob) live in the
/// executor's local `FutureGroup`, which is dropped when we abandon
/// the future. Multi-poll async jobs would be orphaned. In menu-ui
/// the only async work is module loading at startup (synthetic
/// modules, single-poll), driven via `await_blocking` before any
/// recurring timer is registered, so this is safe.
pub fn tick_jobs(executor: &Rc<SimpleJobExecutor>, context: &mut Context) -> JsResult<()> {
    use futures_lite::future;

    const MAX_ITERATIONS: u32 = 16;

    let cell = RefCell::new(context);
    let exec = executor.clone();
    future::block_on(async move {
        let mut fut = Box::pin(exec.run_jobs_async(&cell));
        for _ in 0..MAX_ITERATIONS {
            match future::poll_once(fut.as_mut()).await {
                Some(result) => return result,
                None => continue,
            }
        }
        Ok(())
    })
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
