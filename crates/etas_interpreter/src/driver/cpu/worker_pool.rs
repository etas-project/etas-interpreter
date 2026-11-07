use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::{
    CpuBudgetDelta, CpuExecutionFailure, CpuExecutor, CpuJobId, CpuSubmitError, CpuTaskCapsule,
    CpuTaskCompletion, CpuTaskMetrics,
};
use crate::orchestration::CpuWorkerId;

mod queue;
mod state;
use queue::{Job, WorkQueue};
use state::{JobTicket, RunState};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum WorkerExit {
    Clean,
    Panicked(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CpuWorkerHealth {
    Starting,
    Idle,
    Running(CpuJobId),
    Exited(WorkerExit),
}

/// One endpoint per invocation; completions and durable job IDs stay run-local.
pub(crate) struct WorkerPoolCpuExecutor {
    pool: Arc<SharedCpuPool>,
    id: u64,
    quota: NonZeroUsize,
    next_job: AtomicU64,
    run: Arc<RunState>,
}

/// Pool ownership is separate from run endpoints and from the worker closures.
pub(crate) struct SharedCpuPool {
    capacity: NonZeroUsize,
    next_run: AtomicU64,
    work: Arc<WorkQueue>,
    health: Arc<Mutex<Vec<CpuWorkerHealth>>>,
    threads: Vec<std::thread::JoinHandle<()>>,
    live: Arc<AtomicUsize>,
}

impl SharedCpuPool {
    pub(crate) fn new(capacity: NonZeroUsize) -> Result<Self, std::io::Error> {
        if capacity.get() > usize::from(u16::MAX) + 1 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "CPU worker count exceeds logical ID space",
            ));
        }
        let mut pool = Self {
            capacity,
            next_run: AtomicU64::new(0),
            work: Arc::new(WorkQueue::default()),
            health: Arc::new(Mutex::new(vec![CpuWorkerHealth::Starting; capacity.get()])),
            threads: Vec::with_capacity(capacity.get()),
            live: Arc::new(AtomicUsize::new(0)),
        };
        for index in 0..capacity.get() {
            let work = pool.work.clone();
            let health = pool.health.clone();
            let live = pool.live.clone();
            let worker = CpuWorkerId(index as u16);
            let thread = std::thread::Builder::new()
                .name(format!("etas-cpu-{index}"))
                .spawn(move || {
                    let _lifetime = WorkerLifetime::start(live);
                    let mut current = None;
                    let exit = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        loop {
                            health.lock().expect("worker health poisoned")[index] =
                                CpuWorkerHealth::Idle;
                            // The queue lock covers dispatch only. Evaluators and their
                            // Rc state are created and released on this worker.
                            let Some(Job { capsule, run }) = work.next() else {
                                return WorkerExit::Clean;
                            };
                            current = Some((JobTicket::new(&capsule), run.clone()));
                            health.lock().expect("worker health poisoned")[index] =
                                CpuWorkerHealth::Running(capsule.job);
                            run.complete(capsule.execute(worker));
                            current = None;
                        }
                    }));
                    let exit = match exit {
                        Ok(exit) => exit,
                        Err(payload) => {
                            let message = panic_message(payload);
                            if let Some((ticket, run)) = current {
                                run.complete(ticket.failed(
                                    worker,
                                    message.clone(),
                                    Some(std::thread::current().id()),
                                ));
                            }
                            WorkerExit::Panicked(message)
                        }
                    };
                    health.lock().expect("worker health poisoned")[index] =
                        CpuWorkerHealth::Exited(exit);
                    work.wake_runs();
                })?;
            pool.threads.push(thread);
        }
        #[cfg(test)]
        TEST_POOLS.with(|pools| pools.borrow_mut().push(Arc::downgrade(&pool.live)));
        Ok(pool)
    }

    pub(crate) fn capacity(&self) -> NonZeroUsize {
        self.capacity
    }

    pub(crate) fn executor(self: &Arc<Self>, quota: NonZeroUsize) -> WorkerPoolCpuExecutor {
        let id = self.next_run.fetch_add(1, Ordering::Relaxed);
        let run = Arc::new(RunState::default());
        self.work.register(id, &run);
        WorkerPoolCpuExecutor {
            pool: self.clone(),
            id,
            quota: quota.min(self.capacity),
            next_job: AtomicU64::new(0),
            run,
        }
    }
}

impl WorkerPoolCpuExecutor {
    pub(crate) fn new(capacity: NonZeroUsize) -> Result<Self, std::io::Error> {
        Ok(Arc::new(SharedCpuPool::new(capacity)?).executor(capacity))
    }
}

impl CpuExecutor for WorkerPoolCpuExecutor {
    fn capacity(&self) -> NonZeroUsize {
        self.quota
    }

    fn submit(&self, mut capsule: CpuTaskCapsule) -> Result<CpuJobId, CpuSubmitError> {
        let job = CpuJobId(self.next_job.fetch_add(1, Ordering::Relaxed));
        capsule.assign_job(job);
        capsule.submitted = Some(Instant::now());
        self.run
            .completions
            .lock()
            .expect("CPU completions poisoned")
            .pending
            .insert(job, JobTicket::new(&capsule));
        if let Err(error) = self.pool.work.submit(
            self.id,
            Job {
                capsule,
                run: self.run.clone(),
            },
        ) {
            self.run
                .completions
                .lock()
                .expect("CPU completions poisoned")
                .pending
                .remove(&job);
            return Err(error);
        }
        Ok(job)
    }

    fn try_completion(&self) -> Option<CpuTaskCompletion> {
        let mut state = self
            .run
            .completions
            .lock()
            .expect("CPU completions poisoned");
        if let Some(completion) = state.ready.pop_front() {
            state.pending.remove(&completion.job);
            return Some(completion);
        }
        let health = self.pool.health.lock().expect("worker health poisoned");
        if health
            .iter()
            .all(|worker| matches!(worker, CpuWorkerHealth::Exited(_)))
        {
            let (_, ticket) = state.pending.pop_first()?;
            let failed = health
                .iter()
                .position(|worker| {
                    matches!(worker, CpuWorkerHealth::Exited(WorkerExit::Panicked(_)))
                })
                .unwrap_or(0);
            return Some(ticket.failed(
                CpuWorkerId(failed as u16),
                "CPU worker pool exited before completing the job".into(),
                None,
            ));
        }
        None
    }

    fn completion_signal(&self) -> &tokio::sync::Notify {
        &self.run.ready
    }

    fn cancel(&self, job: CpuJobId, _reason: etas_host::execution::CancellationReason) {
        if let Some(ticket) = self
            .run
            .completions
            .lock()
            .expect("CPU completions poisoned")
            .pending
            .get(&job)
        {
            ticket.cancel();
        }
    }

    fn health(&self) -> Vec<CpuWorkerHealth> {
        self.pool
            .health
            .lock()
            .expect("worker health poisoned")
            .clone()
    }
}

impl Drop for WorkerPoolCpuExecutor {
    fn drop(&mut self) {
        self.run.cancel_all();
        self.pool.work.remove(self.id);
    }
}

impl Drop for SharedCpuPool {
    fn drop(&mut self) {
        self.work.stop();
        // Cancellation is cooperative. A broken internal invariant must not make
        // runtime destruction block its caller forever.
        let deadline = Instant::now() + Duration::from_secs(2);
        while self.threads.iter().any(|thread| !thread.is_finished()) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(1));
        }
        for thread in self.threads.drain(..) {
            if thread.is_finished() {
                let _ = thread.join();
            }
        }
    }
}

struct WorkerLifetime(Arc<AtomicUsize>);
impl WorkerLifetime {
    fn start(live: Arc<AtomicUsize>) -> Self {
        live.fetch_add(1, Ordering::Relaxed);
        Self(live)
    }
}
impl Drop for WorkerLifetime {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Release);
    }
}

fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_owned()
    } else {
        "CPU worker panicked with a non-string payload".to_owned()
    }
}

#[cfg(test)]
std::thread_local! {
    static TEST_POOLS: std::cell::RefCell<Vec<std::sync::Weak<AtomicUsize>>> = const { std::cell::RefCell::new(Vec::new()) };
}

#[cfg(test)]
pub(crate) fn take_test_pools() -> Vec<std::sync::Weak<AtomicUsize>> {
    TEST_POOLS.with(|pools| std::mem::take(&mut *pools.borrow_mut()))
}
