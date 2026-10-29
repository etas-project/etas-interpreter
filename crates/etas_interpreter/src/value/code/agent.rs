use std::sync::Arc;

use etas_code::CallableId;
use etas_frontend::CheckedProgram;
use etas_hir::HirItemId;

use crate::{api::code::PreparedCodeRevision, plan::code::RevisionCallable};

mod bridge;
mod checkpoint;

/// A native executable reference, never deserialized from descriptive code data.
#[derive(Clone)]
pub struct AgentRevisionValue {
    prepared: Arc<PreparedCodeRevision>,
    source_revision: Option<Arc<crate::plan::code::RevisionTypeBridge>>,
    callable: CallableId,
    frozen_agent: Option<HirItemId>,
}

impl AgentRevisionValue {
    pub(crate) fn new_in_program(
        prepared: Arc<PreparedCodeRevision>,
        source_revision: Option<Arc<PreparedCodeRevision>>,
        program: &CheckedProgram,
        ty: etas_types::TypeId,
    ) -> Result<Self, String> {
        if let Some(source) = &source_revision
            && !Arc::ptr_eq(
                source.type_bridge().original_program(),
                prepared.type_bridge().original_program(),
            )
        {
            return Err(
                "compiled revision has no common checked source universe with its caller".into(),
            );
        }
        let callable = prepared.targets.default.clone();
        let value = Self {
            prepared,
            source_revision: source_revision.map(|source| source.type_bridge().clone()),
            callable,
            frozen_agent: None,
        };
        if !Arc::ptr_eq(program, value.source_program()) || value.source_component()? != ty {
            return Err(
                "compiled agent result disagrees with its checked original nominal identity".into(),
            );
        }
        Ok(value)
    }

    pub(crate) fn frozen_declaration(
        prepared: Arc<PreparedCodeRevision>,
        program: &CheckedProgram,
        agent: HirItemId,
    ) -> Result<Self, String> {
        if !Arc::ptr_eq(program, prepared.program()) {
            return Err(
                "frozen declaration binding belongs to a different candidate program".into(),
            );
        }
        let callable = prepared
            .targets
            .frozen_agents
            .get(&agent)
            .ok_or("frozen declaration has no checked default callable")?
            .clone();
        Ok(Self {
            source_revision: Some(prepared.type_bridge().clone()),
            prepared,
            callable,
            frozen_agent: Some(agent),
        })
    }

    pub(crate) fn frozen_agent(&self) -> Option<HirItemId> {
        self.frozen_agent
    }

    pub(crate) fn frozen_method(
        prepared: Arc<PreparedCodeRevision>,
        program: &CheckedProgram,
        body: HirItemId,
    ) -> Result<Self, String> {
        if !Arc::ptr_eq(program, prepared.program()) {
            return Err("frozen method belongs to a different candidate program".into());
        }
        let (agent, callable) = prepared
            .targets
            .frozen_methods
            .get(&body)
            .ok_or("frozen method has no canonical declaration binding")?
            .clone();
        Ok(Self {
            source_revision: Some(prepared.type_bridge().clone()),
            prepared,
            callable,
            frozen_agent: Some(agent),
        })
    }

    pub(crate) fn destination_program(&self) -> &CheckedProgram {
        if self.frozen_agent.is_some() {
            self.prepared.type_bridge().original_program()
        } else {
            self.prepared.program()
        }
    }

    pub(crate) fn execution_plan(&self) -> Result<&Arc<crate::api::InterpreterPlan>, String> {
        let target = self.target()?;
        let item = if self.frozen_agent.is_some() {
            target.original_item
        } else if self.is_default_entry() {
            self.prepared.entry().item
        } else {
            target.candidate_item
        };
        self.prepared
            .callable_plan(item, self.frozen_agent.is_some())
    }

    pub(crate) fn source_program(&self) -> &CheckedProgram {
        match &self.source_revision {
            Some(source) => source.candidate_program(),
            None => self.prepared.type_bridge().original_program(),
        }
    }

    pub(crate) fn source_component(&self) -> Result<etas_types::TypeId, String> {
        let identity = self
            .callable
            .as_method()
            .map(|method| method.implementation().subject())
            .ok_or("agent callable has no canonical nominal owner")?;
        let original = if self.frozen_agent.is_some() {
            self.prepared
                .source_definitions
                .definitions()
                .catalog()
                .agent_component(identity)
                .ok_or("agent callable has no checked source nominal component")?
        } else {
            self.prepared
                .targets
                .agent_component()
                .ok_or("agent revision has no nominal agent owner")?
        };
        match &self.source_revision {
            Some(source) => source
                .to_candidate(
                    self.prepared.type_bridge().original_program(),
                    &etas_code::descriptor::GenericOwner::Declaration(identity.clone()),
                    original,
                )
                .map_err(|error| error.to_string()),
            None => Ok(original),
        }
    }

    pub(crate) fn source_agent(&self) -> Result<HirItemId, String> {
        let component = self.source_component()?;
        match &self.source_revision {
            Some(source) => source.candidate_agent(component),
            None => self.prepared.type_bridge().original_agent(component),
        }
        .ok_or_else(|| {
            "compiled agent has no checked nominal declaration in the caller program".into()
        })
    }

    pub(crate) fn select_method(
        &self,
        program: &CheckedProgram,
        agent: HirItemId,
        body: HirItemId,
    ) -> Result<Self, String> {
        if !Arc::ptr_eq(program, self.source_program()) || agent != self.source_agent()? {
            return Err(
                "agent method receiver belongs to a different checked program or nominal agent"
                    .into(),
            );
        }
        let mut selected = None;
        let callables = if self.frozen_agent.is_some() {
            &self.prepared.targets.frozen_callables
        } else {
            &self.prepared.targets.callables
        };
        for id in callables.keys() {
            let source_item = match &self.source_revision {
                Some(source) => source
                    .candidate_callable(id)
                    .ok_or("selected method has no checked caller revision identity")?,
                None => self
                    .prepared
                    .type_bridge()
                    .original_callable(id)
                    .ok_or("selected method has no checked source callable identity")?,
            };
            if source_item == body && selected.replace(id).is_some() {
                return Err("selected agent method has ambiguous stable identity".into());
            }
        }
        let Some(callable) = selected else {
            return Err(
                "selected agent method is absent from the frozen callable interface".into(),
            );
        };
        Ok(Self {
            prepared: self.prepared.clone(),
            source_revision: self.source_revision.clone(),
            callable: callable.clone(),
            frozen_agent: self.frozen_agent,
        })
    }

    pub(crate) fn prepared(&self) -> &Arc<PreparedCodeRevision> {
        &self.prepared
    }
    pub(crate) fn is_default_entry(&self) -> bool {
        match self.frozen_agent {
            Some(agent) => self.prepared.targets.frozen_agents.get(&agent) == Some(&self.callable),
            None => self.callable == self.prepared.targets.default,
        }
    }
    pub(crate) fn target(&self) -> Result<&RevisionCallable, String> {
        self.prepared
            .type_bridge()
            .bind_runtime_owner(&self.prepared);
        (if self.frozen_agent.is_some() {
            &self.prepared.targets.frozen_callables
        } else {
            &self.prepared.targets.callables
        })
        .get(&self.callable)
        .ok_or_else(|| "pinned agent callable is absent from its prepared revision".into())
    }

    pub(crate) fn source_signature(&self) -> Result<&etas_types::CallableSignature, String> {
        let item = match &self.source_revision {
            Some(source) => source.candidate_callable(&self.callable),
            None => self
                .prepared
                .type_bridge()
                .original_callable(&self.callable),
        }
        .ok_or("revision callable has no checked source declaration")?;
        match self.source_program().types.item_signatures.get(&item) {
            Some(
                etas_types::ItemSignature::Flow(signature)
                | etas_types::ItemSignature::Tool(signature),
            ) => Ok(signature),
            Some(etas_types::ItemSignature::Agent(signature)) => signature
                .callable()
                .ok_or_else(|| "revision agent has no checked default signature".into()),
            _ => Err("revision callable has no checked source signature".into()),
        }
    }
}

impl std::fmt::Debug for AgentRevisionValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentRevisionValue")
            .field("identity", self.prepared.identity())
            .field("revision", self.prepared.revision())
            .field("callable", &self.callable)
            .field("frozen_agent", &self.frozen_agent)
            .finish()
    }
}
impl PartialEq for AgentRevisionValue {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.prepared, &other.prepared)
            && self.callable == other.callable
            && self.frozen_agent == other.frozen_agent
    }
}
impl Eq for AgentRevisionValue {}
