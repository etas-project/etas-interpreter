use etas_frontend::CheckedProject;
use etas_hir::{HirExpr, ResolveResult};

pub(super) fn validate(project: &CheckedProject) -> Result<(), String> {
    for (expr, node) in project.hir.exprs.iter() {
        let HirExpr::Perform { action, .. } = node else {
            continue;
        };
        if !project.types.expr_types.contains_key(&expr) {
            continue;
        }
        if let ResolveResult::Resolved(symbol) = action.action_symbol
            && let Some(target) = standard_target(project, symbol)
            && project.std_registry.action_declarations(target).count() > 1
            && !project.types.action_calls.contains_key(&expr)
        {
            return Err("overloaded perform is missing its checked signature selection".into());
        }
    }
    for (expr, fact) in &project.types.action_calls {
        let Some(HirExpr::Perform { action, args, .. }) = project.hir.exprs.get(*expr) else {
            return Err("checked action overload does not identify a perform expression".into());
        };
        if action.action_symbol != ResolveResult::Resolved(fact.action_symbol)
            || standard_target(project, fact.action_symbol) != Some(fact.target)
        {
            return Err("checked action overload differs from its resolved action symbol".into());
        }
        let declaration = project
            .std_registry
            .action_declarations(fact.target)
            .nth(fact.overload)
            .ok_or("checked action refers to an absent registry overload")?;
        let performed = project
            .effects
            .performed_actions
            .get(expr)
            .ok_or("checked action overload is missing its solved performed-action fact")?;
        let identity = match &performed.invocation {
            etas_effects::Effect::Action(action) => action,
            etas_effects::Effect::AppliedAction(action) => &action.action,
            _ => return Err("checked action overload has no action invocation identity".into()),
        };
        if declaration.stable_id != Some(identity.action.0)
            || fact.signature.params.len() != args.len()
            || project.types.expr_types.get(expr) != Some(&fact.signature.output)
            || fact
                .signature
                .generic_params
                .iter()
                .map(|param| &param.name)
                .collect::<Vec<_>>()
                != declaration
                    .type_params
                    .iter()
                    .map(|param| &param.name)
                    .collect::<Vec<_>>()
        {
            return Err("checked action overload disagrees with its registry identity or payload/result ABI".into());
        }
    }
    Ok(())
}

fn standard_target(
    project: &CheckedProject,
    symbol: etas_hir::SymbolId,
) -> Option<etas_std::StdSymbolId> {
    if let Some(target) = project.hir.standard_action_targets.get(&symbol) {
        return Some(*target);
    }
    let etas_hir::SymbolDef::ImportAlias { path, .. } = &project.hir.symbols.get(symbol)?.def
    else {
        return None;
    };
    project
        .std_registry
        .lookup_qualified(path)
        .map(|symbol| symbol.id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn direct_action_overload_is_required_and_bound_to_the_current_registry_and_result() {
        let original = crate::testing::project::checked_project(
            "module app.main; flow target(value: string) -> string ![] { return value; } flow main() -> unit { let report = (perform Code.check(^target))?; return; }",
        );
        assert_eq!(original.types.action_calls.len(), 1);
        for mutation in 0..4 {
            let mut corrupt = original.clone();
            let checked = Arc::make_mut(&mut corrupt);
            let types = Arc::make_mut(&mut checked.types);
            match mutation {
                0 => types.action_calls.clear(),
                1 => types.action_calls.values_mut().next().unwrap().overload = usize::MAX,
                2 => {
                    types.action_calls.values_mut().next().unwrap().target =
                        etas_std::StdSymbolId(u32::MAX)
                }
                3 => {
                    types
                        .action_calls
                        .values_mut()
                        .next()
                        .unwrap()
                        .signature
                        .output = types.known_std_types.code.syntax_block.unwrap()
                }
                _ => unreachable!(),
            }
            assert!(validate(&corrupt).is_err(), "mutation {mutation}");
        }
        validate(&original).unwrap();
    }
}
