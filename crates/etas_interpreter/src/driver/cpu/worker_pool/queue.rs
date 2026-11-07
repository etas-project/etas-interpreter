use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Condvar, Mutex, Weak};

use super::state::RunState;
use crate::driver::cpu::{CpuSubmitError, CpuTaskCapsule};

pub(super) struct Job {
    pub capsule: CpuTaskCapsule,
    pub run: Arc<RunState>,
}

struct RunQueue {
    state: Weak<RunState>,
    jobs: VecDeque<Job>,
}

#[derive(Default)]
struct State {
    runs: BTreeMap<u64, RunQueue>,
    ready: VecDeque<u64>,
    stopped: bool,
}

/// Rotate runnable invocations after each dispatch. Empty runs consume no turn.
#[derive(Default)]
pub(super) struct WorkQueue {
    state: Mutex<State>,
    available: Condvar,
}

impl WorkQueue {
    pub fn register(&self, id: u64, run: &Arc<RunState>) {
        self.state
            .lock()
            .expect("CPU work queue poisoned")
            .runs
            .insert(
                id,
                RunQueue {
                    state: Arc::downgrade(run),
                    jobs: VecDeque::new(),
                },
            );
    }

    pub fn submit(&self, id: u64, job: Job) -> Result<(), CpuSubmitError> {
        let mut state = self.state.lock().expect("CPU work queue poisoned");
        if state.stopped {
            return Err(CpuSubmitError::ExecutorStopped);
        }
        let run = state.runs.get_mut(&id).expect("registered CPU invocation");
        let empty = run.jobs.is_empty();
        run.jobs.push_back(job);
        if empty {
            state.ready.push_back(id);
        }
        self.available.notify_one();
        Ok(())
    }

    pub fn next(&self) -> Option<Job> {
        let mut state = self.state.lock().expect("CPU work queue poisoned");
        loop {
            if state.stopped {
                return None;
            }
            if let Some(id) = state.ready.pop_front() {
                let run = state.runs.get_mut(&id).expect("runnable CPU invocation");
                let job = run.jobs.pop_front().expect("runnable CPU job");
                if !run.jobs.is_empty() {
                    state.ready.push_back(id);
                }
                return Some(job);
            }
            state = self.available.wait(state).expect("CPU work queue poisoned");
        }
    }

    pub fn remove(&self, id: u64) {
        let mut state = self.state.lock().expect("CPU work queue poisoned");
        let removed = state.runs.remove(&id);
        state.ready.retain(|ready| *ready != id);
        drop(state);
        drop(removed);
    }

    pub fn wake_runs(&self) {
        for run in self
            .state
            .lock()
            .expect("CPU work queue poisoned")
            .runs
            .values()
        {
            if let Some(run) = run.state.upgrade() {
                run.ready.notify_one();
            }
        }
    }

    pub fn stop(&self) {
        let mut state = self.state.lock().expect("CPU work queue poisoned");
        for queue in state.runs.values() {
            if let Some(run) = queue.state.upgrade() {
                run.cancel_all();
            }
        }
        state.stopped = true;
        state.ready.clear();
        let runs = std::mem::take(&mut state.runs);
        self.available.notify_all();
        drop(state);
        for queue in runs.values() {
            if let Some(run) = queue.state.upgrade() {
                run.ready.notify_one();
            }
        }
    }
}
