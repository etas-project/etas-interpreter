use super::*;
use etas_std::{LoweringHint, StdDecl};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CheckedDefaultAction {
    pub invocation: etas_effects::Effect,
    pub payload: Vec<usize>,
    pub parameters: Vec<etas_types::TypeId>,
    pub output: etas_types::TypeId,
    pub action_output: etas_types::TypeId,
    pub reducer: Option<etas_std::StdActionReducer>,
}

type ActionTable = HashMap<HirExprId, CheckedDefaultAction>;

#[derive(Clone, PartialEq, Eq)]
struct Adapter {
    action: etas_effects::ActionRef,
    payload: Vec<usize>,
    selectors: Vec<etas_std::StdStaticArg>,
    action_parameters: Vec<etas_types::TypeId>,
    action_output: etas_types::TypeId,
    reducer: Option<etas_std::StdActionReducer>,
}

pub(super) fn lower(
    project: &CheckedProject,
    imported: &HashMap<StdIntrinsicId, IntrinsicDispatch>,
    origins: &HashMap<HirExprId, CheckedStdIntrinsicCall>,
) -> Result<(ActionTable, HashSet<StdIntrinsicId>), String> {
    let mut adapters = HashMap::new();
    for symbol in project.std_registry.symbols() {
        let Some(descriptor) = &symbol.intrinsic else {
            continue;
        };
        let (stable_id, payload, reducer) = match &descriptor.lowering {
            LoweringHint::DefaultAction { stable_id, payload } => (stable_id, payload, None),
            LoweringHint::ActionReducer {
                stable_id,
                payload,
                reducer,
            } => (stable_id, payload, Some(reducer.clone())),
            _ => continue,
        };
        if !imported.contains_key(&descriptor.id) {
            continue;
        }
        let StdDecl::Flow(flow) = &symbol.decl else {
            return Err("standard action adapter has no flow declaration".into());
        };
        let signature = project
            .effect_registry
            .standard_action(etas_effects::EffectActionId(*stable_id))
            .ok_or("standard action adapter has no checked registry declaration")?;
        if !signature.has_default_handler {
            return Err("standard action adapter has no checked default handler".into());
        }
        let declaration = project.std_registry.symbols().find(|symbol| matches!(&symbol.decl, StdDecl::EffectAction(action) if action.stable_id == Some(*stable_id))).ok_or("standard action adapter has no canonical declaration")?;
        let typed = project
            .types
            .qualified_action_signatures
            .get(&declaration.qualified_path.join("."))
            .ok_or("standard action adapter has no checked action signature")?;
        if typed.params.len() != payload.len() {
            return Err(
                "standard action payload projection differs from its checked declaration".into(),
            );
        }
        let action = etas_effects::ActionRef {
            tag: signature.owner,
            action: signature.id,
        };
        let [requested] = flow.requested_actions.as_slice() else {
            return Err("standard action adapter has no exact request contract".into());
        };
        let adapter = Adapter {
            action,
            payload: payload.clone(),
            selectors: requested.args.clone(),
            action_parameters: typed.params.clone(),
            action_output: typed.output,
            reducer,
        };
        if adapters
            .insert(descriptor.id, adapter.clone())
            .is_some_and(|previous| previous != adapter)
        {
            return Err("standard intrinsic has conflicting action adapters".into());
        }
    }
    let mut calls = HashMap::new();
    for (expr, fact) in &project.effects.std_action_calls {
        let id = StdIntrinsicId(fact.intrinsic);
        let origin = origins
            .get(expr)
            .ok_or("checked standard action fact has no resolved intrinsic origin")?;
        if origin.identity.intrinsic != id {
            return Err(
                "checked standard action fact differs from its resolved intrinsic origin".into(),
            );
        }
        if origin.parameter_types != fact.parameters || origin.result_type != fact.output {
            return Err(
                "checked standard action ABI differs from its resolved intrinsic origin".into(),
            );
        }
        let adapter = adapters
            .get(&id)
            .ok_or("checked standard action fact has no registered adapter")?;
        let args = checked_selectors(project, *expr, &adapter.selectors)?;
        let expected = if args.is_empty() {
            etas_effects::Effect::Action(adapter.action.clone())
        } else {
            etas_effects::Effect::AppliedAction(etas_effects::ActionInstanceRef {
                action: adapter.action.clone(),
                args,
            })
        };
        if fact.invocation != expected {
            return Err(
                "checked standard action instance differs from its generic instantiation".into(),
            );
        }
        if adapter
            .payload
            .iter()
            .any(|index| *index >= fact.parameters.len())
        {
            return Err(
                "checked standard action fact has an inconsistent payload or action identity"
                    .into(),
            );
        }
        let bindings = project
            .types
            .generic_instantiations
            .get(expr)
            .map(|fact| fact.type_bindings.iter().cloned().collect())
            .unwrap_or_default();
        let substitute = |ty| {
            etas_types::substitute_named_params_in_store(&project.type_store, ty, &bindings)
                .map_err(|error| format!("standard action ABI is not materialized: {error}"))
        };
        let action_parameters = adapter
            .action_parameters
            .iter()
            .map(|ty| substitute(*ty))
            .collect::<Result<Vec<_>, _>>()?;
        let payload_parameters = adapter
            .payload
            .iter()
            .map(|index| fact.parameters[*index])
            .collect::<Vec<_>>();
        if action_parameters != payload_parameters {
            return Err("standard action payload differs from its checked declaration ABI".into());
        }
        let value = CheckedDefaultAction {
            invocation: fact.invocation.clone(),
            payload: adapter.payload.clone(),
            parameters: fact.parameters.clone(),
            output: fact.output,
            action_output: substitute(adapter.action_output)?,
            reducer: adapter.reducer.clone(),
        };
        if value.reducer.is_none() && value.action_output != value.output {
            return Err("standard action result differs from its helper ABI".into());
        }
        calls.insert(*expr, value);
    }
    Ok((calls, adapters.into_keys().collect()))
}

fn checked_selectors(
    project: &CheckedProject,
    expr: HirExprId,
    selectors: &[etas_std::StdStaticArg],
) -> Result<Vec<etas_types::EffectArgRef>, String> {
    use etas_std::{StdStaticArg, StdType};
    use etas_types::EffectArgRef;
    selectors
        .iter()
        .map(|selector| match selector {
            StdStaticArg::Type(StdType::Var(name)) => {
                let ty = project
                    .types
                    .generic_instantiations
                    .get(&expr)
                    .and_then(|fact| fact.type_bindings.iter().find(|(param, _)| param == name))
                    .map(|(_, ty)| *ty)
                    .ok_or_else(|| {
                        format!(
                            "standard action selector is missing checked generic binding `{name}`"
                        )
                    })?;
                project
                    .type_store
                    .get(ty)
                    .ok_or("standard action selector refers to a missing checked type")?;
                Ok(EffectArgRef::Type(ty))
            }
            StdStaticArg::Wildcard => Ok(EffectArgRef::Wildcard),
            StdStaticArg::String(value) => Ok(EffectArgRef::String(value.clone())),
            StdStaticArg::Int(value) => Ok(EffectArgRef::Int(value.clone())),
            StdStaticArg::Path(path) => Ok(EffectArgRef::Path(path.clone())),
            StdStaticArg::Type(_) => {
                Err("standard action adapter has an invalid selector template".into())
            }
        })
        .collect()
}

impl IntrinsicDispatchTable {
    pub(crate) fn default_action(
        &self,
        call: &CheckedStdIntrinsicCall,
    ) -> Result<Option<&CheckedDefaultAction>, String> {
        self.validate_std_call_origin(call.origin, call.identity)?;
        if !self
            .program
            .default_action_intrinsics
            .contains(&call.identity.intrinsic)
        {
            return Ok(None);
        }
        self.program
            .default_actions
            .get(&call.origin)
            .map(Some)
            .ok_or_else(|| {
                "standard action adapter requires a checked invocation fact for this origin".into()
            })
    }
}
