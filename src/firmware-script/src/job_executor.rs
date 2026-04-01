//! Job executor that processes one batch of each job type per call.
//! Uses synchronous execution to avoid nested `block_on` deadlocks.
//! Async jobs are handled via `block_on` only when present (module loading).
//! Timeout jobs are processed one batch at a time to prevent infinite
//! loops from React's scheduler.

use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::mem;
use std::rc::Rc;

use boa_engine::context::time::JsInstant;
use boa_engine::job::{GenericJob, Job, JobExecutor, NativeAsyncJob, PromiseJob, TimeoutJob};
use boa_engine::{Context, JsResult};
use futures_concurrency::future::FutureGroup;
use futures_lite::{StreamExt, future};

#[derive(Default)]
pub struct ImmediateJobExecutor {
    promise_jobs: RefCell<VecDeque<PromiseJob>>,
    async_jobs: RefCell<VecDeque<NativeAsyncJob>>,
    generic_jobs: RefCell<VecDeque<GenericJob>>,
    timeout_jobs: RefCell<BTreeMap<JsInstant, Vec<TimeoutJob>>>,
}

impl std::fmt::Debug for ImmediateJobExecutor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImmediateJobExecutor").finish_non_exhaustive()
    }
}

impl ImmediateJobExecutor {
    /// Synchronously drain promise and generic jobs until no more are pending.
    fn drain_microtasks(&self, context: &mut Context) -> JsResult<()> {
        let max = 500;
        for round in 0..max {
            let mut did_work = false;

            let pjobs: VecDeque<_> = mem::take(&mut *self.promise_jobs.borrow_mut());
            let pcount = pjobs.len();
            for job in pjobs {
                job.call(context)?;
                did_work = true;
            }

            let gjobs: VecDeque<_> = mem::take(&mut *self.generic_jobs.borrow_mut());
            let gcount = gjobs.len();
            for job in gjobs {
                job.call(context)?;
                did_work = true;
            }

            if round < 5 || round % 100 == 0 {
                if pcount > 0 || gcount > 0 {
                    eprintln!("[Jobs] microtask round {round}: promises={pcount} generics={gcount}");
                }
            }

            if !did_work {
                break;
            }
        }
        Ok(())
    }
}

impl JobExecutor for ImmediateJobExecutor {
    fn enqueue_job(self: Rc<Self>, job: Job, context: &mut Context) {
        match job {
            Job::PromiseJob(p) => self.promise_jobs.borrow_mut().push_back(p),
            Job::AsyncJob(a) => self.async_jobs.borrow_mut().push_back(a),
            Job::GenericJob(g) => self.generic_jobs.borrow_mut().push_back(g),
            Job::TimeoutJob(t) => {
                let now = context.clock().now();
                self.timeout_jobs
                    .borrow_mut()
                    .entry(now + t.timeout())
                    .or_default()
                    .push(t);
            }
            _ => {}
        }
    }

    fn run_jobs(self: Rc<Self>, context: &mut Context) -> JsResult<()> {
        // If there are async jobs, use block_on to poll them (needed for module loading).
        // This is safe because async jobs don't re-enter run_jobs.
        if !self.async_jobs.borrow().is_empty() {
            future::block_on(self.clone().run_jobs_async(&RefCell::new(context)))?;
            return Ok(());
        }

        // Synchronous path: process one batch of timeouts + drain microtasks.
        // No block_on, so timeout callbacks that internally use block_on won't deadlock.

        // 1. Extract and run due timeout jobs
        let now = context.clock().now();
        let jobs_to_run = {
            let mut timeout_jobs = self.timeout_jobs.borrow_mut();
            let mut jobs_to_keep = timeout_jobs.split_off(&now);
            jobs_to_keep.retain(|_, jobs| {
                jobs.retain(|job| !job.cancelled());
                !jobs.is_empty()
            });
            mem::replace(&mut *timeout_jobs, jobs_to_keep)
        };

        for jobs in jobs_to_run.into_values() {
            for job in jobs {
                if !job.cancelled() {
                    eprintln!("[Jobs] sync: calling timeout job...");
                    job.call(context)?;
                    eprintln!("[Jobs] sync: timeout job returned");
                }
            }
        }

        // 2. Drain microtasks (promises + generics) produced by timeout callbacks
        eprintln!("[Jobs] sync: draining microtasks...");
        self.drain_microtasks(context)?;
        eprintln!("[Jobs] sync: microtasks done");

        context.clear_kept_objects();
        Ok(())
    }

    async fn run_jobs_async(
        self: Rc<Self>,
        context: &RefCell<&mut Context>,
    ) -> JsResult<()> {
        let mut group = FutureGroup::new();

        // Loop until async futures and microtasks are drained.
        // Timeouts: one batch only.
        let mut timeout_batch_done = false;
        let max_rounds = 1000;

        for _ in 0..max_rounds {
            let mut did_work = false;

            for job in mem::take(&mut *self.async_jobs.borrow_mut()) {
                group.insert(job.call(context));
                did_work = true;
            }

            if !timeout_batch_done {
                timeout_batch_done = true;
                let now = context.borrow().clock().now();
                let jobs_to_run = {
                    let mut timeout_jobs = self.timeout_jobs.borrow_mut();
                    let mut jobs_to_keep = timeout_jobs.split_off(&now);
                    jobs_to_keep.retain(|_, jobs| {
                        jobs.retain(|job| !job.cancelled());
                        !jobs.is_empty()
                    });
                    mem::replace(&mut *timeout_jobs, jobs_to_keep)
                };

                for jobs in jobs_to_run.into_values() {
                    for job in jobs {
                        if !job.cancelled() {
                            job.call(&mut context.borrow_mut())?;
                            did_work = true;
                        }
                    }
                }
            }

            if !group.is_empty() {
                if let Some(Err(err)) = future::poll_once(group.next()).await.flatten() {
                    return Err(err);
                }
                did_work = true;
            }

            {
                let jobs: VecDeque<_> = mem::take(&mut *self.promise_jobs.borrow_mut());
                for job in jobs {
                    job.call(&mut context.borrow_mut())?;
                    did_work = true;
                }
            }

            {
                let jobs: VecDeque<_> = mem::take(&mut *self.generic_jobs.borrow_mut());
                for job in jobs {
                    job.call(&mut context.borrow_mut())?;
                    did_work = true;
                }
            }

            if !did_work && group.is_empty() {
                break;
            }

            future::yield_now().await;
        }

        context.borrow_mut().clear_kept_objects();
        Ok(())
    }
}
