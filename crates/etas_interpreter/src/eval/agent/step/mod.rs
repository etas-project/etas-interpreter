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
        let controller_round = matches!(
            call.origin,
            crate::control::AgentOperationOrigin::Controller { .. }
        );
        self.start_agent_step(call.result_type, args, controller_round, span)
    }

    pub(in crate::eval) fn source_agent_step(
        &mut self,
        perform: &PendingPerform,
    ) -> Option<ControlSignal> {
        if !self.source_compiler_default(perform, etas_std::CompilerActionRole::AgentStep) {
            return None;
        }
        let proof = perform
            .checked_action
            .as_ref()
            .expect("checked source step");
        let signal = self.start_agent_step(proof.output, perform.args.clone(), false, perform.span);
        Some(self.continue_chain_signal(signal, perform.continuation.clone()))
    }

    pub(in crate::eval) fn agent_action_uses_caller_context(
        &self,
        perform: &PendingPerform,
    ) -> bool {
        self.task.agent_context.as_ref().is_some_and(|context| {
            !std::sync::Arc::ptr_eq(&context.program.checked, self.checked)
                && (self.source_compiler_default(perform, etas_std::CompilerActionRole::AgentStep)
                    || self.source_default_signature(perform) == Some(("Tool", "call")))
        })
    }

    fn start_agent_step(
        &mut self,
        result_type: etas_types::TypeId,
        args: Vec<InterpValue>,
        controller_round: bool,
        span: Span,
    ) -> ControlSignal {
        match self.prepare_agent_step(result_type, args, controller_round, span) {
            Ok(model) => ControlSignal::pending_model(model),
            Err(fault) => ControlSignal::Fault(Box::new(fault)),
        }
    }

    fn prepare_agent_step(
        &mut self,
        result_type: etas_types::TypeId,
        args: Vec<InterpValue>,
        controller_round: bool,
        span: Span,
    ) -> Result<PendingModel, ExecutionFault> {
        let missing =
            |reason| ExecutionFault::new(AnalysisDiagnosticCode::MissingCheckedFact, span, reason);
        let Some(context) = self.task.agent_context.clone() else {
            return Err(missing(
                "step requires an active checked agent request context",
            ));
        };
        let Some(Type::Applied { args: types, .. }) = self.checked.type_store.get(result_type)
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
        let [conversation, schema]: [InterpValue; 2] = args
            .try_into()
            .map_err(|_| missing("step requires Conversation and Schema<O>"))?;
        if !crate::value::source_schema::matches_output(self.checked, &schema, output) {
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
        self.apply_agent_annotations(
            agent,
            method.body,
            &mut policy,
            &mut native_tools,
            &mut trace_plan,
        )?;
        if controller_round {
            policy.tools = context.policy.tools.clone();
            policy.tool_choice = context.policy.tool_choice.clone();
        }
        self.constrain_model_policy(&mut policy, span)?;
        let messages = match conversation {
            InterpValue::Conversation(conversation) => {
                self.step_messages(&conversation, &native_tools, span)?
            }
            value => self.source_step_messages(&value, &native_tools, span)?,
        };
        let response_schema = self.step_response_schema(&schema, output, span)?;
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
        let mut options = policy.options;
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
                result: result_type,
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
