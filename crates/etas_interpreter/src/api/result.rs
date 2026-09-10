use etas_core::Diagnostic;

use crate::{
    orchestration::{InterpreterCheckpoint, WorkflowEvent},
    plan::InterpreterPlan,
    value::InterpValue,
};

#[derive(Clone, Debug)]
pub struct PlanResult {
    pub plan: Option<InterpreterPlan>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Clone, Debug)]
pub struct RunResult {
    pub outcome: RunOutcome,
    pub termination: etas_host::execution::TerminationReport,
    pub diagnostics: Vec<Diagnostic>,
    pub events: Vec<WorkflowEvent>,
    pub checkpoints: Vec<InterpreterCheckpoint>,
}

#[derive(Clone, Debug)]
pub enum RunOutcome {
    Completed(InterpValue),
    Failed(RunFailure),
    Cancelled(etas_host::execution::CancellationCause),
}

#[derive(Clone, Debug)]
pub enum RunFailure {
    PreparationRejected { origin: etas_core::Span },
    RestoreRejected { origin: etas_core::Span },
    Language(Box<Diagnostic>),
    ExecutionFault(crate::control::ExecutionFault),
}

/// Infrastructure failures cannot assert that local work has terminated.
#[derive(Debug)]
pub enum RunInfrastructureError {
    Lifecycle(etas_host::HostError),
    RuntimeInitialization(std::io::Error),
}

impl std::fmt::Display for RunInfrastructureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Lifecycle(error) => write!(f, "execution lifecycle: {error}"),
            Self::RuntimeInitialization(error) => write!(f, "execution runtime: {error}"),
        }
    }
}

impl std::error::Error for RunInfrastructureError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Lifecycle(error) => Some(error),
            Self::RuntimeInitialization(error) => Some(error),
        }
    }
}

impl From<etas_host::HostError> for RunInfrastructureError {
    fn from(error: etas_host::HostError) -> Self {
        Self::Lifecycle(error)
    }
}

impl RunResult {
    pub fn value(&self) -> Option<&InterpValue> {
        match &self.outcome {
            RunOutcome::Completed(value) => Some(value),
            _ => None,
        }
    }
}
