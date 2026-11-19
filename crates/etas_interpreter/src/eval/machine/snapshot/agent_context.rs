use std::{collections::BTreeMap, sync::Arc};

use crate::{
    api::code::{PreparedCodeRevision, PreparedRevisionSet},
    control::{AgentExecutionState, AgentRequestContext},
    orchestration::{AgentExecutionStateSnapshot, AgentRequestContextSnapshot},
    value::CheckedExecutionProgram,
};

pub(crate) fn capture_agent_context(
    request: &AgentRequestContext,
    revisions: &mut BTreeMap<etas_code::RevisionKey, Arc<PreparedCodeRevision>>,
) -> AgentRequestContextSnapshot {
    AgentRequestContextSnapshot {
        program: request.program.checkpoint_identity(revisions),
        method: request.method,
        request: request.request,
        policy: super::capture_model_policy(&request.policy),
    }
}

pub(crate) fn restore_agent_context(
    request: &AgentRequestContextSnapshot,
    source: &CheckedExecutionProgram,
    revisions: Option<&PreparedRevisionSet>,
) -> Result<Box<AgentRequestContext>, String> {
    let program = CheckedExecutionProgram::restore_identity(&request.program, source, revisions)?;
    if !program
        .checked
        .types
        .agent_methods
        .contains_key(&request.method)
    {
        return Err("agent request context has no checked method identity".into());
    }
    Ok(Box::new(AgentRequestContext {
        program,
        method: request.method,
        request: request.request,
        policy: super::restore_model_policy(request.policy.clone()),
    }))
}

impl super::CaptureContext {
    pub(super) fn agent_state(
        &mut self,
        state: &AgentExecutionState,
    ) -> AgentExecutionStateSnapshot {
        AgentExecutionStateSnapshot {
            request: state
                .request
                .as_ref()
                .map(|request| Box::new(capture_agent_context(request, &mut self.revisions))),
            policy: super::capture_model_policy(&state.policy),
        }
    }
}

impl super::RestoreContext {
    pub(super) fn agent_context(
        &self,
        request: &AgentRequestContextSnapshot,
    ) -> Result<Box<AgentRequestContext>, String> {
        restore_agent_context(
            request,
            self.execution_program
                .as_ref()
                .ok_or("agent request restore has no checked program")?,
            self.revisions.as_deref(),
        )
    }

    pub(super) fn agent_state(
        &self,
        state: AgentExecutionStateSnapshot,
    ) -> Result<AgentExecutionState, String> {
        Ok(AgentExecutionState {
            request: state
                .request
                .as_ref()
                .map(|request| self.agent_context(request))
                .transpose()?,
            policy: super::restore_model_policy(state.policy),
        })
    }
}
