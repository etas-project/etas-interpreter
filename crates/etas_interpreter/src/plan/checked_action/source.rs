use std::collections::HashMap;

use etas_hir::{HirExpr, HirExprId, ResolveResult};

use super::CheckedActionAbi;
use crate::value::CheckedExecutionProgram;

pub(crate) fn source_action(
    execution: &CheckedExecutionProgram,
    expr: HirExprId,
    bindings: &crate::control::GenericBindings,
) -> Result<CheckedActionAbi, String> {
    let checked = &execution.checked;
    if bindings
        .types
        .iter()
        .any(|(name, ty)| name.is_empty() || checked.type_store.get(*ty).is_none())
    {
        return Err("action origin has an invalid checked type binding".into());
    }
    if bindings.parameters.iter().any(|(parameter, argument)| {
        !crate::control::GenericBindings::is_checked_parameter(checked, *parameter)
            || checked.type_store.get(*argument).is_none()
    }) {
        return Err("action origin has an invalid checked owner binding".into());
    }
    let (action, args) = match checked.hir.exprs.get(expr) {
        Some(HirExpr::Perform { action, args, .. }) => (action, Some(args)),
        Some(HirExpr::Infer { action, .. }) => (action, None),
        _ => return Err("action origin is not a checked perform/inference expression".into()),
    };
    let ResolveResult::Resolved(symbol) = action.action_symbol else {
        return Err("perform has no checked action symbol".into());
    };
    let signature = checked
        .types
        .action_signatures
        .get(&symbol)
        .ok_or("perform has no checked action signature")?;
    let (parameters, output, generic_params, returns_never) =
        match checked.types.action_calls.get(&expr) {
            Some(selected) => (
                &selected.signature.params,
                selected.signature.output,
                &selected.signature.generic_params,
                matches!(
                    checked.type_store.get(selected.signature.output),
                    Some(etas_types::Type::Primitive(
                        etas_types::PrimitiveType::Never
                    ))
                ),
            ),
            None => (
                &signature.params,
                signature.output,
                &signature.generic_params,
                signature.returns_never,
            ),
        };
    let fact = checked
        .effects
        .performed_actions
        .get(&expr)
        .ok_or("perform has no checked action instance")?;
    if fact.expr != expr
        || fact.action_symbol != Some(symbol)
        || fact.action != action.action
        || !fact.invocation.is_action()
        || !fact
            .summary
            .requested_actions
            .effects
            .contains(&fact.invocation)
    {
        return Err("perform invocation disagrees with its checked action fact".into());
    }
    if let Some(args) = args {
        if args.len() != parameters.len()
            || args.len() != fact.args.len()
            || args.iter().zip(&fact.args).any(|(arg, expected)| {
                let (etas_hir::HirArg::Positional(value) | etas_hir::HirArg::Named { value, .. }) =
                    arg;
                value != expected
            })
        {
            return Err("perform payload differs from its checked argument descriptors".into());
        }
    } else {
        let inference = checked
            .types
            .agent_inferences
            .get(&expr)
            .ok_or("inference origin has no checked schema/output fact")?;
        if inference.action_symbol != symbol
            || checked.type_store.get(inference.schema)
                != Some(&etas_types::Type::Schema(inference.output))
        {
            return Err("inference origin disagrees with its checked schema or action".into());
        }
    }
    let substitute_caller = |ty| {
        bindings
            .substitution(&checked.type_store)
            .substitute(ty)
            .map_err(|error| error.to_string())
    };
    let mut action_bindings = bindings.clone();
    if args.is_some() {
        if let Some(instantiation) = checked.types.generic_instantiations.get(&expr) {
            for (name, ty) in &instantiation.type_bindings {
                let actual = substitute_caller(*ty)?;
                etas_types::TypeSubstitutionEngine::existing(&checked.type_store, &HashMap::new())
                    .validate_closed_argument(actual)
                    .map_err(|error| {
                        format!("action origin generic argument is not closed: {error}")
                    })?;
                let parameter = generic_params
                    .iter()
                    .find(|parameter| {
                        parameter.name == *name
                            && parameter.kind == etas_types::CallableGenericParamKind::Type
                    })
                    .ok_or("action origin references an undeclared type parameter")?;
                action_bindings.types.insert(name.clone(), actual);
                action_bindings.parameters.insert(parameter.subject, actual);
            }
        }
    }
    let substitute = |ty| {
        action_bindings
            .substitution(&checked.type_store)
            .substitute(ty)
            .map_err(|error| error.to_string())
    };
    let mut obligations = Vec::new();
    for param in generic_params {
        if param.bounds.is_empty() {
            continue;
        }
        let actual = action_bindings
            .get(&param.name)
            .copied()
            .ok_or("action origin is missing a bounded generic argument")?;
        for bound in &param.bounds {
            obligations.push(etas_types::SpecObligation {
                ty: actual,
                spec: bound.spec.clone(),
                args: bound
                    .args
                    .iter()
                    .map(|ty| substitute(*ty))
                    .collect::<Result<_, _>>()?,
                span: action.span,
            });
        }
    }
    if !obligations.is_empty() {
        let report = etas_types::solver::spec_solver::solve_spec_obligations(
            &checked.type_store,
            execution.plan.checked_spec_facts(),
            &obligations,
            &action_bindings.types,
        );
        if let Some(failure) = report.failures.first() {
            return Err(format!(
                "action origin violates a checked spec bound: {}",
                failure.message
            ));
        }
    }
    let mut effect = fact.invocation.clone();
    if let etas_effects::Effect::AppliedAction(instance) = &mut effect {
        for arg in &mut instance.args {
            if let etas_types::EffectArgRef::Type(ty) = arg {
                *ty = substitute_caller(*ty)?;
            }
        }
    }
    let abi = CheckedActionAbi {
        effect,
        parameters: parameters
            .iter()
            .map(|ty| substitute(*ty))
            .collect::<Result<_, _>>()?,
        output: substitute(output)?,
        returns_never,
    };
    if args.is_none() {
        let inference = checked
            .types
            .agent_inferences
            .get(&expr)
            .ok_or("inference origin has no checked schema/output fact")?;
        let [prompt, schema] = abi.parameters.as_slice() else {
            return Err("inference action has no checked Prompt/Schema payload contract".into());
        };
        if checked.type_store.get(*prompt) != Some(&etas_types::Type::Prompt)
            || *schema != inference.schema
            || abi.output != inference.output
        {
            return Err(
                "inference action origin disagrees with its checked payload/output facts".into(),
            );
        }
    }
    Ok(abi)
}
