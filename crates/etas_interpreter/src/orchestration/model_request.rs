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

    pub(crate) fn restore(self, current: &crate::api::HostExecutionContext) -> ModelRequest {
        ModelRequest {
            id: self.id,
            provider: self.provider,
            model: self.model,
            messages: self.messages,
            tools: self.tools,
            tool_choice: self.tool_choice,
            response_schema: self.response_schema,
            policy_ref: self.policy_ref,
            options: self.options,
            authority: current.authority.clone(),
            trace: current.trace.clone(),
            // Scoped limits remain bounded by the current run-owned ledger.
            budget: current.budget.with_limits(self.budget_limits),
        }
    }
}
