use super::*;
use crate::orchestration::{CheckpointAgentReference, CheckpointCodeRevision};

impl AgentRevisionValue {
    pub(crate) fn checkpoint_reference(&self) -> Result<CheckpointAgentReference, String> {
        self.target()?;
        Ok(CheckpointAgentReference {
            revision: CheckpointCodeRevision::for_revision(&self.prepared),
            callable: self.callable.clone(),
            frozen_agent: self.frozen_agent,
        })
    }

    pub(crate) fn restore_checkpoint_reference(
        reference: &CheckpointAgentReference,
        prepared: Arc<PreparedAgentRevision>,
        source_revision: Option<Arc<crate::plan::code::RevisionTypeBridge>>,
        program: &CheckedProgram,
    ) -> Result<Self, String> {
        let value = Self {
            prepared,
            source_revision,
            callable: reference.callable.clone(),
            frozen_agent: reference.frozen_agent,
        };
        value.target()?;
        if let Some(agent) = value.frozen_agent {
            let owner = value
                .prepared
                .targets
                .frozen_agents
                .get(&agent)
                .and_then(CallableId::as_method)
                .map(|method| method.implementation().subject());
            if owner
                != value
                    .callable
                    .as_method()
                    .map(|method| method.implementation().subject())
            {
                return Err(
                    "checkpoint frozen method differs from its declared agent owner".into(),
                );
            }
        } else if value
            .callable
            .as_method()
            .map(|method| method.implementation().subject())
            != Some(value.prepared.identity())
        {
            return Err("checkpoint agent method differs from its compiled nominal owner".into());
        }
        if !Arc::ptr_eq(program, value.source_program()) {
            return Err("checkpoint agent reference belongs to a different caller program".into());
        }
        value.source_agent()?;
        // Resolve the prepared entry as well as the stable callable identity.
        // Neither a serialized method name nor an item ID is executable proof.
        value.execution_plan()?;
        Ok(value)
    }
}
