mod invoke;
mod resolve;
pub(in crate::eval) mod schema;

use crate::{control::AgentRequestContext, value::InterpValue};

#[derive(Clone, Debug)]
pub struct ToolInvocation {
    pub(crate) call: crate::control::CheckedAgentOperation,
    pub(crate) input: InterpValue,
    pub(crate) context: Box<AgentRequestContext>,
    pub(crate) request: etas_host::HostRequestId,
    pub(crate) span: etas_core::Span,
}

pub(in crate::eval) struct ResolvedToolInvocation {
    pub binding: NativeToolBinding,
    pub call_id: String,
    pub args: etas_host::HostValue,
    pub identity: String,
    pub revision: String,
}

#[derive(Debug)]
pub(in crate::eval) struct ToolRejection {
    pub code: &'static str,
    pub message: String,
}

#[derive(Debug)]
pub(in crate::eval) enum ToolInvocationError {
    Rejected(ToolRejection),
    Fault(crate::control::ExecutionFault),
}

impl From<ToolRejection> for ToolInvocationError {
    fn from(value: ToolRejection) -> Self {
        Self::Rejected(value)
    }
}

#[derive(Clone, Debug)]
pub(crate) struct NativeToolBinding {
    pub symbol: etas_hir::SymbolId,
    pub schema: etas_host::ToolSchema,
    pub source: Option<crate::control::SourceToolBinding>,
    pub output: etas_types::TypeId,
}
