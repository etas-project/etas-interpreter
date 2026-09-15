use super::{
    continuation::{frame_snapshot, runtime_limit_snapshot},
    intrinsic::intrinsic_dispatch_name,
};
use crate::control::CallTarget;
use serde_json::{Value, json};
mod decode;
mod target;
#[cfg(test)]
mod tests;
pub(super) use decode::call_target_snapshots_from_json;
pub(crate) use decode::{call_target_from_artifact_snapshot, call_target_snapshot_from_json};

pub(crate) fn call_target_snapshot(target: &CallTarget) -> Value {
    match target {
        CallTarget::FlowItem(item) => json!({ "kind": "flow", "item": item.0 }),
        CallTarget::AgentItem(item) => json!({ "kind": "agent", "item": item.0 }),
        CallTarget::ToolItem(item) => json!({ "kind": "tool", "item": item.0 }),
        CallTarget::SpecImplMethod(symbol) => {
            json!({ "kind": "spec_impl_method", "symbol": symbol.0 })
        }
        CallTarget::Lambda { expr, captured } => json!({
            "kind": "lambda",
            "expr": expr.0,
            "captured": frame_snapshot(captured),
        }),
        CallTarget::EnumVariant(symbol) => {
            json!({ "kind": "enum_variant", "symbol": symbol.0 })
        }
        CallTarget::NominalConstructor(ty) => {
            json!({ "kind": "nominal_constructor", "ty": ty.0 })
        }
        CallTarget::PureIntrinsic(call) => json!({
            "kind": "pure_intrinsic",
            "intrinsic": call.intrinsic.0,
            "parameter_types": call.parameter_types.iter().map(|ty| ty.0).collect::<Vec<_>>(),
            "result_type": call.result_type.0,
        }),
        CallTarget::StdIntrinsic(call) => json!({
            "kind": "std_intrinsic",
            "intrinsic": call.identity.intrinsic.0,
            "dispatch": intrinsic_dispatch_name(call.identity.dispatch),
            "parameter_types": call.parameter_types.iter().map(|ty| ty.0).collect::<Vec<_>>(),
            "result_type": call.result_type.0,
        }),
        CallTarget::Specialized {
            target,
            type_bindings,
        } => json!({
            "kind": "specialized",
            "target": call_target_snapshot(target),
            "type_bindings": type_bindings.iter().map(|(name, ty)| {
                json!({ "name": name, "type": ty.0 })
            }).collect::<Vec<_>>(),
        }),
        CallTarget::Limited { target, limits } => json!({
            "kind": "limited",
            "target": call_target_snapshot(target),
            "limits": limits.iter().map(runtime_limit_snapshot).collect::<Vec<_>>(),
        }),
        CallTarget::Composed(targets) => json!({
            "kind": "composed",
            "targets": targets.iter().map(call_target_snapshot).collect::<Vec<_>>(),
        }),
    }
}
