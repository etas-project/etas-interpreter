use super::*;
use crate::{eval::EvalContext, value::conversation::protocol};
use etas_types::Type;

impl EvalContext<'_> {
    pub(in crate::eval) fn resolve_tool_invocation(
        &mut self,
        invocation: &ToolInvocation,
    ) -> Result<ResolvedToolInvocation, ToolInvocationError> {
        let reject = |message: String| {
            ToolInvocationError::Rejected(ToolRejection {
                code: "invalid_tool_call",
                message,
            })
        };
        let missing = |message: &str| {
            ToolInvocationError::Fault(crate::control::ExecutionFault::new(
                etas_core::AnalysisDiagnosticCode::MissingCheckedFact,
                invocation.span,
                message,
            ))
        };
        if !std::sync::Arc::ptr_eq(&invocation.context.program.checked, self.checked) {
            return Err(missing(
                "tool context belongs to a different checked program",
            ));
        }
        let Some(Type::Applied { args, .. }) =
            self.checked.type_store.get(invocation.call.result_type)
        else {
            return Err(missing("invoke requires its checked ToolResult<C> ABI"));
        };
        if args.len() != 1 || !self.agent_context_matches(args[0], invocation.context.method) {
            return Err(missing(
                "invoke context differs from the active checked agent method",
            ));
        }
        let field =
            |value, name| protocol::field(value, name).map_err(|error| reject(error.message));
        let text = |value: &InterpValue| match value {
            InterpValue::String(text) => Ok(text.to_string()),
            _ => Err(reject("tool protocol identity is not text".into())),
        };
        let call_id = text(field(&invocation.input, "id")?)?;
        if call_id.is_empty() {
            return Err(reject("tool call ID must be nonempty".into()));
        }
        let tool = field(&invocation.input, "tool")?;
        let identity = text(field(tool, "id")?)?;
        let revision = text(field(tool, "revision")?)?;
        let args =
            crate::eval::interp_to_host_value(field(&invocation.input, "args")?).map_err(reject)?;
        self.storage_limits
            .result_value_size(&args)
            .map_err(|error| ToolRejection {
                code: "resource_limit",
                message: error.message,
            })?;
        let agent = self
            .checked
            .types
            .agent_methods
            .get(&invocation.context.method)
            .ok_or_else(|| missing("invoke has no checked method identity"))?
            .agent;
        for binding in self
            .agent_tool_surface(agent)
            .map_err(ToolInvocationError::Fault)?
        {
            let (id, current) = self
                .canonical_tool_identity(binding.symbol)
                .map_err(|message| missing(&message))?;
            if id != identity {
                continue;
            }
            if current != revision {
                return Err(ToolRejection {
                    code: "stale_tool_revision",
                    message: "tool implementation revision is no longer registered".into(),
                }
                .into());
            }
            schema::validate_host_value_schema(&args, &binding.schema.input, "args").map_err(
                |message| ToolRejection {
                    code: "invalid_arguments",
                    message,
                },
            )?;
            return Ok(ResolvedToolInvocation {
                binding,
                call_id,
                args,
                identity,
                revision,
            });
        }
        Err(ToolRejection {
            code: "tool_not_exposed",
            message: "tool is not exposed by the active checked agent method".into(),
        }
        .into())
    }
}
