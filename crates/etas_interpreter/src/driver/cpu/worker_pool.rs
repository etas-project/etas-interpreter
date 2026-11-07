use std::collections::BTreeMap;
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use super::{
    CpuBudgetDelta, CpuExecutionFailure, CpuExecutor, CpuJobId, CpuSubmitError, CpuTaskCapsule,
    CpuTaskCompletion, CpuTaskMetrics,
};
use crate::orchestration::CpuWorkerId;

enum WorkerMessage {
    Run(CpuTaskCapsule),
    Stop,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum WorkerExit {
    Clean,
    Panicked(String),
    ChannelClosed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CpuWorkerHealth {
    Starting,
    Idle,
    Running(CpuJobId),
    Exited(WorkerExit),
}

#[derive(Clone)]
struct JobTicket {
    job: CpuJobId,
    dispatch_sequence: u64,
    cancellation: Arc<std::sync::atomic::AtomicBool>,
    started: Instant,
}

impl JobTicket {
    fn new(capsule: &CpuTaskCapsule) -> Self {
        Self {
            job: capsule.job,
            dispatch_sequence: capsule.dispatch_sequence,
            cancellation: capsule.cancellation.clone(),
            started: Instant::now(),
        }
    }

    fn failed(
        &self,
        worker: CpuWorkerId,
        message: String,
        thread: Option<std::thread::ThreadId>,
    ) -> CpuTaskCompletion {
        CpuTaskCompletion {
            job: self.job,
            result: Err(CpuExecutionFailure::Infrastructure { worker, message }),
            consumed_budget: CpuBudgetDelta::default(),
            metrics: CpuTaskMetrics {
                worker,
                dispatch_sequence: self.dispatch_sequence,
                thread,
                elapsed: self.started.elapsed(),
                finished_at: Instant::now(),
                timings: Default::default(),
            },
        }
    }
}

struct Completions {
    receiver: mpsc::Receiver<CpuTaskCompletion>,
    pending: BTreeMap<CpuJobId, JobTicket>,
}

pub(crate) struct WorkerPoolCpuExecutor {
    capacity: NonZeroUsize,
    next_job: AtomicU64,
    work: mpsc::Sender<WorkerMessage>,
    completions: Mutex<Completions>,
    ready: Arc<tokio::sync::Notify>,
    health: Arc<Mutex<Vec<CpuWorkerHealth>>>,
    threads: Vec<std::thread::JoinHandle<()>>,
    live: Arc<AtomicUsize>,
}

impl WorkerPoolCpuExecutor {
    pub(crate) fn new(capacity: NonZeroUsize) -> Result<Self, std::io::Error> {
        if capacity.get() > usize::from(u16::MAX) + 1 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "CPU worker count exceeds logical ID space",
            ));
        }
        let (completion_tx, completion_rx) = mpsc::channel();
        let (work_tx, work_rx) = mpsc::channel();
        let work_rx = Arc::new(Mutex::new(work_rx));
        let mut pool = Self {
            capacity,
            next_job: AtomicU64::new(0),
            work: work_tx,
            completions: Mutex::new(Completions {
                receiver: completion_rx,
                pending: BTreeMap::new(),
            }),
            ready: Arc::new(tokio::sync::Notify::new()),
            health: Arc::new(Mutex::new(vec![CpuWorkerHealth::Starting; capacity.get()])),
            threads: Vec::with_capacity(capacity.get()),
            live: Arc::new(AtomicUsize::new(0)),
        };
        for index in 0..capacity.get() {
            let work_rx = work_rx.clone();
            let completion_tx = completion_tx.clone();
            let ready = pool.ready.clone();
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
                            // Only dequeue is shared. Each evaluator and its Rc state
                            // are created and destroyed outside the queue lock.
                            let message = work_rx.lock().expect("CPU work queue poisoned").recv();
                            match message {
                                Ok(WorkerMessage::Run(capsule)) => {
                                    current = Some(JobTicket::new(&capsule));
                                    health.lock().expect("worker health poisoned")[index] =
                                        CpuWorkerHealth::Running(capsule.job);
                                    let completion = capsule.execute(worker);
                                    if completion_tx.send(completion).is_err() {
                                        return WorkerExit::ChannelClosed;
                                    }
                                    current = None;
                                    ready.notify_one();
                                }
                                Ok(WorkerMessage::Stop) => return WorkerExit::Clean,
                                Err(_) => return WorkerExit::ChannelClosed,
                            }
                        }
                    }));
                    let exit = match exit {
                        Ok(exit) => exit,
                        Err(payload) => {
                            let message = panic_message(payload);
                            if let Some(ticket) = current {
                                let _ = completion_tx.send(ticket.failed(
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
                    ready.notify_one();
                })?;
            pool.threads.push(thread);
        }
        #[cfg(test)]
        TEST_POOLS.with(|pools| pools.borrow_mut().push(Arc::downgrade(&pool.live)));
        Ok(pool)
    }
}

impl CpuExecutor for WorkerPoolCpuExecutor {
    fn capacity(&self) -> NonZeroUsize {
        self.capacity
    }

    fn submit(&self, mut capsule: CpuTaskCapsule) -> Result<CpuJobId, CpuSubmitError> {
        let job = CpuJobId(self.next_job.fetch_add(1, Ordering::Relaxed));
        capsule.assign_job(job);
        capsule.submitted = Some(Instant::now());
        self.completions
            .lock()
            .expect("CPU completions poisoned")
            .pending
            .insert(job, JobTicket::new(&capsule));
        if self.work.send(WorkerMessage::Run(capsule)).is_err() {
            self.completions
                .lock()
                .expect("CPU completions poisoned")
                .pending
                .remove(&job);
            return Err(CpuSubmitError::ExecutorStopped);
        }
        Ok(job)
    }

    fn try_completion(&self) -> Option<CpuTaskCompletion> {
        let mut state = self.completions.lock().expect("CPU completions poisoned");
        if let Ok(completion) = state.receiver.try_recv() {
            state.pending.remove(&completion.job);
            return Some(completion);
        }
        let health = self.health.lock().expect("worker health poisoned");
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
        &self.ready
    }

    fn cancel(&self, job: CpuJobId, _reason: etas_host::execution::CancellationReason) {
        if let Some(ticket) = self
            .completions
            .lock()
            .expect("CPU completions poisoned")
            .pending
            .get(&job)
        {
            ticket.cancellation.store(true, Ordering::Release);
        }
    }

    fn health(&self) -> Vec<CpuWorkerHealth> {
        self.health.lock().expect("worker health poisoned").clone()
    }
}

impl Drop for WorkerPoolCpuExecutor {
    fn drop(&mut self) {
        for ticket in self
            .completions
            .get_mut()
            .expect("CPU completions poisoned")
            .pending
            .values()
        {
            ticket.cancellation.store(true, Ordering::Release);
        }
        for _ in &self.threads {
            let _ = self.work.send(WorkerMessage::Stop);
        }
        // Cancellation is cooperative. A broken internal invariant must not make
        // executor destruction block its caller forever.
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
