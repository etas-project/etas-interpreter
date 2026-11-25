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

impl ToolInvocation {
    fn parts(&self) -> Result<(&InterpValue, &InterpValue, &InterpValue), String> {
        use crate::value::conversation::protocol::field;
        if matches!(
            self.call.origin,
            crate::control::AgentOperationOrigin::SourceTool(_)
        ) {
            if let InterpValue::Tuple(values) = &self.input
                && let [tool, args, id] = values.as_slice()
            {
                return Ok((tool, args, id));
            }
            return Err("Tool.call arguments differ from their checked ABI".into());
        }
        let get = |name| field(&self.input, name).map_err(|error| error.message);
        Ok((get("tool")?, get("args")?, get("id")?))
    }
}

pub(in crate::eval) struct ResolvedToolInvocation {
    pub binding: NativeToolBinding,
    pub call_id: String,
    pub args: etas_host::HostValue,
    pub identity: String,
    pub revision: String,
    pub member_state: Option<InterpValue>,
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
