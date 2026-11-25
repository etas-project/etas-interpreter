use super::{EvalContext, run_state::TaskLocalState};
use crate::{
    api::ModelExecutionPolicy, orchestration::RetryAttemptRecord, value::CheckedExecutionProgram,
};

/// A Host reply belongs to the program and dynamic retry context that issued
/// the request, even while its task is executing a delegated Handler arm.
pub(crate) struct HostCompletionContext {
    program: CheckedExecutionProgram,
    handler_floor: usize,
    retries: Vec<RetryAttemptRecord>,
    model_policy: ModelExecutionPolicy,
    host_context: Option<crate::eval::request_context::AgentHostContext>,
}

impl HostCompletionContext {
    pub(crate) fn capture(ctx: &EvalContext<'_>) -> Self {
        Self {
            program: ctx.checked_execution_program(),
            handler_floor: ctx.handler_floor,
            retries: ctx.task.retry_stack.clone(),
            model_policy: ctx.task.model_policy.clone(),
            host_context: ctx.task.capture_agent_host_context(),
        }
    }

    pub(crate) fn run<T>(
        &self,
        ctx: &mut EvalContext<'_>,
        commit: impl FnOnce(&mut EvalContext<'_>) -> T,
    ) -> T {
        let _request_context = self
            .host_context
            .clone()
            .map(|context| ctx.task.borrow_agent_host_context(context, None));
        let TaskLocalState {
            retry_stack,
            model_policy,
            ..
        } = ctx.task;
        let retries = std::mem::replace(retry_stack, self.retries.clone());
        let policy = std::mem::replace(model_policy, self.model_policy.clone());
        let result = commit(&mut ctx.scoped_context(&self.program, self.handler_floor));
        ctx.task.retry_stack = retries;
        ctx.task.model_policy = policy;
        result
    }
}
