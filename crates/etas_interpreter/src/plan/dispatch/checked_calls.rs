use super::*;

pub(super) fn lower(
    project: &CheckedProject,
    symbols: &HashMap<SymbolId, StdIntrinsicIdentity>,
    methods: &HashMap<HirExprId, CheckedStdIntrinsicCall>,
) -> Result<HashMap<HirExprId, CheckedStdIntrinsicCall>, String> {
    let mut origins = HashMap::new();
    for (expr, node) in project.hir.exprs.iter() {
        let path = match node {
            HirExpr::Call { callee, .. } => match project.hir.exprs.get(*callee) {
                Some(HirExpr::Path(path)) => path,
                _ => continue,
            },
            HirExpr::Path(path) => path,
            _ => continue,
        };
        let ResolveResult::Resolved(symbol) = path.resolution else {
            continue;
        };
        let Some(identity) = symbols.get(&symbol) else {
            continue;
        };
        if identity.dispatch == IntrinsicDispatch::PureKernel {
            continue;
        }
        let Some(ty) = project.types.expr_types.get(&expr) else {
            continue;
        };
        let (parameter_types, result_type) = match node {
            HirExpr::Call { args, .. } => {
                let parameters = args
                    .iter()
                    .map(|arg| match arg {
                        etas_hir::HirArg::Positional(expr)
                        | etas_hir::HirArg::Named { value: expr, .. } => {
                            project.types.expr_types.get(expr).copied().ok_or_else(|| {
                                "standard intrinsic origin is missing a checked argument type"
                                    .to_owned()
                            })
                        }
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                (parameters, *ty)
            }
            HirExpr::Path(_) => {
                let Some(etas_types::Type::Function(signature)) = project.type_store.get(*ty)
                else {
                    return Err(
                        "standard intrinsic reference origin is missing its checked function type"
                            .into(),
                    );
                };
                (signature.input.clone(), signature.output)
            }
            _ => return Err("standard intrinsic origin is not a callable expression".into()),
        };
        origins.insert(
            expr,
            CheckedStdIntrinsicCall {
                identity: *identity,
                origin: expr,
                substitutions: Vec::new(),
                parameter_types,
                result_type,
            },
        );
    }
    origins.extend(methods.iter().map(|(expr, call)| (*expr, call.clone())));
    Ok(origins)
}

impl IntrinsicDispatchTable {
    pub(crate) fn validate_std_call_origin(
        &self,
        origin: HirExprId,
        identity: StdIntrinsicIdentity,
    ) -> Result<(), String> {
        self.resolve_std_callable(identity)?;
        match self.program.std_call_origins.get(&origin) {
            Some(expected) if expected.identity == identity => Ok(()),
            Some(_) => Err("standard intrinsic origin resolves to a different descriptor".into()),
            None => Err("standard intrinsic origin has no checked descriptor binding".into()),
        }
    }

    pub(crate) fn validate_std_call_template(
        &self,
        origin: HirExprId,
        identity: StdIntrinsicIdentity,
        substitutions: &[crate::intrinsic::dispatch::IntrinsicSubstitutionStep],
        parameters: &[etas_types::TypeId],
        result: etas_types::TypeId,
        store: &etas_types::TypeStore,
    ) -> Result<(), String> {
        self.validate_std_call_origin(origin, identity)?;
        let call = self
            .program
            .std_call_origins
            .get(&origin)
            .ok_or("standard intrinsic origin has no checked descriptor binding")?;
        let input = call
            .parameter_types
            .iter()
            .map(|ty| CheckedStdIntrinsicCall::instantiate_type_with(store, *ty, substitutions))
            .collect::<Result<Vec<_>, _>>()?;
        let output =
            CheckedStdIntrinsicCall::instantiate_type_with(store, call.result_type, substitutions)?;
        if input.len() != parameters.len()
            || input
                .iter()
                .zip(parameters)
                .any(|(left, right)| !same_abi_type(store, *left, *right))
            || !same_abi_type(store, output, result)
        {
            return Err("standard intrinsic ABI differs from its checked origin".into());
        }
        Ok(())
    }
}

fn same_abi_type(
    store: &etas_types::TypeStore,
    left: etas_types::TypeId,
    right: etas_types::TypeId,
) -> bool {
    if left == right {
        return true;
    }
    match (store.get(left), store.get(right)) {
        (Some(etas_types::Type::Function(left)), Some(etas_types::Type::Function(right))) => {
            let pure = |row: &Option<etas_types::EffectRowRef>| {
                row.as_ref()
                    .is_none_or(|row| row.tail.is_none() && row.effects.is_empty())
            };
            // Named pure flows and inferred closures can spell the same empty
            // row as None or Some([]); their input and output ABI stays exact.
            left.input == right.input
                && left.output == right.output
                && pure(&left.effects)
                && pure(&right.effects)
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_call_origins_require_the_current_resolved_descriptor() {
        let project = crate::testing::project::checked_project(
            "module app.main; import std.io.{println, eprintln}; flow main() -> unit { println(\"out\"); eprintln(\"err\"); return; }",
        );
        let table = IntrinsicDispatchTable::for_project(&project).unwrap();
        let entries = table.program.std_call_origins.iter().collect::<Vec<_>>();
        let (origin, call) = entries[0];
        let identity = call.identity;
        table.validate_std_call_origin(*origin, identity).unwrap();
        let (_, other) = entries
            .iter()
            .find(|(_, other)| other.identity != identity)
            .unwrap();
        let error = table
            .validate_std_call_origin(*origin, other.identity)
            .unwrap_err();
        assert!(error.contains("different descriptor"), "{error}");
        let error = table
            .validate_std_call_origin(HirExprId(u32::MAX), identity)
            .unwrap_err();
        assert!(error.contains("no checked descriptor binding"), "{error}");
    }

    #[test]
    fn snapshot_validation_rejects_an_intrinsic_with_a_forged_origin() {
        use crate::{
            eval::machine::snapshot::SnapshotValidator,
            orchestration::{
                CallTargetSnapshot, ContinuationSnapshot, MachineFrameSnapshot, MachineSnapshot,
            },
        };
        let project = crate::testing::project::checked_project(
            "module app.main; import std.io.println; flow main() -> unit { println(\"out\"); return; }",
        );
        let plan = crate::Interpreter
            .plan(&project, crate::api::PlanOptions)
            .plan
            .unwrap();
        let limits = etas_host::StorageLimits::default();
        let validator = SnapshotValidator::new(
            &project,
            &plan.slots,
            &plan.dispatch,
            &plan.closures,
            &limits,
        );
        let (origin, args) = project
            .hir
            .exprs
            .iter()
            .find_map(|(expr, node)| match node {
                HirExpr::Call { args, .. } => Some((expr, args)),
                _ => None,
            })
            .unwrap();
        let identity = plan.dispatch.program.std_call_origins[&origin].identity;
        let parameters = args
            .iter()
            .map(|arg| match arg {
                etas_hir::HirArg::Positional(expr)
                | etas_hir::HirArg::Named { value: expr, .. } => project.types.expr_types[expr],
            })
            .collect::<Vec<_>>();
        let result = project.types.expr_types[&origin];
        let span = project.hir.blocks.iter().next().unwrap().1.span;
        for (origin, parameters, result, valid) in [
            (origin, parameters.clone(), result, true),
            (HirExprId(u32::MAX), parameters.clone(), result, false),
            (origin, vec![result], result, false),
            (origin, parameters.clone(), parameters[0], false),
        ] {
            let target = CallTargetSnapshot::StdIntrinsic {
                origin,
                substitutions: Vec::new(),
                intrinsic: identity.intrinsic,
                dispatch: identity.dispatch,
                parameter_types: parameters,
                result_type: result,
            };
            let machine = MachineSnapshot {
                auxiliary: Vec::new(),
                input: None,
                program_owners: Default::default(),
                suspended_programs: Vec::new(),
                frames: vec![MachineFrameSnapshot::Continuation {
                    continuation: ContinuationSnapshot::ComposedCall {
                        site: None,
                        remaining: vec![target],
                        span,
                    },
                }],
                residual: Default::default(),
            };
            let actual = validator.validate_machine(&machine);
            assert_eq!(actual.is_ok(), valid, "{actual:?}");
        }
    }
}
