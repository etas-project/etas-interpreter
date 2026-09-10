use std::{fmt, path::PathBuf};

use etas_core::{Diagnostic, SourceId, Span, TextRange, TextSize};
use etas_hir::{HirBlockId, HirExprId, HirItemId, HirPatId, HirTypeId, ScopeId, SymbolId};
use etas_host::{
    Budget, CostBudget, ExecutionBudgetSnapshot, HostJsonValue, HostRequestId, HostValue,
    ModelContent, ModelMessage, ModelName, ModelProviderId, ModelRole, ModelToolCall, TimeBudget,
    TokenBudget, TraceContext, TraceId, TraceSpanId,
};
use etas_types::TypeId;
use serde_json::{Value, json};

use crate::{
    api::RunResult,
    orchestration::{
        ActiveHandlerArmRecord, ActiveHandlerRecord, CheckpointBudgetSnapshot,
        CheckpointCompilationIdentity, CheckpointHostState, CheckpointId, CompletedHostBoundary,
        CompletedHostBoundaryResult, ContinuationSnapshot, ExecutionProgressSnapshot,
        HandlerScopeId, HandlerSnapshot, HostBoundaryLedger, InterpreterCheckpoint,
        MachineFrameSnapshot, MachineSnapshot, ResourceVersionRecord, ResourceVersionSnapshot,
        RetryAttemptId, RetryAttemptRecord, RetrySnapshot, TraceSnapshot, WorkflowEvent,
    },
    value::{InterpValue, codec as value_codec},
};

mod checkpoint;
mod json_helpers;
mod machine;
mod report;
mod value;

pub use checkpoint::event_json;
use checkpoint::*;
pub(crate) use checkpoint::{budget_from_json, budget_json};
pub use checkpoint::{
    checkpoint_artifact_json, checkpoint_from_json, checkpoint_from_json_with_limits,
    checkpoint_id, sources_and_flow_from_checkpoint_json,
};
use json_helpers::*;
pub use report::run_report_json;
#[cfg(test)]
pub(crate) use value::value_from_json;
pub use value::value_json;
use value::*;
pub(crate) use value::{host_value_from_json, host_value_json, value_from_json_with_limits};

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
