use etas_host::{
    Budget, HostRequestId, HostSchema, HostValue, ModelMessage, ModelName, ModelOptions,
    ModelProviderId, ModelRequest, ModelToolChoice, ToolSchema,
};

/// Durable request data, never a saved grant or opened workspace binding.
#[derive(Clone, Debug)]
pub(crate) struct ModelRequestSnapshot {
    pub id: HostRequestId,
    pub provider: Option<ModelProviderId>,
    pub model: ModelName,
    pub messages: Vec<ModelMessage>,
    pub tools: Vec<ToolSchema>,
    pub tool_choice: ModelToolChoice,
    pub response_schema: Option<HostSchema>,
    pub policy_ref: Option<HostValue>,
    pub options: ModelOptions,
    pub budget_limits: Budget,
}

impl ModelRequestSnapshot {
    pub(crate) fn capture(request: &ModelRequest) -> Self {
        Self {
            id: request.id,
            provider: request.provider.clone(),
            model: request.model.clone(),
            messages: request.messages.clone(),
            tools: request.tools.clone(),
            tool_choice: request.tool_choice.clone(),
            response_schema: request.response_schema.clone(),
            policy_ref: request.policy_ref.clone(),
            options: request.options.clone(),
            budget_limits: request.budget.limits().clone(),
        }
    }

    pub(crate) fn restore(
        self,
        current: &crate::api::HostExecutionContext,
        locked_model: Option<&ModelName>,
    ) -> Result<ModelRequest, etas_host::HostError> {
        let budget = current.budget.with_limits(self.budget_limits)?;
        Ok(ModelRequest {
            id: self.id,
            provider: self.provider,
            model: locked_model.cloned().unwrap_or(self.model),
            messages: self.messages,
            tools: self.tools,
            tool_choice: self.tool_choice,
            response_schema: self.response_schema,
            policy_ref: self.policy_ref,
            options: bounded_model_options(self.options, &budget),
            authority: current.authority.clone(),
            trace: current.trace.clone(),
            // Scoped limits remain bounded by the current run-owned ledger.
            budget,
        })
    }
}

pub(crate) fn bounded_model_options(
    mut options: ModelOptions,
    budget: &etas_host::ExecutionBudget,
) -> ModelOptions {
    if let Some(tokens) = budget.limits().tokens {
        options.max_output_tokens = Some(
            options
                .max_output_tokens
                .map_or(tokens.max_tokens, |limit| limit.min(tokens.max_tokens)),
        );
    }
    options
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restored_requests_keep_current_authority_model_override_and_budget_ceiling() {
        let current = crate::api::HostExecutionContext {
            budget: etas_host::ExecutionBudget::start(Budget {
                tokens: Some(etas_host::TokenBudget { max_tokens: 7 }),
                ..Default::default()
            }),
            ..Default::default()
        };
        let saved = ModelRequestSnapshot {
            id: HostRequestId(17),
            provider: None,
            model: ModelName("old-model".into()),
            messages: vec![],
            tools: vec![],
            tool_choice: ModelToolChoice::Auto,
            response_schema: None,
            policy_ref: None,
            options: ModelOptions {
                max_output_tokens: Some(23),
                ..Default::default()
            },
            budget_limits: Budget {
                tokens: Some(etas_host::TokenBudget { max_tokens: 23 }),
                ..Default::default()
            },
        };
        let restored = saved
            .restore(&current, Some(&ModelName("current-model".into())))
            .unwrap();
        assert_eq!(restored.id, HostRequestId(17));
        assert_eq!(restored.model.0, "current-model");
        assert_eq!(restored.authority, current.authority);
        assert_eq!(restored.options.max_output_tokens, Some(7));
        assert_eq!(restored.budget.limits(), current.budget.limits());
        let reservation = restored.budget.reserve_tokens(3).unwrap();
        restored.budget.settle_tokens(reservation, 2).unwrap();
        assert_eq!(current.budget.remaining_tokens().unwrap(), Some(5));
    }
}
