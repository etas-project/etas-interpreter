use std::collections::BTreeMap;
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Mutex, mpsc};

use super::{CpuExecutor, CpuJobId, CpuSubmitError, CpuTaskCapsule, CpuTaskCompletion};

enum WorkerMessage {
    Run(CpuTaskCapsule),
    Stop,
}

pub(crate) struct WorkerPoolCpuExecutor {
    capacity: NonZeroUsize,
    next_job: AtomicU64,
    next_worker: AtomicUsize,
    workers: Vec<mpsc::Sender<WorkerMessage>>,
    completions: Mutex<mpsc::Receiver<CpuTaskCompletion>>,
    ready: std::sync::Arc<tokio::sync::Notify>,
    cancellations: Mutex<BTreeMap<CpuJobId, std::sync::Arc<std::sync::atomic::AtomicBool>>>,
    threads: Mutex<Vec<std::thread::JoinHandle<()>>>,
}

impl WorkerPoolCpuExecutor {
    pub(crate) fn new(capacity: NonZeroUsize) -> Self {
        let (completion_tx, completion_rx) = mpsc::channel();
        let ready = std::sync::Arc::new(tokio::sync::Notify::new());
        let mut workers = Vec::with_capacity(capacity.get());
        let mut threads = Vec::with_capacity(capacity.get());
        for index in 0..capacity.get() {
            let (work_tx, work_rx) = mpsc::channel();
            let completion_tx = completion_tx.clone();
            let ready = ready.clone();
            let thread = std::thread::Builder::new()
                .name(format!("etas-cpu-{index}"))
                .spawn(move || {
                    while let Ok(message) = work_rx.recv() {
                        match message {
                            WorkerMessage::Run(capsule) => {
                                let completion = capsule.execute();
                                if completion_tx.send(completion).is_err() {
                                    break;
                                }
                                ready.notify_one();
                            }
                            WorkerMessage::Stop => break,
                        }
                    }
                })
                .expect("CPU worker thread must start");
            workers.push(work_tx);
            threads.push(thread);
        }
        Self {
            capacity,
            next_job: AtomicU64::new(0),
            next_worker: AtomicUsize::new(0),
            workers,
            completions: Mutex::new(completion_rx),
            ready,
            cancellations: Mutex::new(BTreeMap::new()),
            threads: Mutex::new(threads),
        }
    }
}

impl CpuExecutor for WorkerPoolCpuExecutor {
    fn capacity(&self) -> NonZeroUsize {
        self.capacity
    }

    fn submit(&self, mut capsule: CpuTaskCapsule) -> Result<CpuJobId, CpuSubmitError> {
        let job = CpuJobId(self.next_job.fetch_add(1, Ordering::Relaxed));
        capsule.assign_job(job);
        self.cancellations
            .lock()
            .expect("CPU cancellation map poisoned")
            .insert(job, capsule.cancellation.clone());
        let worker = self.next_worker.fetch_add(1, Ordering::Relaxed) % self.workers.len();
        if self.workers[worker]
            .send(WorkerMessage::Run(capsule))
            .is_err()
        {
            self.cancellations
                .lock()
                .expect("CPU cancellation map poisoned")
                .remove(&job);
            return Err(CpuSubmitError::ExecutorStopped);
        }
        Ok(job)
    }

    fn try_completion(&self) -> Option<CpuTaskCompletion> {
        let completion = self
            .completions
            .lock()
            .expect("CPU completion receiver poisoned")
            .try_recv()
            .ok()?;
        self.cancellations
            .lock()
            .expect("CPU cancellation map poisoned")
            .remove(&completion.job);
        Some(completion)
    }

    fn completion_signal(&self) -> &tokio::sync::Notify {
        &self.ready
    }

    fn cancel(&self, job: CpuJobId, _reason: etas_host::execution::CancellationReason) {
        if let Some(signal) = self
            .cancellations
            .lock()
            .expect("CPU cancellation map poisoned")
            .get(&job)
        {
            signal.store(true, Ordering::Release);
        }
    }
}

impl Drop for WorkerPoolCpuExecutor {
    fn drop(&mut self) {
        for worker in &self.workers {
            let _ = worker.send(WorkerMessage::Stop);
        }
        for thread in self
            .threads
            .get_mut()
            .expect("CPU worker thread list poisoned")
            .drain(..)
        {
            let _ = thread.join();
        }
    }
}
