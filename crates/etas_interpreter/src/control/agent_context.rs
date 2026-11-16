use crate::{api::ModelExecutionPolicy, value::CheckedExecutionProgram};

/// Engine-owned invocation context, never reconstructed from protocol records.
#[derive(Clone, Debug)]
pub struct AgentRequestContext {
    pub(crate) program: CheckedExecutionProgram,
    pub(crate) method: etas_hir::SymbolId,
    pub(crate) request: etas_host::HostRequestId,
    pub(crate) policy: ModelExecutionPolicy,
}

#[derive(Clone, Debug)]
pub struct AgentExecutionState {
    pub(crate) request: Option<Box<AgentRequestContext>>,
    pub(crate) policy: ModelExecutionPolicy,
}
