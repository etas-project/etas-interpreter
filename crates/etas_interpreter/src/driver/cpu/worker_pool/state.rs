use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex, atomic::Ordering};
use std::time::Instant;

use super::{
    CpuBudgetDelta, CpuExecutionFailure, CpuJobId, CpuTaskCapsule, CpuTaskCompletion,
    CpuTaskMetrics, CpuWorkerId,
};

#[derive(Clone)]
pub(super) struct JobTicket {
    pub job: CpuJobId,
    dispatch_sequence: u64,
    cancellation: Arc<std::sync::atomic::AtomicBool>,
    started: Instant,
}

impl JobTicket {
    pub fn new(capsule: &CpuTaskCapsule) -> Self {
        Self {
            job: capsule.job,
            dispatch_sequence: capsule.dispatch_sequence,
            cancellation: capsule.cancellation.clone(),
            started: Instant::now(),
        }
    }

    pub fn failed(
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

    pub fn cancel(&self) {
        self.cancellation.store(true, Ordering::Release);
    }
}

#[derive(Default)]
pub(super) struct Completions {
    pub pending: BTreeMap<CpuJobId, JobTicket>,
    pub ready: VecDeque<CpuTaskCompletion>,
}

#[derive(Default)]
pub(super) struct RunState {
    pub completions: Mutex<Completions>,
    pub ready: tokio::sync::Notify,
}

impl RunState {
    pub fn complete(&self, completion: CpuTaskCompletion) {
        self.completions
            .lock()
            .expect("CPU completions poisoned")
            .ready
            .push_back(completion);
        self.ready.notify_one();
    }

    pub fn cancel_all(&self) {
        for ticket in self
            .completions
            .lock()
            .expect("CPU completions poisoned")
            .pending
            .values()
        {
            ticket.cancel();
        }
    }
}
