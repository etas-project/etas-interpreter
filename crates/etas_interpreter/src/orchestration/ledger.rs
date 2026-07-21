use super::{CheckpointId, RetryAttemptId};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WorkflowStepId(pub u32);

pub use etas_host::HostRequestId;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkflowEvent {
    StepStarted(WorkflowStepId),
    StepCompleted(WorkflowStepId),
    HostRequestSent(HostRequestId),
    HostResponseReceived(HostRequestId),
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
    SessionCompacted {
        session: String,
        summary_message_count: usize,
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
