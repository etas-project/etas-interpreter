use super::*;
use crate::{eval::EvalContext, value::conversation::protocol};

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
        let context_type = invocation
            .call
            .tool_context(self.checked)
            .ok_or_else(|| missing("invoke requires its checked agent context"))?;
        if !self.agent_context_matches(context_type, invocation.context.method) {
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
        let (tool, args, id) = invocation.parts().map_err(reject)?;
        let call_id = text(id)?;
        if call_id.is_empty() {
            return Err(reject("tool call ID must be nonempty".into()));
        }
        let identity = text(field(tool, "id")?)?;
        let revision = text(field(tool, "revision")?)?;
        let args = crate::eval::interp_to_host_value(args).map_err(reject)?;
        self.storage_limits
            .result_value_size(&args)
            .map_err(|error| ToolRejection {
                code: "resource_limit",
                message: error.message,
            })?;
        let method = self
            .checked
            .types
            .agent_methods
            .get(&invocation.context.method)
            .ok_or_else(|| missing("invoke has no checked method identity"))?;
        for binding in self
            .agent_tool_surface(method.agent, method.body)
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
            let member_state = if let Some(source) = &binding.source
                && self.checked.hir.member_tool_owner(source.item).is_some()
            {
                let record = match tool {
                    InterpValue::Nominal { value, .. } => value.as_ref(),
                    value => value,
                };
                let snapshot = match record {
                    InterpValue::Record(record) => record.member_tool_snapshot(),
                    _ => None,
                };
                let snapshot = snapshot
                    .filter(|snapshot| {
                        snapshot.request == self.task.logical_identity
                            && snapshot.calls.get(&call_id) == Some(&binding.symbol)
                            && self
                                .task
                                .agent_turn
                                .as_ref()
                                .is_some_and(|turn| turn.reference == snapshot.instance)
                    })
                    .ok_or_else(|| ToolRejection {
                        code: "tool_not_exposed",
                        message: "member tool call requires a snapshot from this Agent request"
                            .into(),
                    })?;
                self.checked
                    .hir
                    .member_tool_receiver(source.item)
                    .map(|_| snapshot.state.clone())
            } else {
                None
            };
            return Ok(ResolvedToolInvocation {
                binding,
                call_id,
                args,
                identity,
                revision,
                member_state,
            });
        }
        Err(ToolRejection {
            code: "tool_not_exposed",
            message: "tool is not exposed by the active checked agent method".into(),
        }
        .into())
    }
}
