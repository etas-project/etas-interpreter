use super::*;
use crate::control::{AgentExecutionState, AgentRequestContext};

impl EvalContext<'_> {
    pub(in crate::eval) fn agent_context_matches(
        &self,
        ty: etas_types::TypeId,
        method: SymbolId,
    ) -> bool {
        match self.checked.type_store.get(ty) {
            Some(Type::AgentContext(etas_types::AgentContextRef::Source(source))) => {
                *source == method
            }
            Some(Type::AgentContext(etas_types::AgentContextRef::Portable(identity))) => {
                let Some(fact) = self.checked.types.agent_methods.get(&method) else {
                    return false;
                };
                self.code_definitions
                    .as_ref()
                    .filter(|prepared| prepared.is_for(self.checked))
                    .is_some_and(|prepared| {
                        prepared.definitions().catalog().native_callables().any(
                            |(callable, item)| {
                                item == fact.body && callable.as_method() == Some(identity)
                            },
                        )
                    })
            }
            _ => false,
        }
    }

    pub(in crate::eval) fn new_agent_context(&mut self, method: SymbolId) -> AgentRequestContext {
        AgentRequestContext {
            program: self.checked_execution_program(),
            method,
            request: self.next_host_request_id(),
            policy: self.task.model_policy.clone(),
        }
    }

    pub(in crate::eval) fn enter_agent_context(
        &mut self,
        context: Box<AgentRequestContext>,
    ) -> AgentExecutionState {
        let policy = std::mem::replace(&mut self.task.model_policy, context.policy.clone());
        let request = self.task.agent_context.replace(context);
        AgentExecutionState { request, policy }
    }

    pub(in crate::eval) fn agent_execution_state(&self) -> AgentExecutionState {
        AgentExecutionState {
            request: self.task.agent_context.clone(),
            policy: self.task.model_policy.clone(),
        }
    }

    pub(in crate::eval) fn restore_agent_context(&mut self, previous: AgentExecutionState) {
        self.task.agent_context = previous.request;
        self.task.model_policy = previous.policy;
    }

    pub(in crate::eval) fn agent_scope_continuation(
        &mut self,
        state: AgentExecutionState,
        inner: Continuation,
    ) -> Continuation {
        let previous = self.agent_execution_state();
        self.restore_agent_context(state);
        compose_continuation(inner, Continuation::AgentExecutionState(Box::new(previous)))
    }
}
