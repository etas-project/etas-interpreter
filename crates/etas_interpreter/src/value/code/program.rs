use std::sync::Arc;

use etas_frontend::CheckedProgram;

use crate::api::{
    InterpreterPlan,
    code::{PreparedCodeDefinitions, PreparedCodeRevision},
};

/// Immutable code ownership only. Invocation authority and mutable execution
/// state belong to EvalRunState, not to a callable's defining program.
#[derive(Clone, Debug)]
pub(crate) struct CheckedExecutionProgram {
    pub checked: CheckedProgram,
    pub plan: Arc<InterpreterPlan>,
    pub definitions: Option<Arc<PreparedCodeDefinitions>>,
    pub revision: Option<Arc<PreparedCodeRevision>>,
    pub foreign: Option<Arc<ForeignExecutionLink>>,
}

/// A live code link supplied by the caller of an installed library body.
/// It retains code ownership while invocation authority remains in EvalRunState.
#[derive(Clone, Debug)]
pub(crate) struct ForeignExecutionLink {
    pub owner: CheckedExecutionProgram,
    pub projection: crate::plan::code::RevisionProjection,
}

impl ForeignExecutionLink {
    pub(crate) fn method(
        &self,
        library: &CheckedProgram,
        identity: &etas_code::MethodId,
    ) -> Result<crate::control::CallTarget, String> {
        if !Arc::ptr_eq(self.projection.source(), library)
            || !Arc::ptr_eq(self.projection.target(), &self.owner.checked)
        {
            return Err("foreign method link disagrees with its checked program boundary".into());
        }
        let method =
            crate::plan::code::checked_foreign_method(library, &self.owner.checked, identity)?;
        let signature = match self.owner.checked.types.item_signatures.get(&method.item) {
            Some(etas_types::ItemSignature::Flow(signature)) => signature,
            _ => return Err("foreign method has no checked callable signature".into()),
        };
        let contract = self
            .owner
            .checked
            .effects
            .public_contracts
            .iter()
            .find(|contract| contract.item == method.item)
            .ok_or("foreign method has no checked public effect contract")?;
        let parameters = signature
            .generic_params
            .iter()
            .filter(|parameter| parameter.kind == etas_types::CallableGenericParamKind::Effect)
            .map(|parameter| parameter.name.clone())
            .collect::<Vec<_>>();
        let row = etas_effects::facts::checked_type_effect_row(
            &contract.public_row,
            &self.owner.checked.effect_registry,
            &parameters,
        )?;
        let flow = etas_types::FlowType {
            input: signature.params.clone(),
            output: signature.output,
            effects: Some(row),
        };
        let ty = self
            .owner
            .checked
            .type_store
            .iter()
            .find_map(|(ty, candidate)| {
                matches!(candidate, etas_types::Type::Function(candidate) if *candidate == flow)
                    .then_some(ty)
            })
            .ok_or("foreign method has no materialized checked ABI")?;
        let value = self.projection.reversed().value_in_scope(
            &etas_code::descriptor::GenericOwner::Method(identity.clone()),
            ty,
            crate::value::InterpValue::Callable(crate::control::CallTarget::FlowItem(method.item)),
            &self.owner,
        )?;
        let crate::value::InterpValue::Callable(target) = value else {
            return Err("foreign method projection produced a non-callable value".into());
        };
        Ok(target)
    }
}

impl CheckedExecutionProgram {
    pub(crate) fn restore_identity(
        saved: &crate::orchestration::ExecutionProgramSnapshot,
        source: &Self,
        revisions: Option<&crate::api::code::PreparedRevisionSet>,
    ) -> Result<Self, String> {
        use crate::orchestration::{CheckpointProgramReference, ExecutionRootSnapshot as R};
        let owns_program = if let Some(revisions) = revisions {
            // An installed std body and its source caller can both have no
            // dynamic code revision. Resolve their actual checked owner before
            // interpreting the saved root's local HIR identifiers.
            Arc::ptr_eq(&revisions.checked_program(&saved.program)?, &source.checked)
        } else {
            match (&saved.program, &source.revision) {
                (CheckpointProgramReference::Source, None) => true,
                (CheckpointProgramReference::Revision { revision: saved }, Some(current)) => {
                    **saved == crate::orchestration::CheckpointCodeRevision::for_revision(current)
                }
                _ => false,
            }
        };
        if owns_program {
            let root = match &saved.root {
                R::Item(item) => etas_frontend::CheckedCallableRoot::Item(*item),
                R::Lambda(expr) => etas_frontend::CheckedCallableRoot::Lambda(*expr),
                R::StdReference(expr) => etas_frontend::CheckedCallableRoot::StdReference(*expr),
                R::ParameterDefault(reference) => {
                    etas_frontend::CheckedCallableRoot::ParameterDefault(
                        source
                            .plan
                            .arguments
                            .default_unit(reference)
                            .ok_or("checkpoint default root has no matching expression revision")?
                            .parameter,
                    )
                }
            };
            let plan = source
                .plan
                .select_callable(&source.checked, root)
                .map_err(|errors| {
                    format!("checkpoint callable has no rechecked plan: {errors:?}")
                })?;
            return Ok(Self {
                checked: source.checked.clone(),
                plan: Arc::new(plan),
                definitions: source.definitions.clone(),
                revision: source.revision.clone(),
                foreign: source.foreign.clone(),
            });
        }
        revisions
            .ok_or("checkpoint program has no prepared revision set")?
            .execution_snapshot(saved)
    }

    pub(crate) fn checkpoint_identity(
        &self,
        revisions: &mut std::collections::BTreeMap<
            etas_code::RevisionKey,
            Arc<PreparedCodeRevision>,
        >,
    ) -> crate::orchestration::ExecutionProgramSnapshot {
        use crate::orchestration::{
            CheckpointCodeRevision, CheckpointProgramReference, ExecutionProgramSnapshot,
            ExecutionRootSnapshot,
        };
        let program = match &self.revision {
            Some(revision) => {
                revisions.insert(revision.revision().key(), revision.clone());
                CheckpointProgramReference::Revision {
                    revision: Box::new(CheckpointCodeRevision::for_revision(revision)),
                }
            }
            None => CheckpointProgramReference::Source,
        };
        let root = match self.plan.root {
            etas_frontend::CheckedCallableRoot::Item(item) => ExecutionRootSnapshot::Item(item),
            etas_frontend::CheckedCallableRoot::Lambda(expr) => ExecutionRootSnapshot::Lambda(expr),
            etas_frontend::CheckedCallableRoot::StdReference(expr) => {
                ExecutionRootSnapshot::StdReference(expr)
            }
            etas_frontend::CheckedCallableRoot::ParameterDefault(parameter) => {
                ExecutionRootSnapshot::ParameterDefault(
                    self.plan
                        .arguments
                        .source_default_reference(parameter)
                        .expect("prepared default root retains its canonical expression reference")
                        .clone(),
                )
            }
        };
        ExecutionProgramSnapshot { program, root }
    }
}
