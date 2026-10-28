use super::*;

impl EvalContext<'_> {
    pub(in crate::eval) fn eval_agent_inference(
        &mut self,
        expr: HirExprId,
        frame: &mut Frame,
        span: Span,
    ) -> ControlSignal {
        let Some(fact) = self.checked.types.agent_inferences.get(&expr) else {
            return ControlSignal::missing_checked_fact(
                "inference has no checked agent/prompt/output fact",
                span,
            );
        };
        let signal = self.eval_expr(fact.prompt, frame);
        compose_signal_continuation(
            signal,
            Continuation::AgentInference {
                expr,
                span,
                model_policy: Box::new(self.model_policy.clone()),
            },
        )
    }

    pub(crate) fn perform_agent_inference(
        &mut self,
        expr: HirExprId,
        value: InterpValue,
        span: Span,
    ) -> ControlSignal {
        let Some(fact) = self.checked.types.agent_inferences.get(&expr) else {
            return ControlSignal::missing_checked_fact(
                "inference has no checked owner/output fact",
                span,
            );
        };
        let Some(HirExpr::Infer { action, .. }) = self.checked.hir.exprs.get(expr) else {
            return ControlSignal::missing_checked_fact(
                "inference fact does not reference an inference expression",
                span,
            );
        };
        if !matches!(value, InterpValue::Prompt(_)) {
            return ControlSignal::invalid_arguments(
                "checked inference operand is not a Prompt",
                span,
            );
        }
        let mut pending = PendingPerform {
            checked_action: None,
            default_boundary: None,
            forwarded_call: None,
            expr: Some(expr),
            action: action.clone(),
            error_type: None,
            args: vec![value, InterpValue::Schema(fact.output)],
            inference: Some(Box::new(self.model_policy.clone())),
            span,
            continuation: Continuation::BlockValue,
        };
        let proof = match self.inference_action_invocation(&pending, fact) {
            Ok(proof) => proof,
            Err(reason) => return ControlSignal::missing_checked_fact(reason, span),
        };
        pending.checked_action = Some(proof);
        ControlSignal::pending_perform(pending)
    }

    fn inference_action_invocation(
        &self,
        pending: &PendingPerform,
        fact: &etas_types::facts::inference::CheckedAgentInferenceFact,
    ) -> Result<crate::control::CheckedActionInvocation, String> {
        if !matches!(pending.action.action_symbol, ResolveResult::Resolved(symbol) if symbol == fact.action_symbol)
            || self.checked.type_store.get(fact.schema)
                != Some(&etas_types::Type::Schema(fact.output))
        {
            return Err("inference invocation disagrees with its checked schema or action".into());
        }
        let signature = self
            .checked
            .types
            .action_signatures
            .get(&fact.action_symbol)
            .ok_or("inference has no checked action signature")?;
        let [output] = signature.generic_params.as_slice() else {
            return Err("inference action has no single checked output parameter".into());
        };
        if output.kind != etas_types::CallableGenericParamKind::Type
            || signature.output != output.subject
        {
            return Err("inference output is not its checked action type parameter".into());
        }
        let bindings = std::collections::HashMap::from([(output.name.clone(), fact.output)]);
        let proof = self.action_invocation_with_bindings(pending, &bindings)?;
        let [prompt, schema] = proof.parameters.as_slice() else {
            return Err("inference action has no checked Prompt/Schema payload contract".into());
        };
        if self.checked.type_store.get(*prompt) != Some(&etas_types::Type::Prompt)
            || *schema != fact.schema
            || proof.output != fact.output
        {
            return Err("inference payload/output types disagree with its checked facts".into());
        }
        Ok(proof)
    }

    pub(crate) fn default_agent_inference(
        &mut self,
        perform: &PendingPerform,
    ) -> Option<ControlSignal> {
        let expr = perform.expr?;
        if !matches!(
            self.checked.hir.exprs.get(expr),
            Some(HirExpr::Infer { .. })
        ) {
            return None;
        }
        Some(self.start_agent_model_request(expr, perform))
    }

    fn start_agent_model_request(
        &mut self,
        expr: HirExprId,
        perform: &PendingPerform,
    ) -> ControlSignal {
        let span = perform.span;
        let Some(fact) = self.checked.types.agent_inferences.get(&expr) else {
            return ControlSignal::missing_checked_fact(
                "default inference requires checked owner/output facts",
                span,
            );
        };
        let Some(HirExpr::Infer { action, .. }) = self.checked.hir.exprs.get(expr) else {
            return ControlSignal::missing_checked_fact(
                "default inference requires a matching HIR action",
                span,
            );
        };
        if action != &perform.action
            || !matches!(action.action_symbol, ResolveResult::Resolved(symbol) if symbol == fact.action_symbol)
        {
            return ControlSignal::missing_checked_fact(
                "inference action identity does not match its checked fact",
                span,
            );
        }
        let [InterpValue::Prompt(prompt), InterpValue::Schema(output)] = perform.args.as_slice()
        else {
            return ControlSignal::invalid_arguments(
                "inference action requires Prompt and checked Schema arguments",
                span,
            );
        };
        if *output != fact.output {
            return ControlSignal::missing_checked_fact(
                "inference schema does not match its checked output type",
                span,
            );
        }
        let Some(method) = self.checked.types.agent_methods.get(&fact.method) else {
            return ControlSignal::missing_checked_fact(
                "inference requires a checked agent method",
                span,
            );
        };
        if method.agent != fact.agent || method.body != fact.owner {
            return ControlSignal::missing_checked_fact(
                "inference method identity is inconsistent",
                span,
            );
        }
        let item = fact.agent;
        let output_type = fact.output;
        let prompt = prompt.clone();
        let Some(policy) = &perform.inference else {
            return ControlSignal::missing_checked_fact(
                "inference action is missing its scoped model context",
                span,
            );
        };
        let mut model_policy = (**policy).clone();
        if let Some(model) = &self.locked_model {
            model_policy.model = model.clone();
            model_policy.model_locked = true;
        }
        let mut source_tools = Vec::new();
        let mut trace_plan = Vec::new();
        if let Err(fault) = self.apply_agent_annotations(
            item,
            &mut model_policy,
            &mut source_tools,
            &mut trace_plan,
        ) {
            return ControlSignal::Fault(Box::new(fault));
        }
        if !trace_plan.is_empty() {
            self.events
                .push(crate::orchestration::WorkflowEvent::AgentTracePlan {
                    item: item.0,
                    trace: trace_plan,
                });
        }

        let decode = self.agent_model_decode(output_type);
        let response_schema = match decode {
            ModelDecode::Typed(expected) => {
                let schema = match self.host_schema_for_type(expected, span, 0) {
                    Ok(schema) => schema,
                    Err(fault) => return ControlSignal::Fault(Box::new(fault)),
                };
                Some(schema)
            }
            ModelDecode::String | ModelDecode::ModelResponse => None,
        };
        if let Err(fault) =
            self.model_provider_supports_request(&model_policy, response_schema.is_some(), span)
        {
            return ControlSignal::Fault(Box::new(fault));
        }

        let budget = match model_policy.budget.clone() {
            Some(limits) => match self.host_budget().with_limits(limits) {
                Ok(budget) => budget,
                Err(error) => {
                    return ControlSignal::invalid_arguments(
                        format!("incompatible model budget limits: {error}"),
                        span,
                    );
                }
            },
            None => self.host_budget(),
        };
        let request_id = HostRequestId(self.next_host_request);
        self.next_host_request += 1;
        ControlSignal::pending_model(PendingModel {
            request: ModelRequest {
                id: request_id,
                provider: model_policy.provider.clone(),
                model: model_policy.model.clone(),
                messages: prompt
                    .into_iter()
                    .map(|message| ModelMessage {
                        role: match message.role {
                            crate::value::PromptRole::System => ModelRole::System,
                            crate::value::PromptRole::User | crate::value::PromptRole::Data => {
                                ModelRole::User
                            }
                            crate::value::PromptRole::Assistant => ModelRole::Assistant,
                        },
                        content: vec![ModelContent::Text(message.text.into_string())],
                        tool_call_id: None,
                        tool_calls: Vec::new(),
                    })
                    .collect(),
                tools: model_policy.tools.clone(),
                tool_choice: model_policy.tool_choice.clone(),
                response_schema,
                policy_ref: model_policy.policy_ref.clone(),
                options: crate::orchestration::bounded_model_options(
                    model_policy.options.clone(),
                    &budget,
                ),
                authority: self.host_authority(),
                trace: self.host_trace(),
                budget,
            },
            decode,
            max_tool_rounds: model_policy.max_tool_rounds,
            source_tools,
            span,
            continuation: perform.continuation.clone(),
        })
    }
}
