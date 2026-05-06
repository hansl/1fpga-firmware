//! Frame-loop-friendly Boa job executor.
//!
//! Boa's `SimpleJobExecutor::run_jobs` blocks until *all* queued jobs
//! (including future-scheduled timeouts) complete. That works for
//! one-shot scripts, but not for our frame loop: a single
//! `setInterval(fn, 250)` makes the queue never empty, so a
//! `run_jobs()` call would spin forever.
//!
//! This executor instead does a single "tick": run every promise /
//! generic job that's already enqueued and every timeout job whose
//! fire-time has passed, then return. Future timeouts stay queued
//! and are checked again on the next tick.

use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::mem;
use std::rc::Rc;

use boa_engine::Context;
use boa_engine::JsResult;
use boa_engine::context::time::JsInstant;
use boa_engine::job::{
    GenericJob, Job, JobExecutor, NativeAsyncJob, PromiseJob, TimeoutJob,
};

#[derive(Default)]
pub struct FrameJobExecutor {
    promise_jobs: RefCell<VecDeque<PromiseJob>>,
    generic_jobs: RefCell<VecDeque<GenericJob>>,
    timeout_jobs: RefCell<BTreeMap<JsInstant, Vec<TimeoutJob>>>,
    /// We don't drive `NativeAsyncJob` futures (no async runtime
    /// installed). Instead we collect them and silently drop — none
    /// of the JS we run today actually relies on async-only paths.
    _async_jobs: RefCell<VecDeque<NativeAsyncJob>>,
}

impl FrameJobExecutor {
    pub fn new() -> Rc<Self> {
        Rc::new(Self::default())
    }
}

impl JobExecutor for FrameJobExecutor {
    fn enqueue_job(self: Rc<Self>, job: Job, context: &mut Context) {
        match job {
            Job::PromiseJob(p) => self.promise_jobs.borrow_mut().push_back(p),
            Job::GenericJob(g) => self.generic_jobs.borrow_mut().push_back(g),
            Job::TimeoutJob(t) => {
                let now = context.clock().now();
                let fire_at = now + t.timeout();
                self.timeout_jobs
                    .borrow_mut()
                    .entry(fire_at)
                    .or_default()
                    .push(t);
            }
            Job::AsyncJob(a) => self._async_jobs.borrow_mut().push_back(a),
            Job::FinalizationRegistryCleanupJob(_) => {
                // Ignored: weakref/finalization cleanup is not
                // observable by the menu-ui workloads we run.
            }
            // `Job` is `#[non_exhaustive]`; new variants land in
            // future Boa versions and are silently dropped here.
            _ => {}
        }
    }

    fn run_jobs(self: Rc<Self>, context: &mut Context) -> JsResult<()> {
        // Drain promise jobs to a fixpoint — promise resolutions may
        // schedule further promise jobs. Bound iterations to avoid
        // any pathological infinite chain (e.g. `Promise.resolve()
        // .then(() => Promise.resolve())` looped recursively).
        for _ in 0..1024 {
            let jobs = mem::take(&mut *self.promise_jobs.borrow_mut());
            if jobs.is_empty() {
                break;
            }
            for job in jobs {
                job.call(context)?;
            }
        }

        // Past-due timeouts. `BTreeMap::split_off(&now)` returns the
        // entries with key >= now (still pending); we keep those and
        // run everything before.
        let due_jobs = {
            let mut timeouts = self.timeout_jobs.borrow_mut();
            let keep = timeouts.split_off(&context.clock().now());
            mem::replace(&mut *timeouts, keep)
        };
        for (_, jobs) in due_jobs {
            for job in jobs {
                if job.cancelled() {
                    continue;
                }
                job.call(context)?;
            }
        }

        // Generic jobs (rare; mostly for host-driven hooks).
        let jobs = mem::take(&mut *self.generic_jobs.borrow_mut());
        for job in jobs {
            job.call(context)?;
        }

        // Final promise drain — timeouts and generic jobs may have
        // scheduled new promises (React's setState path is one).
        for _ in 0..1024 {
            let jobs = mem::take(&mut *self.promise_jobs.borrow_mut());
            if jobs.is_empty() {
                break;
            }
            for job in jobs {
                job.call(context)?;
            }
        }

        context.clear_kept_objects();
        Ok(())
    }
}
