use std::sync::Arc;

use super::RevisionTypeBridge;
use crate::value::AgentRevisionValue;
use etas_frontend::CheckedProgram;
use etas_types::TypeId;

#[derive(Clone, Debug)]
pub(crate) enum RevisionProjection {
    Candidate(Arc<RevisionTypeBridge>),
    Original(Arc<RevisionTypeBridge>),
}

impl RevisionProjection {
    pub(crate) fn runtime_owner(&self) -> Option<Arc<crate::api::code::PreparedCodeRevision>> {
        match self {
            Self::Candidate(bridge) | Self::Original(bridge) => bridge.runtime_owner(),
        }
    }
    pub(crate) fn source(&self) -> &CheckedProgram {
        match self {
            Self::Candidate(bridge) => bridge.original_program(),
            Self::Original(bridge) => bridge.candidate_program(),
        }
    }
    pub(crate) fn target(&self) -> &CheckedProgram {
        match self {
            Self::Candidate(bridge) => bridge.candidate_program(),
            Self::Original(bridge) => bridge.original_program(),
        }
    }
    pub(crate) fn runtime_type(&self, ty: TypeId) -> Result<TypeId, String> {
        match self {
            Self::Candidate(bridge) => bridge.runtime_to_candidate(self.source(), ty),
            Self::Original(bridge) => bridge.runtime_to_original(self.source(), ty),
        }
        .map_err(|error| error.to_string())
    }
    pub(crate) fn action(
        &self,
        action: &etas_effects::ActionRef,
    ) -> Result<etas_effects::ActionRef, String> {
        match self {
            Self::Candidate(bridge) => bridge
                .action_to_candidate(self.source(), action)
                .map(|pair| pair.candidate.clone()),
            Self::Original(bridge) => bridge
                .action_to_original(self.source(), action)
                .map(|pair| pair.original.clone()),
        }
        .map_err(|error| error.to_string())
    }
    pub(crate) fn reversed(&self) -> Self {
        match self {
            Self::Candidate(bridge) => Self::Original(bridge.clone()),
            Self::Original(bridge) => Self::Candidate(bridge.clone()),
        }
    }

    pub(crate) fn effect_row(
        &self,
        row: etas_types::EffectRowRef,
    ) -> Result<etas_types::EffectRowRef, String> {
        match self {
            Self::Candidate(bridge) => bridge.runtime_row_to_candidate(self.source(), row),
            Self::Original(bridge) => bridge.runtime_row_to_original(self.source(), row),
        }
        .map_err(|error| error.to_string())
    }

    pub(crate) fn value(
        &self,
        ty: TypeId,
        value: crate::value::InterpValue,
        execution: &crate::value::CheckedExecutionProgram,
    ) -> Result<(TypeId, crate::value::InterpValue), String> {
        let target = self.runtime_type(ty)?;
        let value = super::convert_scoped(
            self.source(),
            self.target(),
            ty,
            target,
            value,
            Some(execution),
            Some(self.clone()),
        )
        .map_err(|error| error.to_string())?;
        Ok((target, value))
    }

    pub(super) fn agent(
        &self,
        source: &CheckedProgram,
        target: &CheckedProgram,
        from: TypeId,
        to: TypeId,
        value: AgentRevisionValue,
    ) -> Result<AgentRevisionValue, String> {
        let (bridge, candidate) = match self {
            Self::Candidate(bridge) => (bridge, true),
            Self::Original(bridge) => (bridge, false),
        };
        let (expected_source, expected_target) = if candidate {
            (bridge.original_program(), bridge.candidate_program())
        } else {
            (bridge.candidate_program(), bridge.original_program())
        };
        if !Arc::ptr_eq(source, expected_source) || !Arc::ptr_eq(target, expected_target) {
            return Err("agent projection disagrees with its checked revision edge".into());
        }
        value.project_checked(source, target, from, to, candidate.then(|| bridge.clone()))
    }
}
