use std::collections::{BTreeMap, VecDeque};
use std::num::NonZeroUsize;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use super::{CpuExecutor, CpuJobId, CpuSubmitError, CpuTaskCapsule, CpuTaskCompletion};

pub(crate) struct InlineCpuExecutor {
    next_job: AtomicU64,
    completed: Mutex<VecDeque<CpuTaskCompletion>>,
    ready: tokio::sync::Notify,
    cancellations: Mutex<BTreeMap<CpuJobId, std::sync::Arc<std::sync::atomic::AtomicBool>>>,
}

impl InlineCpuExecutor {
    pub(crate) fn new() -> Self {
        Self {
            next_job: AtomicU64::new(0),
            completed: Mutex::new(VecDeque::new()),
            ready: tokio::sync::Notify::new(),
            cancellations: Mutex::new(BTreeMap::new()),
        }
    }
}

impl CpuExecutor for InlineCpuExecutor {
    fn capacity(&self) -> NonZeroUsize {
        NonZeroUsize::MIN
    }

    fn submit(&self, mut capsule: CpuTaskCapsule) -> Result<CpuJobId, CpuSubmitError> {
        let job = CpuJobId(self.next_job.fetch_add(1, Ordering::Relaxed));
        capsule.assign_job(job);
        self.cancellations
            .lock()
            .expect("inline CPU cancellation map poisoned")
            .insert(job, capsule.cancellation.clone());
        let completion = capsule.execute();
        self.completed
            .lock()
            .expect("inline CPU completion queue poisoned")
            .push_back(completion);
        self.ready.notify_one();
        Ok(job)
    }

    fn try_completion(&self) -> Option<CpuTaskCompletion> {
        let completion = self
            .completed
            .lock()
            .expect("inline CPU completion queue poisoned")
            .pop_front()?;
        self.cancellations
            .lock()
            .expect("inline CPU cancellation map poisoned")
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
            .expect("inline CPU cancellation map poisoned")
            .get(&job)
        {
            signal.store(true, Ordering::Release);
        }
    }
}
