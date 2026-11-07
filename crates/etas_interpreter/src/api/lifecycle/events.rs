use crate::api::WorkflowEvent;
use std::sync::Arc;

/// Per-task CPU boundary costs, excluded from durable workflow events.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CpuTaskTimings {
    pub input_freeze: std::time::Duration,
    pub program_prepare: std::time::Duration,
    pub queue_wait: std::time::Duration,
    pub input_thaw: std::time::Duration,
    pub bootstrap: std::time::Duration,
    pub execution: std::time::Duration,
    pub output_freeze: std::time::Duration,
    pub output_thaw: std::time::Duration,
    pub return_wait: std::time::Duration,
}

/// Process-local evidence for profiling. This data is never part of replay or
/// checkpoint workflow events.
#[derive(Clone, Debug)]
pub struct CpuTaskTelemetry {
    pub job: u64,
    pub worker: crate::orchestration::CpuWorkerId,
    pub dispatch_sequence: u64,
    pub completion_sequence: u64,
    pub thread: Option<std::thread::ThreadId>,
    pub elapsed: std::time::Duration,
    pub consumed_steps: u64,
    pub timings: CpuTaskTimings,
}

/// A non-blocking observer of the same events retained in the final run result.
/// Observers must not perform I/O or re-enter the interpreter.
pub trait RunEventObserver: std::fmt::Debug + Send + Sync {
    fn observe(&self, event: &WorkflowEvent);
    fn observe_cpu(&self, _telemetry: &CpuTaskTelemetry) {}
}

pub(crate) struct EventLog {
    events: Vec<WorkflowEvent>,
    observer: Option<Arc<dyn RunEventObserver>>,
}

impl EventLog {
    pub(crate) fn new(observer: Option<Arc<dyn RunEventObserver>>) -> Self {
        Self {
            events: Vec::new(),
            observer,
        }
    }
    pub(crate) fn push(&mut self, event: WorkflowEvent) {
        if let Some(observer) = &self.observer {
            observer.observe(&event);
        }
        self.events.push(event);
    }
    pub(crate) fn len(&self) -> usize {
        self.events.len()
    }
    pub(crate) fn cpu_telemetry(&self, telemetry: &CpuTaskTelemetry) {
        if let Some(observer) = &self.observer {
            observer.observe_cpu(telemetry);
        }
    }
    pub(crate) fn into_events(self) -> Vec<WorkflowEvent> {
        self.events
    }
}
