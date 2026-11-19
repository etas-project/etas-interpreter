use std::sync::Arc;

use etas_frontend::{CheckedProgram, CheckedProject};
use etas_types::TypeId;

use super::{ExistentialHeader, ExistentialValue};
use crate::{
    control::GenericBindings, plan::code::RevisionProjection, value::CheckedExecutionProgram,
};

/// The original packing proof stays in its checked program. Payloads and the
/// presented interface are relocated through the same checked program edges.
#[derive(Clone, Debug)]
pub(crate) struct ExistentialOrigin {
    pub(crate) program: CheckedExecutionProgram,
    pub(crate) header: ExistentialHeader,
    pub(crate) projections: Vec<RevisionProjection>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ExistentialOriginSnapshot {
    program: crate::orchestration::ExecutionProgramSnapshot,
    header: ExistentialHeader,
    projections: Vec<crate::orchestration::CheckedProjectionSnapshot>,
}

impl ExistentialOrigin {
    pub(super) fn validate(
        &self,
        presented: &ExistentialHeader,
        checked: &CheckedProject,
    ) -> Result<GenericBindings, String> {
        let bindings = self.header.validate(&self.program.checked)?;
        if presented.pack != self.header.pack
            || presented.type_bindings != self.header.type_bindings
            || presented.type_parameter_bindings != self.header.type_parameter_bindings
            || presented.effect_row_bindings != self.header.effect_row_bindings
        {
            return Err("scoped existential changes its original packing evidence".into());
        }
        let mut program = &self.program.checked;
        let mut object_type = self.header.object_type;
        let mut concrete_type = self.header.concrete_type;
        for edge in &self.projections {
            if !Arc::ptr_eq(program, edge.source()) {
                return Err("existential projection has a different checked source program".into());
            }
            object_type = edge.runtime_type(object_type)?;
            concrete_type = edge.runtime_type(concrete_type)?;
            program = edge.target();
        }
        if !Arc::ptr_eq(program, checked)
            || object_type != presented.object_type
            || concrete_type != presented.concrete_type
        {
            return Err(
                "existential projection differs from its checked receiving interface".into(),
            );
        }
        Ok(bindings)
    }
}

impl ExistentialValue {
    pub(crate) fn method_owner(
        &self,
        checked: &CheckedProgram,
        identity: &etas_code::MethodId,
    ) -> Result<(CheckedExecutionProgram, Vec<RevisionProjection>), String> {
        let (owner, _, _) = checked
            .foreign_method(identity)
            .ok_or("existential method has no checked foreign implementation owner")?;
        let mut object = self;
        loop {
            if let Some(origin) = &object.origin
                && Arc::ptr_eq(
                    origin.program.checked.hir.shared_program(),
                    owner.hir.shared_program(),
                )
            {
                origin.validate(&ExistentialHeader::capture(object), checked)?;
                return Ok((origin.program.clone(), origin.projections.clone()));
            }
            match object.value.as_ref() {
                super::super::InterpValue::Existential(inner) => object = inner,
                _ => return Err("existential method has no retained checked code owner".into()),
            }
        }
    }

    pub(crate) fn project(
        &mut self,
        execution: &CheckedExecutionProgram,
        edge: &RevisionProjection,
        target: TypeId,
    ) -> Result<(), String> {
        self.validate(edge.source())?;
        if edge.runtime_type(self.object_type)? != target {
            return Err("existential projection changes its checked interface".into());
        }
        let mut origin = match &self.origin {
            Some(origin) => origin.as_ref().clone(),
            None => {
                if !Arc::ptr_eq(&execution.checked, edge.source()) {
                    return Err("existential packing has no owning execution program".into());
                }
                ExistentialOrigin {
                    program: execution.clone(),
                    header: ExistentialHeader::capture(self),
                    projections: Vec::new(),
                }
            }
        };
        self.object_type = target;
        self.concrete_type = edge.runtime_type(self.concrete_type)?;
        origin.projections.push(edge.clone());
        self.origin = Some(Arc::new(origin));
        Ok(())
    }
}

impl ExistentialHeader {
    pub(crate) fn checkpoint(
        value: &ExistentialValue,
        context: &mut crate::eval::machine::snapshot::CaptureContext,
    ) -> Result<Self, String> {
        let mut header = Self::capture(value);
        if let Some(origin) = &value.origin {
            for edge in &origin.projections {
                if let Some(revision) = edge.runtime_owner() {
                    context.bind_revision(&revision);
                }
            }
            let program = context.execution_identity(&origin.program)?;
            let projections = origin
                .projections
                .iter()
                .map(|edge| edge.checkpoint(context))
                .collect::<Result<_, _>>()?;
            header.origin = Some(Box::new(ExistentialOriginSnapshot {
                program,
                header: origin.header.clone(),
                projections,
            }));
        }
        Ok(header)
    }

    pub(crate) fn validate_snapshot(
        &self,
        checked: &CheckedProgram,
        revisions: Option<&crate::api::code::PreparedRevisionSet>,
    ) -> Result<GenericBindings, String> {
        match &self.origin {
            Some(origin) => origin.restore(revisions)?.validate(self, checked),
            None => self.validate(checked),
        }
    }
}

impl ExistentialOriginSnapshot {
    pub(super) fn restore(
        &self,
        revisions: Option<&crate::api::code::PreparedRevisionSet>,
    ) -> Result<ExistentialOrigin, String> {
        let revisions =
            revisions.ok_or("scoped existential requires freshly checked program origins")?;
        Ok(ExistentialOrigin {
            program: revisions.execution_snapshot(&self.program)?,
            header: self.header.clone(),
            projections: self
                .projections
                .iter()
                .map(|edge| revisions.checked_projection(edge))
                .collect::<Result<_, _>>()?,
        })
    }
}
