use super::*;
mod repair;
use crate::{
    control::{
        AgentOperationKind, CheckedActionInvocation, CheckedAgentOperation, DefaultActionBoundary,
    },
    value::{
        ArrayValue, ConversationValue,
        conversation::{self, protocol},
    },
};
use repair::{ModelRepairKind, ModelRepairPolicy};

#[derive(Clone, Debug)]
pub struct AgentController {
    pub(crate) inference: HirExprId,
    pub(crate) conversation: ConversationValue,
    pub(crate) calls: ArrayValue,
    pub(crate) next_call: usize,
    pub(crate) results: Vec<InterpValue>,
    pub(crate) round: usize,
    pub(crate) repairs: usize,
    pub(crate) max_tool_rounds: usize,
    pub(crate) span: Span,
}

impl EvalContext<'_> {
    pub(in crate::eval) fn start_default_agent_controller(
        &mut self,
        inference: HirExprId,
        perform: &PendingPerform,
    ) -> ControlSignal {
        let span = perform.span;
        let [InterpValue::Prompt(prompt), InterpValue::Schema(_)] = perform.args.as_slice() else {
            return ControlSignal::missing_checked_fact(
                "default infer requires its checked Prompt/Schema payload",
                span,
            );
        };
        let Some(mut request) = perform.inference.clone() else {
            return ControlSignal::missing_checked_fact(
                "default infer requires its scoped agent request",
                span,
            );
        };
        let Some(method) = self.checked.types.agent_methods.get(&request.method) else {
            return ControlSignal::missing_checked_fact(
                "default infer has no checked method",
                span,
            );
        };
        let mut tools = Vec::new();
        let mut trace = Vec::new();
        request.policy.tools.clear();
        request.policy.tool_choice = ModelToolChoice::Auto;
        if let Err(fault) =
            self.apply_agent_annotations(method.agent, &mut request.policy, &mut tools, &mut trace)
        {
            return ControlSignal::Fault(Box::new(fault));
        }
        if !trace.is_empty() {
            self.events
                .push(crate::orchestration::WorkflowEvent::AgentTracePlan {
                    item: method.agent.0,
                    trace,
                });
        }
        let history = match conversation::from_prompt(prompt.clone(), &self.storage_limits) {
            Ok(history) => history,
            Err(error) => return ControlSignal::invalid_arguments(error.message, span),
        };
        let previous = self.enter_agent_context(request);
        let state = Box::new(AgentController {
            inference,
            conversation: history,
            calls: ArrayValue::new(Vec::new()),
            next_call: 0,
            results: Vec::new(),
            round: 0,
            repairs: 0,
            max_tool_rounds: self.task.model_policy.max_tool_rounds,
            span,
        });
        let signal = self.agent_controller_next_step(state);
        compose_signal_continuation(
            signal,
            Continuation::Chain {
                inner: Continuation::AgentExecutionState(Box::new(previous)).into(),
                outer: perform.continuation.clone().into(),
            },
        )
    }

    fn agent_controller_next_step(&mut self, state: Box<AgentController>) -> ControlSignal {
        let span = state.span;
        let proof = match CheckedActionInvocation::controller_step(
            self.checked_execution_program(),
            state.inference,
        ) {
            Ok(proof) => proof,
            Err(error) => return ControlSignal::missing_checked_fact(error, span),
        };
        let history = InterpValue::Conversation(state.conversation.clone());
        let Some(fact) = self.checked.types.agent_inferences.get(&state.inference) else {
            return ControlSignal::missing_checked_fact(
                "controller origin has no checked inference",
                span,
            );
        };
        let boundary = DefaultActionBoundary::AgentControllerStep {
            inference: state.inference,
            conversation: history.clone(),
            span,
        };
        let signal = self.mediate_native_action(
            proof,
            vec![history, InterpValue::Schema(fact.output)],
            boundary,
            span,
        );
        compose_signal_continuation(signal, Continuation::AgentControllerStep(state))
    }

    pub(in crate::eval) fn resume_agent_controller_step(
        &mut self,
        mut state: Box<AgentController>,
        step: InterpValue,
    ) -> ControlSignal {
        state.conversation =
            match conversation::append_step(state.conversation, &step, &self.storage_limits) {
                Ok(history) => history,
                Err(error) => return ControlSignal::invalid_arguments(error.message, state.span),
            };
        let decision = match protocol::field(&step, "decision") {
            Ok(value) => representation(value),
            Err(error) => return ControlSignal::invalid_arguments(error.message, state.span),
        };
        let InterpValue::Variant { name, fields } = decision else {
            return ControlSignal::missing_checked_fact(
                "step returned an invalid decision ABI",
                state.span,
            );
        };
        match (name.as_str(), fields.as_slice()) {
            ("Final", [answer]) => {
                if state.round == 0
                    && !matches!(self.task.model_policy.tool_choice, ModelToolChoice::Auto)
                {
                    self.agent_controller_repair(
                        state,
                        ModelRepairKind::RequiredToolChoice,
                        "required tool was not called",
                    )
                } else {
                    ControlSignal::Value(answer.clone())
                }
            }
            ("CallTools", [InterpValue::Array(calls)]) => {
                if state.round >= state.max_tool_rounds {
                    return ControlSignal::runtime_fault(
                        "model requested tools after the configured tool-round limit",
                        state.span,
                    );
                }
                state.round += 1;
                state.calls = calls.clone();
                state.next_call = 0;
                state.results.clear();
                self.agent_controller_next_tool(state)
            }
            ("Stopped", [reason]) => {
                if let InterpValue::Variant { name, fields } = representation(reason) {
                    if matches!(name.as_str(), "InvalidOutput" | "InvalidToolCall") {
                        let Some(diagnostic) = fields.first() else {
                            return ControlSignal::missing_checked_fact(
                                "stopped generation has no diagnostic",
                                state.span,
                            );
                        };
                        let Ok(InterpValue::String(message)) =
                            protocol::field(diagnostic, "message")
                        else {
                            return ControlSignal::missing_checked_fact(
                                "stopped generation has no diagnostic message",
                                state.span,
                            );
                        };
                        let kind = if name == "InvalidToolCall" {
                            ModelRepairKind::ToolCall
                        } else {
                            ModelRepairKind::TypedOutput
                        };
                        return self.agent_controller_repair(state, kind, message);
                    }
                }
                let message = match representation(reason) {
                    InterpValue::Variant { name, fields } => match (name.as_str(), fields.as_slice()) {
                        ("Truncated", _) => "model generation failed: IncompleteResponse: model generation was truncated".into(),
                        ("Refused", [InterpValue::String(reason)]) => format!("model generation failed: ProviderRejected: {reason}"),
                        (_, [diagnostic]) => match protocol::field(diagnostic, "message") {
                            Ok(InterpValue::String(message)) => message.to_string(),
                            _ => "model step stopped without a final answer".into(),
                        },
                        _ => "model step stopped without a final answer".into(),
                    },
                    _ => "model step stopped without a final answer".into(),
                };
                ControlSignal::runtime_fault(message, state.span)
            }
            _ => ControlSignal::missing_checked_fact(
                "step decision differs from its checked protocol",
                state.span,
            ),
        }
    }

    fn agent_controller_next_tool(&mut self, mut state: Box<AgentController>) -> ControlSignal {
        let Some(call) = state.calls.borrow().get(state.next_call).cloned() else {
            let results = ArrayValue::new(std::mem::take(&mut state.results));
            state.conversation = match conversation::append_results(
                state.conversation,
                &results,
                &self.storage_limits,
            ) {
                Ok(history) => history,
                Err(error) => return ControlSignal::invalid_arguments(error.message, state.span),
            };
            if let Some(context) = &mut self.task.agent_context {
                if !matches!(context.policy.tool_choice, ModelToolChoice::Auto) {
                    context.policy.tool_choice = ModelToolChoice::Auto;
                    context.policy.tools.clear();
                }
                if state.round >= state.max_tool_rounds {
                    context.policy.tools.clear();
                }
            }
            return self.agent_controller_next_step(state);
        };
        state.next_call += 1;
        let operation = match CheckedAgentOperation::controller(
            self.checked,
            state.inference,
            AgentOperationKind::Invoke,
        ) {
            Ok(operation) => operation,
            Err(error) => return ControlSignal::missing_checked_fact(error, state.span),
        };
        let signal = self.execute_checked_tool_invoke(operation, vec![call], state.span);
        compose_signal_continuation(signal, Continuation::AgentControllerTool(state))
    }

    pub(in crate::eval) fn resume_agent_controller_tool(
        &mut self,
        mut state: Box<AgentController>,
        result: InterpValue,
    ) -> ControlSignal {
        state.results.push(result);
        self.agent_controller_next_tool(state)
    }

    fn agent_controller_repair(
        &mut self,
        mut state: Box<AgentController>,
        kind: ModelRepairKind,
        reason: &str,
    ) -> ControlSignal {
        let policy = ModelRepairPolicy::new(state.max_tool_rounds);
        let repair = match kind {
            ModelRepairKind::TypedOutput => policy.typed_output(state.repairs, reason),
            ModelRepairKind::RequiredToolChoice => {
                policy.required_tool_choice(state.repairs, &self.task.model_policy.tool_choice)
            }
            ModelRepairKind::ToolCall => policy.tool_call(state.repairs, reason),
        };
        let repair = match repair {
            Ok(repair) => repair,
            Err(error) => {
                self.events
                    .push(crate::orchestration::WorkflowEvent::ModelRepairExhausted {
                        kind: error.kind.as_str().into(),
                        attempts: error.attempts,
                        reason: error.reason.clone(),
                    });
                return ControlSignal::fault(
                    AnalysisDiagnosticCode::InvalidArguments,
                    state.span,
                    format!("model repair exhausted: {}", error.reason),
                );
            }
        };
        state.repairs = repair.attempt;
        self.events
            .push(crate::orchestration::WorkflowEvent::ModelRepairAttempted {
                kind: repair.kind.as_str().into(),
                attempt: repair.attempt,
                reason: repair.reason,
            });
        let prompt = repair
            .message
            .content
            .into_iter()
            .map(|content| match content {
                ModelContent::Text(text) => crate::value::PromptMessage {
                    role: crate::value::PromptRole::User,
                    text: text.into(),
                    trust: Some(etas_types::TrustWrapper::Trusted),
                },
                ModelContent::Value(_) => {
                    unreachable!("controller repair policy emits a text instruction")
                }
            })
            .collect();
        state.conversation =
            match conversation::append_prompt(state.conversation, prompt, &self.storage_limits) {
                Ok(history) => history,
                Err(error) => return ControlSignal::invalid_arguments(error.message, state.span),
            };
        self.agent_controller_next_step(state)
    }
}

fn representation(value: &InterpValue) -> &InterpValue {
    match value {
        InterpValue::Nominal { value, .. } => value.as_ref(),
        value => value,
    }
}
