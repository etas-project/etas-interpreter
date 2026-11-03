use etas_frontend::CheckedProject;
use etas_hir::{HirArg, HirExpr, HirExprId};
use etas_types::{
    Type, TypeId,
    facts::reflection::{
        DefinitionFamily, DefinitionOperation as Op, DefinitionOperationSelection as Selection,
    },
};

pub(super) fn validate(project: &CheckedProject) -> Result<(), String> {
    for (expr, fact) in &project.types.std_intrinsic_calls {
        if operation(fact.intrinsic).is_some()
            && !project.types.definition_operations.contains_key(expr)
        {
            return Err(
                "definition intrinsic is missing its checked contract-origin recipe".into(),
            );
        }
    }
    let compile_action = project
        .std_registry
        .intrinsic(etas_std::StdIntrinsicId(
            etas_std::intrinsic::runtime::CODE_COMPILE,
        ))
        .and_then(|descriptor| match descriptor.lowering {
            etas_std::LoweringHint::ForwardAction { stable_id } => Some(stable_id),
            _ => None,
        });
    for (expr, fact) in &project.types.action_calls {
        if let Some(etas_std::StdDecl::EffectAction(action)) = project
            .std_registry
            .symbol(fact.target)
            .map(|symbol| &symbol.decl)
            && compile_action.is_some()
            && action.stable_id == compile_action
            && !project.types.definition_operations.contains_key(expr)
        {
            return Err("compile action is missing its checked contract-origin recipe".into());
        }
    }
    for (expr, recipe) in &project.types.definition_operations {
        let signature = match recipe.selection {
            Selection::Intrinsic {
                intrinsic,
                overload,
            } => {
                let fact = project
                    .types
                    .std_intrinsic_calls
                    .get(expr)
                    .ok_or("definition recipe has no selected intrinsic")?;
                if fact.intrinsic != intrinsic
                    || fact.overload != overload
                    || operation(intrinsic) != Some(recipe.operation)
                {
                    return Err("definition recipe differs from its selected intrinsic".into());
                }
                &fact.signature
            }
            Selection::Action { target, overload } => {
                let fact = project
                    .types
                    .action_calls
                    .get(expr)
                    .ok_or("definition recipe has no selected action")?;
                let declaration = project
                    .std_registry
                    .action_declarations(target)
                    .nth(overload)
                    .ok_or("definition recipe has no registered action overload")?;
                if fact.target != target
                    || fact.overload != overload
                    || recipe.operation != Op::Compile
                    || compile_action.is_none()
                    || declaration.stable_id != compile_action
                {
                    return Err("definition recipe differs from its selected action".into());
                }
                &fact.signature
            }
        };
        if input(project, *expr) != Some(recipe.input)
            || recipe.instantiation != *expr
            || !project
                .types
                .generic_instantiations
                .contains_key(&recipe.instantiation)
        {
            return Err(
                "definition recipe has invalid input or generic/row binding provenance".into(),
            );
        }
        let Some(Type::Applied { constructor, args }) = signature
            .params
            .first()
            .and_then(|ty| project.type_store.get(*ty))
        else {
            return Err("definition recipe has no typed descriptor parameter".into());
        };
        let code = &project.types.known_std_types.code;
        let family = if code.agent_definition == Some(TypeId(constructor.0)) {
            DefinitionFamily::Agent
        } else if code.callable_definition == Some(TypeId(constructor.0)) {
            DefinitionFamily::Callable
        } else {
            return Err("definition recipe names a non-descriptor contract".into());
        };
        if family != recipe.family || args.as_slice() != [recipe.contract_type] {
            return Err(
                "definition recipe has an inconsistent callable family or interface".into(),
            );
        }
    }
    Ok(())
}

fn operation(intrinsic: u32) -> Option<Op> {
    match intrinsic {
        etas_std::intrinsic::runtime::CODE_COMPILE => Some(Op::Compile),
        etas_std::intrinsic::runtime::CODE_WITH_BODY => Some(Op::WithBody),
        _ => None,
    }
}

fn input(project: &CheckedProject, expr: HirExprId) -> Option<HirExprId> {
    match project.hir.exprs.get(expr)? {
        HirExpr::MethodCall { receiver, .. } => Some(*receiver),
        HirExpr::Call { args, .. } | HirExpr::Perform { args, .. } => {
            args.first().map(|arg| match arg {
                HirArg::Positional(value) | HirArg::Named { value, .. } => *value,
            })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn definition_contract_recipes_are_required_and_bound_to_checked_input_and_overload() {
        let project = crate::testing::project::checked_project(
            "module app.main; import std.compiler.{compile, CompileError}; flow target() -> unit ![] { return; } flow main() -> unit ![Error<CompileError>] { let candidate = compile(^target); candidate(); return; }",
        );
        validate(&project).unwrap();
        for mutation in 0..5 {
            let mut damaged = project.clone();
            let facts = Arc::make_mut(&mut Arc::make_mut(&mut damaged).types);
            match mutation {
                0 => facts.definition_operations.clear(),
                1 => {
                    facts
                        .definition_operations
                        .values_mut()
                        .next()
                        .unwrap()
                        .input = HirExprId(u32::MAX)
                }
                2 => {
                    facts
                        .definition_operations
                        .values_mut()
                        .next()
                        .unwrap()
                        .family = DefinitionFamily::Agent
                }
                3 => {
                    facts
                        .definition_operations
                        .values_mut()
                        .next()
                        .unwrap()
                        .contract_type = facts.known_std_types.code.syntax_block.unwrap()
                }
                4 => facts.generic_instantiations.clear(),
                _ => unreachable!(),
            }
            assert!(validate(&damaged).is_err(), "mutation {mutation}");
        }
    }
}
