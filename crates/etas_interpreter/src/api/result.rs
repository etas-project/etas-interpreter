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
    pub value: Option<InterpValue>,
    pub diagnostics: Vec<Diagnostic>,
    pub events: Vec<WorkflowEvent>,
    pub checkpoints: Vec<InterpreterCheckpoint>,
}
