use std::sync::Arc;

use crate::{
    control::CallTarget,
    value::{AgentRevisionValue, CheckedExecutionProgram, InterpValue, ScopedCallableValue},
};

mod specialization;
use specialization::ProgramSpecialization;

#[derive(Clone, Debug)]
pub(crate) struct ProgramCall {
    target: ProgramCallTarget,
    specialization: Option<Arc<ProgramSpecialization>>,
}

#[derive(Clone, Debug)]
pub(crate) enum ProgramCallTarget {
    Agent(AgentRevisionValue),
    Callable(ScopedCallableValue),
}

impl ProgramCall {
    pub(super) fn select(
        target: CallTarget,
        program: &etas_frontend::CheckedProgram,
    ) -> Result<Result<Self, CallTarget>, String> {
        let mut root = &target;
        let mut types = Vec::new();
        let mut rows = Vec::new();
        while let CallTarget::Specialized {
            target,
            type_bindings,
            effect_row_bindings,
        } = root
        {
            types.extend(type_bindings.iter().cloned());
            rows.extend(effect_row_bindings.iter().cloned());
            root = target;
        }
        let target = match root {
            CallTarget::AgentRevision(call) => ProgramCallTarget::Agent(call.clone()),
            CallTarget::Scoped(call) => ProgramCallTarget::Callable(call.clone()),
            _ => return Ok(Err(target)),
        };
        let mut call = Self {
            target,
            specialization: None,
        };
        if !types.is_empty() || !rows.is_empty() {
            call.specialization = Some(Arc::new(ProgramSpecialization::new(
                &call, program, types, rows,
            )?));
        }
        Ok(Ok(call))
    }

    pub(super) fn restore_target(
        target: CallTarget,
        program: &etas_frontend::CheckedProgram,
    ) -> Result<Self, String> {
        Self::select(target, program)?
            .map_err(|_| "checkpoint program boundary restored an unscoped callable".into())
    }

    pub(super) fn target(&self) -> &ProgramCallTarget {
        &self.target
    }

    pub(super) fn call_target(&self) -> CallTarget {
        let target = match &self.target {
            ProgramCallTarget::Agent(call) => CallTarget::AgentRevision(call.clone()),
            ProgramCallTarget::Callable(call) => CallTarget::Scoped(call.clone()),
        };
        match &self.specialization {
            Some(specialization) => CallTarget::Specialized {
                target: target.into(),
                type_bindings: specialization.presented_types.clone(),
                effect_row_bindings: specialization.presented_rows.clone(),
            },
            None => target,
        }
    }

    pub(super) fn action_projections(
        &self,
    ) -> Result<Vec<crate::plan::code::RevisionProjection>, String> {
        match &self.target {
            ProgramCallTarget::Agent(call) => Ok(call.action_projections()),
            ProgramCallTarget::Callable(call) => call.action_projections(),
        }
    }
    pub(super) fn destination(&self) -> Result<CheckedExecutionProgram, String> {
        match &self.target {
            ProgramCallTarget::Callable(call) => Ok(call.program().clone()),
            ProgramCallTarget::Agent(call) => Ok(CheckedExecutionProgram {
                checked: call.destination_program().clone(),
                plan: call.execution_plan()?.clone(),
                definitions: Some(if call.frozen_agent().is_some() {
                    call.prepared().source_definitions.clone()
                } else {
                    call.prepared().definitions.clone()
                }),
                revision: call
                    .frozen_agent()
                    .is_none()
                    .then(|| call.prepared().clone()),
            }),
        }
    }
    pub(super) fn arguments(
        &self,
        program: &CheckedExecutionProgram,
        args: Vec<InterpValue>,
    ) -> Result<Vec<InterpValue>, String> {
        if let Some(specialization) = &self.specialization {
            return specialization.arguments(self, program, args);
        }
        match &self.target {
            ProgramCallTarget::Callable(call) => call.arguments_to_native(program, args),
            ProgramCallTarget::Agent(call) => {
                if !Arc::ptr_eq(&program.checked, call.source_program()) {
                    return Err("revision call is not in its checked source program".into());
                }
                let signature = &call.target()?.original;
                if args.len() != signature.params.len() {
                    return Err(
                        "revision call argument count disagrees with checked signature".into(),
                    );
                }
                args.into_iter()
                    .zip(&signature.params)
                    .map(|(value, ty)| {
                        call.argument_to_revision(&program.checked, *ty, value, Some(program))
                    })
                    .collect()
            }
        }
    }
    pub(super) fn body(&self) -> Result<CallTarget, String> {
        let target = self.native_body()?;
        match &self.specialization {
            Some(specialization) => specialization.body(target),
            None => Ok(target),
        }
    }

    fn native_body(&self) -> Result<CallTarget, String> {
        let call = match &self.target {
            ProgramCallTarget::Callable(call) => return Ok(call.target()),
            ProgramCallTarget::Agent(call) => call,
        };
        let target = call.target()?;
        let item = if call.frozen_agent().is_some() {
            target.original_item
        } else if call.is_default_entry() {
            call.prepared()
                .targets
                .agent_item()
                .ok_or("agent revision has no checked agent owner")?
        } else {
            target.candidate_item
        };
        match call.destination_program().hir.items.get(item) {
            Some(etas_hir::HirItem::Agent(_)) => Ok(CallTarget::AgentItem(item)),
            Some(etas_hir::HirItem::Flow(_)) => Ok(CallTarget::FlowItem(item)),
            _ => Err("revision callable has no executable checked body".into()),
        }
    }
    pub(super) fn result(
        &self,
        program: &CheckedExecutionProgram,
        value: InterpValue,
    ) -> Result<InterpValue, String> {
        if let Some(specialization) = &self.specialization {
            return specialization.result(self, program, value);
        }
        match &self.target {
            ProgramCallTarget::Agent(call) => {
                call.result_to_source(&program.checked, value, Some(program))
            }
            ProgramCallTarget::Callable(call) => call.result_to_presented(program, value),
        }
    }

    pub(super) fn error(
        &self,
        program: &CheckedExecutionProgram,
        ty: etas_types::TypeId,
        value: InterpValue,
    ) -> Result<(etas_types::TypeId, InterpValue), String> {
        if let Some(specialization) = &self.specialization {
            return specialization.project_outgoing(self, program, ty, value);
        }
        match &self.target {
            ProgramCallTarget::Agent(call) => call.error_to_source(program, ty, value),
            ProgramCallTarget::Callable(call) => call.error_to_presented(program, ty, value),
        }
    }
}
