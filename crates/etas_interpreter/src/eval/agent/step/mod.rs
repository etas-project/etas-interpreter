use super::*;
use crate::intrinsic::dispatch::CheckedStdIntrinsicCall;
use etas_host::{HostValue, ModelAttemptPolicy};

mod messages;
mod result;

impl EvalContext<'_> {
    pub(in crate::eval) fn execute_agent_step(
        &mut self,
        call: &CheckedStdIntrinsicCall,
        args: Vec<InterpValue>,
        span: Span,
    ) -> ControlSignal {
        self.execute_checked_agent_step(
            &crate::control::CheckedAgentOperation::standard(call),
            args,
            span,
        )
    }

    pub(in crate::eval) fn execute_checked_agent_step(
        &mut self,
        call: &crate::control::CheckedAgentOperation,
        args: Vec<InterpValue>,
        span: Span,
    ) -> ControlSignal {
        match self.prepare_agent_step(call, args, span) {
            Ok(model) => ControlSignal::pending_model(model),
            Err(fault) => ControlSignal::Fault(Box::new(fault)),
        }
    }

    fn prepare_agent_step(
        &mut self,
        call: &crate::control::CheckedAgentOperation,
        args: Vec<InterpValue>,
        span: Span,
    ) -> Result<PendingModel, ExecutionFault> {
        let missing =
            |reason| ExecutionFault::new(AnalysisDiagnosticCode::MissingCheckedFact, span, reason);
        let Some(context) = self.task.agent_context.clone() else {
            return Err(missing(
                "step requires an active checked agent request context",
            ));
        };
        let Some(Type::Applied { args: types, .. }) = self.checked.type_store.get(call.result_type)
        else {
            return Err(missing("step has no checked StepResult<C, O> result"));
        };
        let [context_type, output] = types.as_slice() else {
            return Err(missing("step has no checked context/output instantiation"));
        };
        let output = *output;
        if !self.agent_context_matches(*context_type, context.method)
            || !std::sync::Arc::ptr_eq(&context.program.checked, self.checked)
        {
            return Err(missing(
                "step context does not match the active checked agent method",
            ));
        }
        let [
            InterpValue::Conversation(conversation),
            InterpValue::Schema(schema),
        ]: [InterpValue; 2] = args
            .try_into()
            .map_err(|_| missing("step requires Conversation and Schema<O>"))?
        else {
            return Err(missing("step arguments disagree with the checked ABI"));
        };
        if schema != output {
            return Err(missing("step output and supplied schema disagree"));
        }
        let method = self
            .checked
            .types
            .agent_methods
            .get(&context.method)
            .ok_or_else(|| missing("step context has no checked agent method"))?;
        let agent = method.agent;
        let mut policy = context.policy.clone();
        // A runtime model profile does not expand this method's declared tool surface.
        policy.tools.clear();
        policy.tool_choice = ModelToolChoice::Auto;
        if let Some(model) = &self.locked_model {
            policy.model = model.clone();
            policy.model_locked = true;
        }
        let mut native_tools = Vec::new();
        let mut trace_plan = Vec::new();
        self.apply_agent_annotations(agent, &mut policy, &mut native_tools, &mut trace_plan)?;
        if matches!(
            call.origin,
            crate::control::AgentOperationOrigin::Controller { .. }
        ) {
            policy.tools = context.policy.tools.clone();
            policy.tool_choice = context.policy.tool_choice.clone();
        }
        let messages = self.step_messages(&conversation, &native_tools, span)?;
        let response_schema = if self.type_is_string(output) {
            None
        } else {
            Some(self.host_schema_for_type(output, span, 0)?)
        };
        self.model_provider_supports_request(&policy, response_schema.is_some(), span)?;
        let budget = match policy.budget.clone() {
            Some(limits) => self.host_budget().with_limits(limits).map_err(|error| {
                ExecutionFault::new(
                    AnalysisDiagnosticCode::InvalidArguments,
                    span,
                    error.to_string(),
                )
            })?,
            None => self.host_budget(),
        };
        let mut options = crate::orchestration::bounded_model_options(policy.options, &budget);
        options.attempt_policy = ModelAttemptPolicy::Single;
        Ok(PendingModel {
            request: ModelRequest {
                action: etas_host::ModelAction::Step,
                id: self.next_host_request_id(),
                provider: policy.provider,
                model: policy.model,
                messages,
                tools: policy.tools,
                tool_choice: policy.tool_choice,
                response_schema,
                policy_ref: policy.policy_ref,
                options,
                authority: self.host_authority(),
                trace: self.host_trace(),
                budget,
            },
            decode: ModelDecode::Step {
                result: call.result_type,
                output,
            },
            span,
            continuation: Continuation::BlockValue,
        })
    }
}

fn variant(name: &str, fields: Vec<HostValue>) -> HostValue {
    HostValue::Variant {
        name: name.into(),
        fields,
    }
}

fn option(value: Option<HostValue>) -> HostValue {
    match value {
        Some(value) => variant("Some", vec![value]),
        None => variant("None", Vec::new()),
    }
}
