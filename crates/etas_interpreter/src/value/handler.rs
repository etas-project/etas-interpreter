use std::sync::Arc;

use etas_frontend::CheckedProgram;
use etas_types::TypeId;

use super::{CheckedExecutionProgram, InterpValue, SharedValue};
use crate::plan::code::RevisionProjection;

/// Executable Handler code and captures remain in their checked native program.
/// A projection describes the interface presented to another program.
#[derive(Clone, Debug)]
pub struct ScopedHandlerValue {
    pub(crate) program: CheckedExecutionProgram,
    pub(crate) native_type: TypeId,
    pub(crate) projections: Vec<RevisionProjection>,
    pub(crate) value: SharedValue,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ScopedHandlerMetadata {
    pub program: crate::orchestration::ExecutionProgramSnapshot,
    pub native_type: TypeId,
    pub projections: Vec<crate::orchestration::CheckedProjectionSnapshot>,
}

impl ScopedHandlerValue {
    pub(crate) fn project(
        execution: &CheckedExecutionProgram,
        source: &CheckedProgram,
        ty: TypeId,
        value: InterpValue,
        edge: RevisionProjection,
        target: TypeId,
    ) -> Result<Self, String> {
        let mut handler = match value {
            InterpValue::ScopedHandler(handler) => *handler,
            value @ InterpValue::Handler { .. } => {
                if !Arc::ptr_eq(&execution.checked, source) {
                    return Err("Handler has no checked native execution owner".into());
                }
                Self {
                    program: execution.clone(),
                    native_type: ty,
                    projections: Vec::new(),
                    value: value.into(),
                }
            }
            _ => return Err("checked Handler interface contains a different runtime value".into()),
        };
        if handler.presented(source)? != ty
            || !Arc::ptr_eq(edge.source(), source)
            || edge.runtime_type(ty)? != target
        {
            return Err("Handler projection differs from its checked interface".into());
        }
        handler.projections.push(edge);
        Ok(handler)
    }

    pub(crate) fn presented(&self, checked: &CheckedProgram) -> Result<TypeId, String> {
        let mut program = &self.program.checked;
        let mut ty = self.native_type;
        for edge in &self.projections {
            if !Arc::ptr_eq(program, edge.source()) {
                return Err("Handler projection has a different checked source".into());
            }
            ty = edge.runtime_type(ty)?;
            program = edge.target();
        }
        if !Arc::ptr_eq(program, checked) {
            return Err("Handler belongs to a different checked receiving program".into());
        }
        Ok(ty)
    }

    pub(crate) fn parts(
        &self,
    ) -> Result<
        (
            etas_hir::HirExprId,
            &[crate::orchestration::ActiveHandlerArmRecord],
            Option<&crate::control::Frame>,
        ),
        String,
    > {
        match self.value.as_ref() {
            InterpValue::Handler {
                fact_expr,
                handlers,
                captured,
            } => Ok((*fact_expr, handlers, captured.as_ref())),
            _ => Err("scoped Handler has no native Handler literal".into()),
        }
    }

    pub(crate) fn validate(
        &self,
        checked: &CheckedProgram,
        expected: TypeId,
        limits: &etas_host::StorageLimits,
    ) -> Result<(), String> {
        if self.presented(checked)? != expected {
            return Err("Handler has a different checked slot type".into());
        }
        self.parts()?;
        super::contract::runtime::validate_value(
            &self.program.checked,
            &self.program.plan,
            &self.value,
            self.native_type,
            limits,
        )
    }

    pub(crate) fn checkpoint(
        &self,
        context: &mut crate::eval::machine::snapshot::CaptureContext,
    ) -> Result<ScopedHandlerMetadata, String> {
        for edge in &self.projections {
            if let Some(revision) = edge.runtime_owner() {
                context.bind_revision(&revision);
            }
        }
        Ok(ScopedHandlerMetadata {
            program: context.execution_identity(&self.program)?,
            native_type: self.native_type,
            projections: self
                .projections
                .iter()
                .map(|edge| edge.checkpoint(context))
                .collect::<Result<_, _>>()?,
        })
    }
}

impl ScopedHandlerMetadata {
    pub(crate) fn resolve(
        &self,
        revisions: &crate::api::code::PreparedRevisionSet,
    ) -> Result<(CheckedExecutionProgram, Vec<RevisionProjection>), String> {
        Ok((
            revisions.execution_snapshot(&self.program)?,
            self.projections
                .iter()
                .map(|edge| revisions.checked_projection(edge))
                .collect::<Result<_, _>>()?,
        ))
    }

    pub(crate) fn restore(
        &self,
        value: InterpValue,
        checked: &CheckedProgram,
        revisions: &crate::api::code::PreparedRevisionSet,
    ) -> Result<ScopedHandlerValue, String> {
        let (program, projections) = self.resolve(revisions)?;
        let handler = ScopedHandlerValue {
            program,
            native_type: self.native_type,
            projections,
            value: value.into(),
        };
        handler.parts()?;
        handler.presented(checked)?;
        Ok(handler)
    }
}
