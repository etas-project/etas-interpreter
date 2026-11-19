use super::CheckedActionAbi;
use crate::{
    control::{AgentOperationKind, CheckedAgentOperation},
    value::CheckedExecutionProgram,
};

pub(crate) fn controller_step(
    execution: &CheckedExecutionProgram,
    inference: etas_hir::HirExprId,
) -> Result<CheckedActionAbi, String> {
    let checked = &execution.checked;
    let inference_fact = checked
        .types
        .agent_inferences
        .get(&inference)
        .ok_or("controller origin is not a checked inference")?;
    let contract = checked
        .types
        .agent_controllers
        .get(&inference)
        .ok_or("controller origin has no checked operation contracts")?;
    if checked.type_store.get(contract.context)
        != Some(&etas_types::Type::AgentContext(
            inference_fact.method.into(),
        ))
    {
        return Err("controller context differs from its checked inference".into());
    }
    let operation =
        CheckedAgentOperation::controller(checked, inference, AgentOperationKind::Step)?;
    let signature = checked
        .effect_registry
        .compiler_action(
            &checked.std_registry,
            etas_std::CompilerActionRole::AgentStep,
        )
        .ok_or("controller step action is missing")?;
    let action = etas_effects::ActionRef {
        tag: signature.owner,
        action: signature.id,
    };
    let args = vec![
        etas_types::EffectArgRef::Type(contract.context),
        etas_types::EffectArgRef::Type(inference_fact.output),
    ];
    let request = contract
        .step
        .signature
        .requested_actions
        .as_ref()
        .filter(|row| row.tail.is_none())
        .and_then(|row| match row.effects.as_slice() {
            [request] => Some(request),
            _ => None,
        })
        .ok_or("controller step must request one checked action")?;
    if checked
        .effect_registry
        .standard_action_by_name(&request.name)
        .as_ref()
        != Some(&action)
        || request.args != args
        || operation.parameter_types.len() != 2
    {
        return Err("controller step differs from its checked action contract".into());
    }
    let abi = CheckedActionAbi {
        effect: etas_effects::Effect::AppliedAction(etas_effects::ActionInstanceRef {
            action,
            args,
        }),
        parameters: operation.parameter_types,
        output: operation.result_type,
        returns_never: signature.returns_never,
    };
    abi.require_mediation(execution)?;
    Ok(abi)
}
