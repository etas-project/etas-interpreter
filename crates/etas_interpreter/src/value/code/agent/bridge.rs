use super::*;
use crate::value::{CheckedExecutionProgram, InterpValue};
use etas_types::TypeId;

impl AgentRevisionValue {
    pub(crate) fn action_projections(&self) -> Vec<crate::plan::code::RevisionProjection> {
        use crate::plan::code::RevisionProjection;
        let mut edges = Vec::new();
        if self.frozen_agent().is_none() {
            edges.push(RevisionProjection::Original(
                self.prepared.type_bridge().clone(),
            ));
        }
        if let Some(source) = &self.source_revision {
            edges.push(RevisionProjection::Candidate(source.clone()));
        }
        edges
    }
    pub(crate) fn project_checked(
        mut self,
        source: &CheckedProgram,
        target: &CheckedProgram,
        from: TypeId,
        to: TypeId,
        revision: Option<Arc<crate::plan::code::RevisionTypeBridge>>,
    ) -> Result<Self, String> {
        if !Arc::ptr_eq(source, self.source_program()) || self.source_component()? != from {
            return Err(
                "agent value is not owned by the projected source nominal interface".into(),
            );
        }
        let root = self.prepared.type_bridge().original_program();
        match &revision {
            Some(bridge)
                if Arc::ptr_eq(root, bridge.original_program())
                    && Arc::ptr_eq(target, bridge.candidate_program()) => {}
            None if Arc::ptr_eq(target, root) => {}
            _ => return Err("agent value projection has no common checked source universe".into()),
        }
        self.source_revision = revision;
        if self.source_component()? != to {
            return Err(
                "agent value projection disagrees with the checked target nominal identity".into(),
            );
        }
        Ok(self)
    }

    pub(crate) fn error_to_source(
        &self,
        program: &CheckedExecutionProgram,
        ty: TypeId,
        value: InterpValue,
    ) -> Result<(TypeId, InterpValue), String> {
        if !Arc::ptr_eq(&program.checked, self.destination_program()) {
            return Err("revision error belongs to a different checked execution program".into());
        }
        let bridge = self.prepared.type_bridge();
        let scope = &self.target()?.scope;
        let (original, value) = if self.frozen_agent().is_some() {
            (ty, value)
        } else {
            let original = bridge
                .to_original(&program.checked, scope, ty)
                .map_err(|error| error.to_string())?;
            let value = to_original(bridge, &program.checked, scope, ty, value, Some(program))?;
            (original, value)
        };
        match &self.source_revision {
            Some(source) => {
                let mapped = source
                    .to_candidate(bridge.original_program(), scope, original)
                    .map_err(|error| error.to_string())?;
                let value = to_candidate(
                    source,
                    bridge.original_program(),
                    scope,
                    original,
                    value,
                    Some(program),
                )?;
                Ok((mapped, value))
            }
            None => Ok((original, value)),
        }
    }

    pub(crate) fn argument_to_revision(
        &self,
        program: &CheckedProgram,
        original: TypeId,
        value: InterpValue,
        execution: Option<&CheckedExecutionProgram>,
    ) -> Result<InterpValue, String> {
        if !Arc::ptr_eq(program, self.source_program()) {
            return Err("revision call is not in its checked source program".into());
        }
        let scope = &self.target()?.scope;
        let root = self.prepared.type_bridge().original_program();
        let value = match &self.source_revision {
            Some(source) => {
                let source_ty = source
                    .to_candidate(root, scope, original)
                    .map_err(|error| error.to_string())?;
                to_original(source, program, scope, source_ty, value, execution)?
            }
            None => value,
        };
        if self.frozen_agent().is_some() {
            Ok(value)
        } else {
            to_candidate(
                self.prepared.type_bridge(),
                root,
                scope,
                original,
                value,
                execution,
            )
        }
    }

    pub(crate) fn result_to_source(
        &self,
        program: &CheckedProgram,
        value: InterpValue,
        execution: Option<&CheckedExecutionProgram>,
    ) -> Result<InterpValue, String> {
        if !Arc::ptr_eq(program, self.destination_program()) {
            return Err("revision result belongs to a different checked execution program".into());
        }
        let target = self.target()?;
        let value = if self.frozen_agent().is_some() {
            value
        } else {
            to_original(
                self.prepared.type_bridge(),
                program,
                &target.scope,
                target.candidate.output,
                value,
                execution,
            )?
        };
        match &self.source_revision {
            Some(source) => to_candidate(
                source,
                self.prepared.type_bridge().original_program(),
                &target.scope,
                target.original.output,
                value,
                execution,
            ),
            None => Ok(value),
        }
    }
}

fn to_original(
    bridge: &Arc<crate::plan::code::RevisionTypeBridge>,
    program: &CheckedProgram,
    scope: &etas_code::descriptor::GenericOwner,
    ty: TypeId,
    value: InterpValue,
    execution: Option<&CheckedExecutionProgram>,
) -> Result<InterpValue, String> {
    match execution {
        Some(execution) => bridge.value_to_original_scoped(program, scope, ty, value, execution),
        None => bridge.value_to_original(program, scope, ty, value),
    }
    .map_err(|error| error.to_string())
}

fn to_candidate(
    bridge: &Arc<crate::plan::code::RevisionTypeBridge>,
    program: &CheckedProgram,
    scope: &etas_code::descriptor::GenericOwner,
    ty: TypeId,
    value: InterpValue,
    execution: Option<&CheckedExecutionProgram>,
) -> Result<InterpValue, String> {
    match execution {
        Some(execution) => bridge.value_to_candidate_scoped(program, scope, ty, value, execution),
        None => bridge.value_to_candidate(program, scope, ty, value),
    }
    .map_err(|error| error.to_string())
}
