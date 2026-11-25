use super::CheckedActionAbi;
use crate::{control::managed_agent::ManagedAgentOrigin, value::CheckedExecutionProgram};
use etas_types::{EffectArgRef, ItemSignature};

pub(crate) fn managed_agent(
    execution: &CheckedExecutionProgram,
    origin: &ManagedAgentOrigin,
) -> Result<CheckedActionAbi, String> {
    let checked = &execution.checked;
    let (selector, output) = match origin {
        ManagedAgentOrigin::Create { expr, bindings } => {
            if !matches!(
                checked.hir.exprs.get(*expr),
                Some(etas_hir::HirExpr::Record(_))
            ) {
                return Err("managed create origin is not a checked construction".into());
            }
            let ty = checked
                .types
                .expression_producer_type(*expr)
                .ok_or("managed create has no checked instance type")?;
            let ty = bindings
                .substitution(&checked.type_store)
                .substitute(ty)
                .map_err(|error| error.to_string())?;
            if !checked.types.is_active_agent(&checked.type_store, ty) {
                return Err("managed create does not construct an active type".into());
            }
            (ty, ty)
        }
        ManagedAgentOrigin::Call {
            item,
            receiver,
            bindings,
        } => {
            let Some(etas_hir::HirItem::Flow(flow)) = checked.hir.items.get(*item) else {
                return Err("managed call origin is not a checked method".into());
            };
            let method = checked
                .types
                .agent_methods
                .get(&flow.symbol)
                .ok_or("managed call has no Agent method identity")?;
            let Some(ItemSignature::Flow(signature)) = checked.types.item_signatures.get(item)
            else {
                return Err("managed call has no checked method signature".into());
            };
            let mut substitution = bindings.substitution(&checked.type_store);
            let receiver_type = signature
                .params
                .first()
                .ok_or("managed method has no checked receiver parameter")?;
            let component = substitution
                .substitute(*receiver_type)
                .map_err(|error| error.to_string())?;
            if method.body != *item
                || component != *receiver
                || !checked
                    .types
                    .is_active_agent(&checked.type_store, component)
            {
                return Err("managed call receiver differs from its checked method".into());
            }
            (
                method.context_type,
                substitution
                    .substitute(signature.output)
                    .map_err(|error| error.to_string())?,
            )
        }
    };
    let action = checked
        .effect_registry
        .action_by_name(&format!("Agentic.{}", origin.name()))
        .ok_or("managed Agent boundary has no checked carried action")?;
    let abi = CheckedActionAbi {
        effect: etas_effects::Effect::AppliedAction(etas_effects::ActionInstanceRef {
            action,
            args: vec![EffectArgRef::Type(selector)],
        }),
        parameters: Vec::new(),
        output,
        returns_never: false,
    };
    abi.require_mediation(execution)?;
    Ok(abi)
}
