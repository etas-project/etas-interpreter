use std::num::{NonZeroU32, NonZeroU64};

use etas_hir::HirItemId;
use etas_host::{
    AuthorityContext, Budget, ExecutionBudget, ModelName, ModelOptions, ModelProviderCapabilities,
    ModelProviderId, ModelToolChoice, ToolSchema, TraceContext, TraceId,
};
use etas_utils::ProfileHandle;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EntryPoint {
    pub item: HirItemId,
}

#[derive(Clone, Debug, Default)]
pub struct PlanOptions;

/// Default number of heap-backed interpreter call frames.
pub const DEFAULT_MAX_CALL_DEPTH: u32 = 4096;

/// Highest explicitly configured heap-backed interpreter call depth.
pub const MAX_CONFIGURABLE_CALL_DEPTH: u32 = 65_536;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExecutionLimits {
    /// Maximum nested source calls before execution returns an Etas diagnostic.
    pub max_call_depth: NonZeroU32,
    pub max_steps: Option<NonZeroU64>,
}

impl ExecutionLimits {
    pub fn new(max_call_depth: NonZeroU32, max_steps: Option<NonZeroU64>) -> Result<Self, String> {
        let limits = Self {
            max_call_depth,
            max_steps,
        };
        limits.validate()?;
        Ok(limits)
    }

    pub fn validate(self) -> Result<(), String> {
        if self.max_call_depth.get() > MAX_CONFIGURABLE_CALL_DEPTH {
            return Err(format!(
                "maximum call depth {} exceeds the configurable hard cap {MAX_CONFIGURABLE_CALL_DEPTH}",
                self.max_call_depth
            ));
        }
        Ok(())
    }

    pub(crate) fn stricter(self, other: Self) -> Self {
        let max_steps = match (self.max_steps, other.max_steps) {
            (Some(left), Some(right)) => Some(left.min(right)),
            (Some(limit), None) | (None, Some(limit)) => Some(limit),
            (None, None) => None,
        };
        Self {
            max_call_depth: self.max_call_depth.min(other.max_call_depth),
            max_steps,
        }
    }
}

impl Default for ExecutionLimits {
    fn default() -> Self {
        Self {
            max_call_depth: NonZeroU32::new(DEFAULT_MAX_CALL_DEPTH)
                .expect("default call-depth limit is non-zero"),
            max_steps: None,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct RunOptions {
    pub plan: PlanOptions,
    pub execution_limits: ExecutionLimits,
    pub host_context: HostExecutionContext,
    pub model_policy: ModelExecutionPolicy,
    pub current_session: Option<String>,
    pub profile: ProfileHandle,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HostExecutionContext {
    pub authority: AuthorityContext,
    pub trace: TraceContext,
    pub budget: ExecutionBudget,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ModelExecutionPolicy {
    pub provider: Option<ModelProviderId>,
    pub provider_capabilities: Option<ModelProviderCapabilities>,
    pub model: ModelName,
    pub model_locked: bool,
    pub tools: Vec<ToolSchema>,
    pub tool_choice: ModelToolChoice,
    pub policy_ref: Option<etas_host::HostValue>,
    pub options: ModelOptions,
    pub budget: Option<Budget>,
    pub response_decode: ModelResponseDecodePolicy,
    pub max_tool_rounds: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelResponseDecodePolicy {
    String,
    ModelResponse,
}

impl ModelExecutionPolicy {
    pub fn phase1_default() -> Self {
        Self {
            provider: None,
            provider_capabilities: None,
            model: ModelName("phase1-default".to_owned()),
            model_locked: false,
            tools: Vec::new(),
            tool_choice: ModelToolChoice::Auto,
            policy_ref: None,
            options: ModelOptions::default(),
            budget: None,
            response_decode: ModelResponseDecodePolicy::String,
            max_tool_rounds: 8,
        }
    }
}

impl Default for ModelExecutionPolicy {
    fn default() -> Self {
        Self::phase1_default()
    }
}

impl Default for HostExecutionContext {
    fn default() -> Self {
        Self {
            authority: AuthorityContext::deny_all(),
            trace: TraceContext::root(TraceId(0)),
            budget: ExecutionBudget::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn execution_limits_default_to_safe_call_depth() {
        let limits = ExecutionLimits::default();
        assert_eq!(limits.max_call_depth.get(), 4096);
        assert_eq!(limits.max_steps, None);
    }

    #[test]
    fn execution_limits_reject_call_depth_above_hard_cap() {
        let error = ExecutionLimits::new(
            NonZeroU32::new(MAX_CONFIGURABLE_CALL_DEPTH + 1).expect("non-zero"),
            None,
        )
        .expect_err("call depth above the hard cap must be rejected");
        assert!(error.contains("hard cap 65536"), "{error}");
    }
}
