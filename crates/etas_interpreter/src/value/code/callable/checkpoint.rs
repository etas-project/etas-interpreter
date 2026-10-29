use super::*;
use crate::orchestration::{
    CallableProjectionSnapshot, CheckpointCodeRevision, CheckpointProgramReference,
    ScopedCallableMetadata,
};

impl ScopedCallableValue {
    /// Selects an identity from checked declarations before preparing its exact
    /// executable plan. This lookup alone never authorizes a checkpoint call.
    pub(crate) fn snapshot_root(
        program: &CheckedProgram,
        dispatch: &crate::plan::IntrinsicDispatchTable,
        mut target: &crate::orchestration::CallTargetSnapshot,
    ) -> Result<CheckedCallableRoot, String> {
        use crate::orchestration::CallTargetSnapshot as T;
        loop {
            match target {
                T::Specialized { target: inner, .. } | T::Limited { target: inner, .. } => {
                    target = inner
                }
                T::Lambda { expr, .. } => return Ok(CheckedCallableRoot::Lambda(*expr)),
                T::FlowItem(item) | T::ToolItem(item) => {
                    return Ok(CheckedCallableRoot::Item(*item));
                }
                T::SpecImplMethod(symbol) => {
                    return match program.symbols.get(*symbol).map(|symbol| &symbol.def) {
                        Some(etas_hir::SymbolDef::Item { item }) => {
                            Ok(CheckedCallableRoot::Item(*item))
                        }
                        _ => Err("checkpoint spec callable has no checked item".into()),
                    };
                }
                T::PureIntrinsic {
                    intrinsic,
                    parameter_types,
                    result_type,
                } => {
                    return dispatch.intrinsic_reference_root(
                        *intrinsic,
                        parameter_types,
                        *result_type,
                    );
                }
                T::StdIntrinsic { origin, .. } => {
                    return Ok(CheckedCallableRoot::StdReference(*origin));
                }
                _ => return Err("checkpoint callable has no checked native root".into()),
            }
        }
    }

    pub(crate) fn checked_snapshot_root(
        native: &CheckedExecutionProgram,
        ty: TypeId,
        mut target: &crate::orchestration::CallTargetSnapshot,
    ) -> Result<CheckedCallableRoot, String> {
        use crate::orchestration::CallTargetSnapshot as T;
        let mut bindings = crate::control::GenericBindings::new();
        let root = Self::snapshot_root(&native.checked, &native.plan.dispatch, target)?;
        loop {
            match target {
                T::Specialized {
                    target: inner,
                    type_bindings,
                    effect_row_bindings,
                } => {
                    merge_checked_bindings(&mut bindings, type_bindings, effect_row_bindings)?;
                    target = inner;
                }
                T::Limited { target: inner, .. } => target = inner,
                T::Lambda { captured, .. } => {
                    merge_checked_bindings(
                        &mut bindings,
                        &captured.type_bindings,
                        &captured.effect_row_bindings,
                    )?;
                    break;
                }
                T::PureIntrinsic {
                    intrinsic,
                    parameter_types,
                    result_type,
                } => {
                    native.plan.dispatch.intrinsic_callable_root(
                        crate::intrinsic::dispatch::StdIntrinsicIdentity {
                            intrinsic: *intrinsic,
                            dispatch: etas_std::IntrinsicDispatch::PureKernel,
                        },
                        parameter_types,
                        *result_type,
                    )?;
                    break;
                }
                T::StdIntrinsic {
                    origin,
                    substitutions,
                    intrinsic,
                    dispatch,
                    parameter_types,
                    result_type,
                } => {
                    native.plan.dispatch.std_callable_root(
                        &crate::intrinsic::dispatch::CheckedStdIntrinsicCall {
                            origin: *origin,
                            substitutions: substitutions.clone(),
                            identity: crate::intrinsic::dispatch::StdIntrinsicIdentity {
                                intrinsic: *intrinsic,
                                dispatch: *dispatch,
                            },
                            parameter_types: parameter_types.clone(),
                            result_type: *result_type,
                        },
                    )?;
                    break;
                }
                T::FlowItem(_) | T::ToolItem(_) | T::SpecImplMethod(_) => break,
                _ => return Err("checkpoint callable has no checked native root".into()),
            }
        }
        let Some(Type::Function(signature)) = native.checked.type_store.get(ty) else {
            return Err("checkpoint native callable type is not a function".into());
        };
        validate_snapshot_signature(
            &native.checked,
            root,
            signature,
            &bindings,
            native.plan.checked_spec_facts(),
        )?;
        Ok(root)
    }
    pub(crate) fn checkpoint_metadata(
        &self,
        revisions: &mut std::collections::BTreeMap<
            etas_code::RevisionKey,
            Arc<crate::api::code::PreparedCodeRevision>,
        >,
    ) -> Result<ScopedCallableMetadata, String> {
        let native_program = match &self.native.program.revision {
            Some(revision) => {
                revisions.insert(revision.revision().key(), revision.clone());
                CheckpointProgramReference::Revision {
                    revision: Box::new(CheckpointCodeRevision::for_revision(revision)),
                }
            }
            None => CheckpointProgramReference::Source,
        };
        let mut projections = Vec::with_capacity(self.projections.len());
        for projection in &self.projections {
            let owner = projection
                .revision_owner()
                .ok_or("checkpoint callable projection has no pinned native revision owner")?;
            let edge = projection
                .action_projection()?
                .ok_or("checkpoint callable projection has no revision edge")?;
            let to_candidate = Arc::ptr_eq(edge.target(), owner.program());
            revisions.insert(owner.revision().key(), owner.clone());
            projections.push(CallableProjectionSnapshot {
                revision: CheckpointCodeRevision::for_revision(owner),
                to_candidate,
                from: projection.source().1,
                to: projection.target().1,
            });
        }
        Ok(ScopedCallableMetadata {
            native_program,
            native_type: self.native.ty,
            projections,
        })
    }

    pub(crate) fn restore_checkpoint(
        mut program: CheckedExecutionProgram,
        ty: TypeId,
        target: CallTarget,
        projections: Vec<Arc<CallableValueProjection>>,
    ) -> Result<Self, String> {
        let Some(Type::Function(signature)) = program.checked.type_store.get(ty) else {
            return Err("checkpoint native callable has no checked function signature".into());
        };
        let signature = signature.clone();
        let root = callable_root(&program, &target)?;
        let application = validate_signature(&program.checked, root, &target, &signature)?;
        let plan = program
            .plan
            .select_callable(&program.checked, root)
            .map_err(|errors| format!("checkpoint callable plan is not executable: {errors:?}"))?;
        program.plan = Arc::new(plan);
        Ok(Self {
            native: Rc::new(NativeCallable {
                program,
                application,
                target: target.into(),
                ty,
                signature,
            }),
            projections,
        })
    }
}
