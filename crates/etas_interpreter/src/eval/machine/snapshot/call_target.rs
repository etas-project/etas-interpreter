use super::RestoreContext;
use crate::control::CallTarget;
use crate::orchestration::CallTargetSnapshot;

mod capture;
#[cfg(test)]
pub(super) use capture::capture_call_target;
pub(super) use capture::capture_call_target_with;
mod restore;
pub(super) use restore::restore_call_target;

#[cfg(test)]
mod tests;

fn capture_leaf(
    target: &CallTarget,
    context: &mut super::capture_context::CaptureContext,
) -> Result<CallTargetSnapshot, String> {
    Ok(match target {
        CallTarget::FlowItem(item) => CallTargetSnapshot::FlowItem(*item),
        CallTarget::AgentItem(item) => CallTargetSnapshot::AgentItem(*item),
        CallTarget::ToolItem(item) => CallTargetSnapshot::ToolItem(*item),
        CallTarget::SpecImplMethod(symbol) => CallTargetSnapshot::SpecImplMethod(*symbol),
        CallTarget::Lambda { expr, captured } => CallTargetSnapshot::Lambda {
            expr: *expr,
            captured: context.frame(captured)?,
        },
        CallTarget::EnumVariant(symbol) => CallTargetSnapshot::EnumVariant(*symbol),
        CallTarget::NominalConstructor(ty) => CallTargetSnapshot::NominalConstructor(*ty),
        CallTarget::PureIntrinsic(call) => CallTargetSnapshot::PureIntrinsic {
            intrinsic: call.intrinsic,
            parameter_types: call.parameter_types.clone(),
            result_type: call.result_type,
        },
        CallTarget::StdIntrinsic(call) => CallTargetSnapshot::StdIntrinsic {
            intrinsic: call.identity.intrinsic,
            dispatch: call.identity.dispatch,
            parameter_types: call.parameter_types.clone(),
            result_type: call.result_type,
        },
        CallTarget::Specialized { .. } | CallTarget::Limited { .. } | CallTarget::Composed(_) => {
            return Err("compound call target must use its capture builder".into());
        }
    })
}

fn restore_leaf(
    snapshot: CallTargetSnapshot,
    context: &mut RestoreContext,
) -> Result<CallTarget, String> {
    Ok(match snapshot {
        CallTargetSnapshot::FlowItem(item) => CallTarget::FlowItem(item),
        CallTargetSnapshot::AgentItem(item) => CallTarget::AgentItem(item),
        CallTargetSnapshot::ToolItem(item) => CallTarget::ToolItem(item),
        CallTargetSnapshot::SpecImplMethod(symbol) => CallTarget::SpecImplMethod(symbol),
        CallTargetSnapshot::Lambda { expr, captured } => CallTarget::Lambda {
            expr,
            captured: super::frame::restore_frame(captured, context)?,
        },
        CallTargetSnapshot::EnumVariant(symbol) => CallTarget::EnumVariant(symbol),
        CallTargetSnapshot::NominalConstructor(ty) => CallTarget::NominalConstructor(ty),
        CallTargetSnapshot::PureIntrinsic {
            intrinsic,
            parameter_types,
            result_type,
        } => CallTarget::PureIntrinsic(crate::intrinsic::dispatch::CheckedPureIntrinsicCall {
            intrinsic,
            parameter_types,
            result_type,
        }),
        CallTargetSnapshot::StdIntrinsic {
            intrinsic,
            dispatch,
            parameter_types,
            result_type,
        } => CallTarget::StdIntrinsic(crate::intrinsic::dispatch::CheckedStdIntrinsicCall {
            identity: crate::intrinsic::dispatch::StdIntrinsicIdentity {
                intrinsic,
                dispatch,
            },
            parameter_types,
            result_type,
        }),
        CallTargetSnapshot::Specialized { .. }
        | CallTargetSnapshot::Limited { .. }
        | CallTargetSnapshot::Composed(_) => {
            return Err("compound call target must use its restore builder".into());
        }
    })
}
