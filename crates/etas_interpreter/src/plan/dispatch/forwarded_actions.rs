use std::collections::HashMap;

use etas_effects::{ActionRef, EffectActionId};
use etas_frontend::CheckedProject;
use etas_std::{IntrinsicDispatch, LoweringHint, StdDecl, StdIntrinsicId};

use crate::intrinsic::dispatch::{StdCallable, StdIntrinsicIdentity};

#[derive(Clone, Debug)]
pub(crate) struct CheckedForwardedAction {
    pub action: ActionRef,
    pub owner_path: Vec<String>,
    pub name: String,
}

#[derive(Clone, Debug)]
pub(crate) struct CheckedCompilerAction {
    pub operation: etas_host::compiler::CompilerOperation,
    pub signature_path: String,
}

pub(super) fn lower_compilers(
    project: &CheckedProject,
) -> Result<HashMap<ActionRef, CheckedCompilerAction>, String> {
    let mut targets = HashMap::new();
    for symbol in project.std_registry.symbols() {
        let Some(descriptor) = &symbol.intrinsic else {
            continue;
        };
        let operation = match descriptor.id.0 {
            etas_std::intrinsic::runtime::CODE_CHECK => {
                etas_host::compiler::CompilerOperation::Check
            }
            etas_std::intrinsic::runtime::CODE_COMPILE => {
                etas_host::compiler::CompilerOperation::Compile
            }
            _ => continue,
        };
        let LoweringHint::ForwardAction { stable_id } = descriptor.lowering else {
            return Err("compiler intrinsic has no registered action-forwarding contract".into());
        };
        let signature = project
            .effect_registry
            .standard_action(EffectActionId(stable_id))
            .ok_or("compiler intrinsic has no canonical action target")?;
        let action = project.std_registry.symbols().find(|symbol| matches!(&symbol.decl, StdDecl::EffectAction(action) if action.stable_id == Some(stable_id)))
            .ok_or("compiler intrinsic references an absent standard action")?;
        targets.insert(
            ActionRef {
                tag: signature.owner,
                action: signature.id,
            },
            CheckedCompilerAction {
                operation,
                signature_path: action.qualified_path.join("."),
            },
        );
    }
    Ok(targets)
}

pub(super) fn lower(
    project: &CheckedProject,
    imported: &HashMap<StdIntrinsicId, IntrinsicDispatch>,
) -> Result<HashMap<StdIntrinsicId, CheckedForwardedAction>, String> {
    project
        .std_registry
        .validate()
        .map_err(|error| error.to_string())?;
    let mut targets = HashMap::new();
    for symbol in project.std_registry.symbols() {
        let Some(descriptor) = symbol.intrinsic.as_ref() else {
            continue;
        };
        if !imported.contains_key(&descriptor.id) {
            continue;
        }
        let LoweringHint::ForwardAction { stable_id } = descriptor.lowering else {
            continue;
        };
        let signature = project
            .effect_registry
            .standard_action(EffectActionId(stable_id))
            .ok_or("std action forwarder has no checked effect registry target")?;
        let action_symbol = project.std_registry.symbols().find(|symbol| matches!(
            &symbol.decl, StdDecl::EffectAction(action) if action.stable_id == Some(stable_id)
        )).ok_or("std action forwarder is absent from the canonical registry")?;
        let (name, owner_path) = action_symbol
            .qualified_path
            .split_last()
            .ok_or("std action forwarder has no canonical owner path")?;
        if owner_path.is_empty()
            || !signature.effect_args.is_empty()
            || !signature.has_default_handler
        {
            return Err(
                "std action forwarder has an invalid checked selector/default contract".into(),
            );
        }
        targets.insert(
            descriptor.id,
            CheckedForwardedAction {
                action: ActionRef {
                    tag: signature.owner,
                    action: signature.id,
                },
                owner_path: owner_path.to_vec(),
                name: name.clone(),
            },
        );
    }
    Ok(targets)
}

impl super::IntrinsicDispatchTable {
    pub(crate) fn compiler_action(&self, action: &ActionRef) -> Option<&CheckedCompilerAction> {
        self.program.compiler_actions.get(action)
    }
    pub(crate) fn forwarded_action(
        &self,
        identity: StdIntrinsicIdentity,
    ) -> Result<&CheckedForwardedAction, String> {
        if self.resolve_std_callable(identity)? != StdCallable::ForwardAction {
            return Err("checked intrinsic is not an action forwarder".into());
        }
        self.program
            .forwarded_actions
            .get(&identity.intrinsic)
            .ok_or_else(|| {
                "checked action forwarder target is missing from the current plan".into()
            })
    }
}

#[cfg(test)]
mod tests {
    use super::super::IntrinsicDispatchTable;
    use super::*;

    #[test]
    fn action_forwarder_requires_a_current_checked_effect_registry_target() {
        let mut checked = crate::testing::project::checked_project(
            r#"
module app.main;
import std.compiler.check;
agent Worker() -> unit { return; }
flow main() -> i32 {
    return handle { let report = check(^Worker); return 99; }
        with { Code.check(_) => { finish 7; } };
}
"#,
        );
        std::sync::Arc::make_mut(&mut checked).effect_registry =
            std::sync::Arc::new(Default::default());
        let errors = IntrinsicDispatchTable::for_project(&checked).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|error| error.contains("checked effect registry target")),
            "{errors:?}"
        );
    }

    #[test]
    fn an_imported_but_unreachable_forwarder_cannot_be_activated() {
        let checked = crate::testing::project::checked_project(
            r#"
module app.main;
import std.compiler.compile;
agent Worker() -> unit { return; }
flow unused() -> i32 {
    return handle { let candidate = compile(^Worker); return 99; }
        with { Code.compile(_) => { finish 7; } };
}
flow main() -> unit { return; }
"#,
        );
        let table = IntrinsicDispatchTable::for_project(&checked).unwrap();
        let error = table
            .forwarded_action(StdIntrinsicIdentity {
                intrinsic: StdIntrinsicId(etas_std::intrinsic::runtime::CODE_COMPILE),
                dispatch: IntrinsicDispatch::Host,
            })
            .unwrap_err();
        assert!(error.contains("not reachable"), "{error}");
    }
}
