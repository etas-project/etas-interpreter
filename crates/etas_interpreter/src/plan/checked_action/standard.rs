use etas_effects::{ActionRef, Effect};
use etas_types::{PrimitiveType, Type};

use super::CheckedActionAbi;
use crate::{
    intrinsic::dispatch::{CheckedStdIntrinsicCall, ConsoleCallable, StdCallable},
    value::CheckedExecutionProgram,
};

pub(crate) fn standard_action(
    execution: &CheckedExecutionProgram,
    call: &CheckedStdIntrinsicCall,
) -> Result<CheckedActionAbi, String> {
    let checked = &execution.checked;
    execution.plan.dispatch.validate_std_call_template(
        call.origin,
        call.identity,
        &call.substitutions,
        &call.parameter_types,
        call.result_type,
        &checked.type_store,
    )?;
    let callable = execution
        .plan
        .dispatch
        .resolve_std_callable(call.identity)?;
    let abi = match callable {
        StdCallable::Console(console) => {
            use etas_effects::{
                CONSOLE_STDERR_WRITE_ACTION, CONSOLE_STDIN_READ_ALL_ACTION,
                CONSOLE_STDIN_READ_LINE_ACTION, CONSOLE_STDOUT_WRITE_ACTION, CONSOLE_TAG,
            };
            let action = ActionRef {
                tag: CONSOLE_TAG,
                action: match console {
                    ConsoleCallable::Print | ConsoleCallable::PrintLn => {
                        CONSOLE_STDOUT_WRITE_ACTION
                    }
                    ConsoleCallable::EPrintLn => CONSOLE_STDERR_WRITE_ACTION,
                    ConsoleCallable::ReadAll => CONSOLE_STDIN_READ_ALL_ACTION,
                    ConsoleCallable::ReadLine => CONSOLE_STDIN_READ_LINE_ACTION,
                },
            };
            let signature = checked
                .effect_registry
                .action_signature(&action)
                .ok_or("default Console action has no checked registry signature")?;
            if !signature.effect_args.is_empty() {
                return Err("default Console action requires a closed checked invocation".into());
            }
            CheckedActionAbi {
                effect: Effect::Action(action),
                parameters: call.parameter_types.clone(),
                output: call.result_type,
                returns_never: signature.returns_never,
            }
        }
        StdCallable::ForwardAction => {
            let target = execution.plan.dispatch.forwarded_action(call.identity)?;
            CheckedActionAbi {
                effect: Effect::Action(target.action.clone()),
                parameters: call.parameter_types.clone(),
                output: call.result_type,
                returns_never: matches!(
                    checked.type_store.get(call.result_type),
                    Some(Type::Primitive(PrimitiveType::Never))
                ),
            }
        }
        _ => {
            let adapter = execution
                .plan
                .dispatch
                .default_action(call)?
                .ok_or("standard callable has no checked action adapter")?;
            let instantiate = |ty| call.instantiate_type(&checked.type_store, ty);
            let parameters = adapter
                .parameters
                .iter()
                .map(|ty| instantiate(*ty))
                .collect::<Result<Vec<_>, _>>()?;
            if parameters != call.parameter_types
                || instantiate(adapter.output)? != call.result_type
            {
                return Err("standard action ABI does not match its checked substitutions".into());
            }
            let mut effect = adapter.invocation.clone();
            if let Effect::AppliedAction(instance) = &mut effect {
                for arg in &mut instance.args {
                    if let etas_types::EffectArgRef::Type(ty) = arg {
                        *ty = instantiate(*ty)?;
                    }
                }
            }
            let action = match &effect {
                Effect::Action(action) => action,
                Effect::AppliedAction(instance) => &instance.action,
                _ => return Err("native action has no closed checked action instance".into()),
            };
            let signature = checked
                .effect_registry
                .action_signature(action)
                .ok_or("native action has no checked declaration")?;
            CheckedActionAbi {
                effect,
                parameters: adapter
                    .payload
                    .iter()
                    .map(|index| {
                        call.parameter_types.get(*index).copied().ok_or_else(|| {
                            "native action payload projection is outside its checked ABI".to_owned()
                        })
                    })
                    .collect::<Result<_, _>>()?,
                output: instantiate(adapter.action_output)?,
                returns_never: signature.returns_never,
            }
        }
    };
    abi.require_mediation(execution)?;
    Ok(abi)
}
