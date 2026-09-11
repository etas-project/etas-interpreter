use crate::api::WorkflowEvent;
use std::sync::Arc;

/// A non-blocking observer of the same events retained in the final run result.
/// Observers must not perform I/O or re-enter the interpreter.
pub trait RunEventObserver: std::fmt::Debug + Send + Sync {
    fn observe(&self, event: &WorkflowEvent);
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
    pub(crate) fn into_events(self) -> Vec<WorkflowEvent> {
        self.events
    }
}
