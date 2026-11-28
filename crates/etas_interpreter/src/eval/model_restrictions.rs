use etas_host::{HostValue, ModelName, ModelProviderId, PolicyContext};

use crate::{api::ModelExecutionPolicy, control::ExecutionFault};

/// Invocation bounds retained independently from model/tool configuration.
/// Tool schemas describe a method's surface; authority controls whether its
/// callbacks may run and is conjoined separately.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct AgentModelRestrictions {
    locked_model: Option<(Option<ModelProviderId>, ModelName)>,
    max_output_tokens: Option<u64>,
    max_context_tokens: Option<u64>,
    max_tool_rounds: usize,
    pub(crate) policy_ref: Option<HostValue>,
    pub(crate) policy: PolicyContext,
}

impl AgentModelRestrictions {
    pub(crate) fn capture(policy: &ModelExecutionPolicy, context: PolicyContext) -> Self {
        Self {
            locked_model: policy
                .model_locked
                .then(|| (policy.provider.clone(), policy.model.clone())),
            max_output_tokens: policy.options.max_output_tokens,
            max_context_tokens: policy.options.max_context_tokens,
            max_tool_rounds: policy.max_tool_rounds,
            policy_ref: policy.policy_ref.clone(),
            policy: context,
        }
    }

    fn apply(
        &self,
        policy: &mut ModelExecutionPolicy,
        span: etas_core::Span,
    ) -> Result<(), ExecutionFault> {
        if let Some((provider, model)) = &self.locked_model {
            if policy.model_locked
                && (policy.model != *model
                    || provider.as_ref().is_some_and(|locked| {
                        policy
                            .provider
                            .as_ref()
                            .is_some_and(|current| current != locked)
                    }))
            {
                return Err(ExecutionFault::new(
                    etas_core::AnalysisDiagnosticCode::InvalidArguments,
                    span,
                    "Agent request has incompatible locked model restrictions",
                ));
            }
            policy.model = model.clone();
            policy.model_locked = true;
            if let Some(provider) = provider {
                policy.provider = Some(provider.clone());
            }
        }
        if let Some(maximum) = self.max_output_tokens {
            policy.options.max_output_tokens = Some(
                policy
                    .options
                    .max_output_tokens
                    .map_or(maximum, |current| current.min(maximum)),
            );
        }
        if let Some(maximum) = self.max_context_tokens {
            policy.options.max_context_tokens = Some(
                policy
                    .options
                    .max_context_tokens
                    .map_or(maximum, |current| current.min(maximum)),
            );
        }
        policy.max_tool_rounds = policy.max_tool_rounds.min(self.max_tool_rounds);
        Ok(())
    }
}

impl super::EvalContext<'_> {
    pub(in crate::eval) fn constrain_model_policy(
        &self,
        policy: &mut ModelExecutionPolicy,
        span: etas_core::Span,
    ) -> Result<(), ExecutionFault> {
        for restriction in self.task.model_restrictions() {
            restriction.apply(policy, span)?;
        }
        Ok(())
    }
}
