use super::*;
use std::collections::HashMap;

pub(crate) fn validate_snapshot_signature(
    checked: &CheckedProgram,
    root: CheckedCallableRoot,
    expected: &FlowType,
    bindings: &crate::control::GenericBindings,
    specs: &etas_types::SpecFacts,
) -> Result<(), String> {
    if let CheckedCallableRoot::Item(item) = root {
        let callable = match checked.types.item_signatures.get(&item) {
            Some(
                etas_types::ItemSignature::Flow(callable)
                | etas_types::ItemSignature::Tool(callable),
            ) => Some(callable),
            Some(etas_types::ItemSignature::Agent(agent)) => agent.callable(),
            _ => None,
        }
        .ok_or("checkpoint callable has no checked item signature")?;
        for name in bindings.rows.keys() {
            if !callable.generic_params.iter().any(|param| {
                param.name == *name && param.kind == etas_types::CallableGenericParamKind::Effect
            }) {
                return Err(
                    "checkpoint callable has an unknown or wrong-kind effect-row argument".into(),
                );
            }
        }
        for param in &callable.generic_params {
            if param.kind == etas_types::CallableGenericParamKind::Effect
                && !bindings.rows.contains_key(&param.name)
            {
                return Err("checkpoint callable is missing a checked effect-row argument".into());
            }
        }
        validate_bounds(
            checked,
            callable,
            checked.hir.items[item].span(),
            bindings,
            specs,
        )?;
    }
    validate_signature_bindings(checked, root, expected, bindings)?;
    super::effects::validate_snapshot_effects(checked, root, expected, bindings)
}

pub(crate) fn validate_bounds(
    checked: &CheckedProgram,
    callable: &etas_types::CallableSignature,
    span: etas_core::Span,
    bindings: &HashMap<String, TypeId>,
    specs: &etas_types::SpecFacts,
) -> Result<(), String> {
    let mut engine = etas_types::TypeSubstitutionEngine::existing(&checked.type_store, bindings);
    let mut obligations = Vec::new();
    for param in &callable.generic_params {
        if param.bounds.is_empty() {
            continue;
        }
        let actual = *bindings
            .get(&param.name)
            .ok_or("checkpoint callable is missing a bounded generic argument")?;
        for bound in &param.bounds {
            obligations.push(etas_types::SpecObligation {
                ty: actual,
                spec: bound.spec.clone(),
                args: bound
                    .args
                    .iter()
                    .map(|ty| engine.substitute(*ty).map_err(|error| error.to_string()))
                    .collect::<Result<_, _>>()?,
                span,
            });
        }
    }
    if !obligations.is_empty() {
        let report = etas_types::solver::spec_solver::solve_spec_obligations(
            &checked.type_store,
            specs,
            &obligations,
            bindings,
        );
        if let Some(failure) = report.failures.first() {
            return Err(format!(
                "checkpoint callable violates a checked spec bound: {}",
                failure.message
            ));
        }
    }
    Ok(())
}
