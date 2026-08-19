use std::{fmt, path::PathBuf};

use etas_core::{Diagnostic, SourceId, Span, TextRange, TextSize};
use etas_hir::{HirBlockId, HirExprId, HirItemId, HirPatId, HirTypeId, ScopeId, SymbolId};
use etas_host::{
    ActionArgPattern, ActionInstance, ActionPattern, ApprovalGrant, AuthorityContext, Budget,
    CommandPolicy, CostBudget, DestructiveOpPolicy, ExecutionBudget, ExecutionBudgetSnapshot,
    FilesystemPolicy, HostActionGrant, HostJsonValue, HostRequestId, HostValue, ModelContent,
    ModelMessage, ModelName, ModelProviderId, ModelRequest, ModelRole, ModelToolCall,
    NetworkEndpoint, NetworkPolicy, PolicyContext, SandboxMode, SandboxPolicy, TimeBudget,
    TokenBudget, TraceContext, TraceId, TraceSpanId, WorkspaceRoot,
};
use etas_types::TypeId;
use serde_json::{Value, json};

use crate::{
    api::{HostExecutionContext, RunResult},
    orchestration::{
        ActiveHandlerArmRecord, ActiveHandlerRecord, CheckpointCompilationIdentity, CheckpointId,
        CompletedHostBoundary, CompletedHostBoundaryResult, ContinuationSnapshot, HandlerScopeId,
        HandlerSnapshot, HostBoundaryLedger, InterpreterCheckpoint, MachineFrameSnapshot,
        MachineSnapshot, ModelLoopFrameSnapshot, ResourceVersionRecord, ResourceVersionSnapshot,
        RetryAttemptId, RetryAttemptRecord, RetrySnapshot, SourceToolReturnFrameSnapshot,
        TraceSnapshot, WorkflowEvent,
    },
    value::{InterpValue, codec as value_codec},
};

mod checkpoint;
mod json_helpers;
mod machine;
mod report;
mod value;

use checkpoint::*;
pub(crate) use checkpoint::{budget_from_json, budget_json};
pub use checkpoint::{
    checkpoint_artifact_json, checkpoint_from_json, checkpoint_id,
    sources_and_flow_from_checkpoint_json,
};
use json_helpers::*;
pub use report::run_report_json;
pub use value::value_json;
use value::*;
pub(crate) use value::{host_value_from_json, host_value_json, value_from_json};

#[cfg(test)]
mod tests;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InterpreterCodecError {
    message: String,
}

impl InterpreterCodecError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for InterpreterCodecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for InterpreterCodecError {}
