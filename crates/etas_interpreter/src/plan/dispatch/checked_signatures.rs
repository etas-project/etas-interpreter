use std::collections::{HashMap, HashSet};

use etas_frontend::CheckedProject;
use etas_hir::{HirExpr, HirExprId};
use etas_std::{FlowSourceMethodOperation, IntrinsicDispatch, StdDecl, StdIntrinsicId};

use crate::intrinsic::{
    dispatch::{CheckedStdIntrinsicCall, StdIntrinsicIdentity},
    registry::StdIntrinsicHandlerRegistry,
};

pub(super) fn lower(
    project: &CheckedProject,
    handlers: &StdIntrinsicHandlerRegistry,
    imported: &mut HashMap<StdIntrinsicId, IntrinsicDispatch>,
) -> Result<
    (
        HashMap<HirExprId, CheckedStdIntrinsicCall>,
        HashSet<HirExprId>,
    ),
    String,
> {
    let mut calls = HashMap::new();
    let mut type_members = HashSet::new();
    for (expr, fact) in &project.types.std_intrinsic_calls {
        if project.types.agent_method_calls.contains_key(expr) {
            return Err("method call has conflicting agent and standard intrinsic targets".into());
        }
        let symbol = project
            .std_registry
            .lookup_qualified(&fact.qualified_path)
            .ok_or("checked std method descriptor is absent from the canonical registry")?;
        let StdDecl::Flow(_) = &symbol.decl else {
            return Err("checked std method is not a flow".into());
        };
        let flow = project
            .std_registry
            .flow_declarations(symbol.id)
            .nth(fact.overload)
            .ok_or("checked standard call refers to an absent overload")?;
        let source = flow.source_method.as_ref();
        let descriptor = symbol
            .intrinsic
            .as_ref()
            .ok_or("checked std method has no intrinsic descriptor")?;
        let collection = match descriptor.lowering {
            etas_std::LoweringHint::Collection(operation) => Some(operation),
            _ => None,
        };
        let type_member =
            source.is_some_and(|source| source.kind == etas_std::FlowSourceMethodKind::TypeMember);
        let (receiver, method, args) = match project.hir.exprs.get(*expr) {
            Some(HirExpr::MethodCall {
                receiver,
                method,
                args,
                ..
            }) => {
                let source = source.ok_or("checked method has no registry receiver contract")?;
                if source.name != *method {
                    return Err("checked method name differs from its selected declaration".into());
                }
                if type_member {
                    let Some(HirExpr::Path(path)) = project.hir.exprs.get(*receiver) else {
                        return Err("checked type member has no resolved type subject".into());
                    };
                    let etas_hir::ResolveResult::Resolved(subject) = path.resolution else {
                        return Err("checked type member has no resolved type identity".into());
                    };
                    validate_collection_subject(project, collection, subject)?;
                }
                ((!type_member).then_some(*receiver), method, args)
            }
            Some(HirExpr::Call { callee, args, .. }) if type_member => {
                let Some(HirExpr::Path(path)) = project.hir.exprs.get(*callee) else {
                    return Err("checked type member call has no resolved type path".into());
                };
                let etas_hir::ResolveResult::PartiallyResolved(partial) = &path.resolution else {
                    return Err("checked type member call has no resolved type prefix".into());
                };
                let [method] = partial.remaining.as_slice() else {
                    return Err("checked type member call has an invalid member path".into());
                };
                validate_collection_subject(
                    project,
                    collection,
                    partial
                        .resolved_prefix
                        .ok_or("checked type member has no resolved subject")?,
                )?;
                (None, method, args)
            }
            Some(HirExpr::Call { args, .. }) if !type_member => (None, &symbol.name, args),
            _ => {
                return Err(
                    "checked std method does not refer to its checked call expression".into(),
                );
            }
        };
        if descriptor.id.0 != fact.intrinsic
            || source.is_some_and(|source| {
                source.name != *method
                    || (source.operation != FlowSourceMethodOperation::CheckedIntrinsic
                        && !(type_member && collection.is_some()))
            })
        {
            return Err("checked std method identity differs from its registry descriptor".into());
        }
        handlers.validate_descriptor(descriptor)?;
        let Some(etas_types::Type::Function(signature)) =
            project.type_store.get(fact.callable_type)
        else {
            return Err("checked std method has no materialized callable ABI".into());
        };
        if signature.input != fact.signature.params
            || signature.output != fact.signature.output
            || signature.effects != fact.signature.effects
            || fact
                .signature
                .generic_params
                .iter()
                .map(|param| &param.name)
                .collect::<Vec<_>>()
                != flow
                    .type_params
                    .iter()
                    .map(|param| &param.name)
                    .collect::<Vec<_>>()
        {
            return Err(
                "checked intrinsic signature differs from its selected overload or callable ABI"
                    .into(),
            );
        }
        if signature.input.len() != args.len() + usize::from(receiver.is_some()) {
            return Err(
                "checked std method ABI does not include its receiver and arguments".into(),
            );
        }
        if imported
            .insert(descriptor.id, descriptor.dispatch)
            .is_some_and(|previous| previous != descriptor.dispatch)
        {
            return Err("checked std method intrinsic has conflicting dispatch categories".into());
        }
        let (parameter_types, result_type) =
            if descriptor.dispatch == IntrinsicDispatch::PureKernel || collection.is_some() {
                let parameter_types = receiver
                    .into_iter()
                    .chain(args.iter().map(|arg| match arg {
                        etas_hir::HirArg::Positional(expr)
                        | etas_hir::HirArg::Named { value: expr, .. } => *expr,
                    }))
                    .map(|arg| {
                        project.types.expr_types.get(&arg).copied().ok_or_else(|| {
                            "checked pure method argument is missing its value type".to_owned()
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let result_type = project
                    .types
                    .expr_types
                    .get(expr)
                    .copied()
                    .ok_or("checked pure method is missing its result value type")?;
                (parameter_types, result_type)
            } else {
                (signature.input.clone(), signature.output)
            };
        calls.insert(
            *expr,
            CheckedStdIntrinsicCall {
                origin: *expr,
                substitutions: Vec::new(),
                identity: StdIntrinsicIdentity {
                    intrinsic: descriptor.id,
                    dispatch: descriptor.dispatch,
                },
                parameter_types,
                result_type,
            },
        );
        if type_member {
            type_members.insert(*expr);
        }
    }
    Ok((calls, type_members))
}

fn validate_collection_subject(
    project: &CheckedProject,
    collection: Option<etas_std::StdCollectionIntrinsic>,
    subject: etas_hir::SymbolId,
) -> Result<(), String> {
    let operation =
        collection.ok_or("type-member dispatch requires a checked collection descriptor")?;
    let Some(etas_hir::SymbolDef::ImportAlias { path, .. }) =
        project.symbols.get(subject).map(|symbol| &symbol.def)
    else {
        return Err("collection type member subject is not a checked std type".into());
    };
    let actual = project.std_registry.lookup_qualified(path);
    let expected =
        project
            .std_registry
            .lookup_qualified(&["std", "collections", operation.kind.name()]);
    if actual.is_none() || actual.map(|s| s.id) != expected.map(|s| s.id) {
        return Err("collection type member subject differs from its checked descriptor".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn project() -> CheckedProject {
        crate::testing::project::checked_project(
            "module app.main; import std.compiler.SyntaxBlock; agent Worker; flow unused(body: SyntaxBlock) -> unit { let d = (^Worker).with_body(body); return; } flow main() -> unit { return; }",
        )
    }

    #[test]
    fn checked_method_targets_validate_registry_identity_and_callable_abi() {
        let original = project();
        for change in [0, 1, 2, 3] {
            let mut checked = original.clone();
            let project = Arc::make_mut(&mut checked);
            let facts = Arc::make_mut(&mut project.types);
            let fact = facts.std_intrinsic_calls.values_mut().next().unwrap();
            match change {
                0 => fact.intrinsic = etas_std::intrinsic::runtime::COMMAND_RUN,
                1 => fact.qualified_path = vec!["std".into(), "missing".into()],
                2 => fact.callable_type = facts.known_std_types.code.syntax_block.unwrap(),
                3 => fact.overload = usize::MAX,
                _ => unreachable!(),
            }
            assert!(super::super::IntrinsicDispatchTable::for_project(&checked).is_err());
        }
    }

    #[test]
    fn an_unused_checked_method_is_not_checkpoint_executable() {
        let checked = project();
        let table = super::super::IntrinsicDispatchTable::for_project(&checked).unwrap();
        let error = table
            .resolve_std_callable(StdIntrinsicIdentity {
                intrinsic: StdIntrinsicId(etas_std::intrinsic::runtime::CODE_WITH_BODY),
                dispatch: IntrinsicDispatch::Runtime,
            })
            .unwrap_err();
        assert!(error.contains("not reachable"), "{error}");
    }

    #[test]
    fn pure_method_abi_requires_checked_receiver_and_result_value_types() {
        let original = crate::testing::project::checked_project(
            "module app.main; flow main() -> usize { return [1, 2].len(); }",
        );
        let expr = *original.types.std_intrinsic_calls.keys().next().unwrap();
        let HirExpr::MethodCall { receiver, .. } = &original.hir.exprs[expr] else {
            panic!("checked method expression");
        };
        let receiver = *receiver;
        let table = super::super::IntrinsicDispatchTable::for_project(&original).unwrap();
        let call = table.std_method(expr).unwrap();
        assert_eq!(call.identity.dispatch, IntrinsicDispatch::PureKernel);
        assert_eq!(
            call.identity.intrinsic.0,
            etas_std::intrinsic::pure::LIST_LEN
        );
        assert!(matches!(
            original.type_store.get(call.parameter_types[0]),
            Some(etas_types::Type::Array(_))
        ));
        assert_eq!(call.result_type, original.types.expr_types[&expr]);

        for missing in [receiver, expr] {
            let mut corrupt = original.clone();
            let checked = Arc::make_mut(&mut corrupt);
            Arc::make_mut(&mut checked.types)
                .expr_types
                .remove(&missing);
            let error = super::super::IntrinsicDispatchTable::for_project(&corrupt).unwrap_err();
            assert!(
                error.iter().any(|message| message.contains("missing")),
                "{error:?}"
            );
        }
    }
}
