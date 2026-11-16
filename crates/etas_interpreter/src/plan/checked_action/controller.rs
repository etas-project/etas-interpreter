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
    let descriptor = checked
        .std_registry
        .intrinsic(etas_std::StdIntrinsicId(
            etas_std::intrinsic::runtime::AGENT_STEP,
        ))
        .ok_or("controller step intrinsic is missing")?;
    let etas_std::LoweringHint::DefaultAction { stable_id, payload } = &descriptor.lowering else {
        return Err("controller step has no canonical action adapter".into());
    };
    let signature = checked
        .effect_registry
        .standard_action(etas_effects::EffectActionId(*stable_id))
        .ok_or("controller step action is missing")?;
    let Some(row) = &contract.step.signature.requested_actions else {
        return Err("controller step has no checked request contract".into());
    };
    let [requested] = row.effects.as_slice() else {
        return Err("controller step must request one checked action".into());
    };
    if row.tail.is_some() || payload != &[0, 1] || operation.parameter_types.len() != 2 {
        return Err("controller step has an inconsistent payload contract".into());
    }
    let abi = CheckedActionAbi {
        effect: etas_effects::Effect::AppliedAction(etas_effects::ActionInstanceRef {
            action: etas_effects::ActionRef {
                tag: signature.owner,
                action: signature.id,
            },
            args: requested.args.clone(),
        }),
        parameters: operation.parameter_types,
        output: operation.result_type,
        returns_never: signature.returns_never,
    };
    abi.require_mediation(execution)?;
    Ok(abi)
}
