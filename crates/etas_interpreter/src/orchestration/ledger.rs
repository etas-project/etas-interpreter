use super::{CheckpointId, RetryAttemptId};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WorkflowStepId(pub u32);

#[derive(Clone, Debug, PartialEq)]
pub enum WorkflowEvent {
    StepStarted(WorkflowStepId),
    StepCompleted(WorkflowStepId),
    ScopeEntered {
        scope: u64,
        parent: u64,
        concurrency: u32,
    },
    ScopeExited {
        scope: u64,
        outcome: String,
    },
    TaskStarted {
        scope: u64,
        task: u64,
        source_index: usize,
    },
    TaskSettled {
        scope: u64,
        task: u64,
        outcome: String,
    },
    TaskParked {
        scope: u64,
        task: u64,
        waiting_on: u64,
    },
    TaskReady {
        scope: u64,
        task: u64,
    },
    ContinuationTransferred {
        scope: u64,
        continuation: u64,
        task: Option<u64>,
        kind: String,
    },
    HostTrace(etas_host::TraceEvent),
    StorageWrite {
        request: etas_host::HostRequestId,
        evidence: etas_host::StorageWriteEvidence,
    },
    CheckpointCreated(CheckpointId),
    MessageCreated {
        id: String,
        from: Option<String>,
        to: Option<String>,
        session: Option<String>,
        role: String,
        created_at: String,
        payload: Box<crate::value::InterpValue>,
        provenance: Option<crate::value::ProvenanceValue>,
    },
    MessageSessionAttached {
        id: String,
        session: String,
        session_config: crate::value::SessionConfigValue,
    },
    MessageHandoff {
        id: String,
        from: Option<String>,
        to: Option<String>,
        session: Option<String>,
        target_item: u32,
    },
    AgentTracePlan {
        item: u32,
        trace: Vec<String>,
    },
    SessionResolved {
        session: String,
        created: bool,
    },
    SessionMessageAppended {
        session: String,
        message: String,
        deduplicated: bool,
    },
    SessionHistoryLoaded {
        session: String,
        message_count: usize,
        has_summary: bool,
        cursor: Option<String>,
    },
    RetryAttemptStarted(RetryAttemptId),
    RetryAttemptSucceeded(RetryAttemptId),
    RetryAttemptFailed(RetryAttemptId),
    RetryExhausted,
    ModelRepairAttempted {
        kind: String,
        attempt: usize,
        reason: String,
    },
    ModelRepairExhausted {
        kind: String,
        attempts: usize,
        reason: String,
    },
}
