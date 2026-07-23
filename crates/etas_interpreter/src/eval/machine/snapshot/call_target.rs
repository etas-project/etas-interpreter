use crate::control::CallTarget;
use crate::orchestration::CallTargetSnapshot;

pub(super) fn capture_call_target(target: &CallTarget) -> Result<CallTargetSnapshot, String> {
    Ok(match target {
        CallTarget::FlowItem(item) => CallTargetSnapshot::FlowItem(*item),
        CallTarget::AgentItem(item) => CallTargetSnapshot::AgentItem(*item),
        CallTarget::ToolItem(item) => CallTargetSnapshot::ToolItem(*item),
        CallTarget::SpecImplMethod(symbol) => CallTargetSnapshot::SpecImplMethod(*symbol),
        CallTarget::Lambda { expr, captured } => CallTargetSnapshot::Lambda {
            expr: *expr,
            captured: super::frame::capture_frame(captured)?,
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
        CallTarget::Limited { target, limits } => CallTargetSnapshot::Limited {
            target: Box::new(capture_call_target(target)?),
            limits: limits.clone(),
        },
        CallTarget::Composed(targets) => CallTargetSnapshot::Composed(
            targets
                .iter()
                .map(capture_call_target)
                .collect::<Result<Vec<_>, _>>()?,
        ),
    })
}

pub(super) fn restore_call_target(snapshot: CallTargetSnapshot) -> Result<CallTarget, String> {
    Ok(match snapshot {
        CallTargetSnapshot::FlowItem(item) => CallTarget::FlowItem(item),
        CallTargetSnapshot::AgentItem(item) => CallTarget::AgentItem(item),
        CallTargetSnapshot::ToolItem(item) => CallTarget::ToolItem(item),
        CallTargetSnapshot::SpecImplMethod(symbol) => CallTarget::SpecImplMethod(symbol),
        CallTargetSnapshot::Lambda { expr, captured } => CallTarget::Lambda {
            expr,
            captured: super::frame::restore_frame(captured)?,
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
        CallTargetSnapshot::Limited { target, limits } => CallTarget::Limited {
            target: Box::new(restore_call_target(*target)?),
            limits,
        },
        CallTargetSnapshot::Composed(targets) => CallTarget::Composed(
            targets
                .into_iter()
                .map(restore_call_target)
                .collect::<Result<Vec<_>, _>>()?,
        ),
    })
}
