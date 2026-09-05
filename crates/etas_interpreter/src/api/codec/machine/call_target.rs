use std::sync::Arc;

use etas_hir::{HirExprId, SymbolId};
use serde_json::{Value, json};

use crate::{control::CallTarget, plan::SlotLayoutTable};

use super::{
    continuation::{frame_snapshot, runtime_limit_snapshot, runtime_limits_from_snapshot},
    frame::{frame_from_artifact_snapshot, frame_from_snapshot},
    intrinsic::{intrinsic_dispatch_from_name, intrinsic_dispatch_name},
    value::{required, required_str, required_u32},
};

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

pub(super) fn call_target_from_snapshot(
    value: &Value,
    slots: Arc<SlotLayoutTable>,
) -> Result<CallTarget, String> {
    call_target_from_snapshot_with_layout(value, Some(slots))
}

pub(crate) fn call_target_from_artifact_snapshot(value: &Value) -> Result<CallTarget, String> {
    call_target_from_snapshot_with_layout(value, None)
}

fn call_target_from_snapshot_with_layout(
    value: &Value,
    slots: Option<Arc<SlotLayoutTable>>,
) -> Result<CallTarget, String> {
    match required_str(value, "kind")? {
        "flow" => Ok(CallTarget::FlowItem(etas_hir::HirItemId(required_u32(
            value, "item",
        )?))),
        "agent" => Ok(CallTarget::AgentItem(etas_hir::HirItemId(required_u32(
            value, "item",
        )?))),
        "tool" => Ok(CallTarget::ToolItem(etas_hir::HirItemId(required_u32(
            value, "item",
        )?))),
        "spec_impl_method" => Ok(CallTarget::SpecImplMethod(SymbolId(required_u32(
            value, "symbol",
        )?))),
        "lambda" => Ok(CallTarget::Lambda {
            expr: HirExprId(required_u32(value, "expr")?),
            captured: match &slots {
                Some(slots) => frame_from_snapshot(required(value, "captured")?, slots.clone())?,
                None => frame_from_artifact_snapshot(required(value, "captured")?)?,
            },
        }),
        "enum_variant" => Ok(CallTarget::EnumVariant(SymbolId(required_u32(
            value, "symbol",
        )?))),
        "nominal_constructor" => Ok(CallTarget::NominalConstructor(etas_types::TypeId(
            required_u32(value, "ty")?,
        ))),
        "pure_intrinsic" => Ok(CallTarget::PureIntrinsic(
            crate::intrinsic::dispatch::CheckedPureIntrinsicCall {
                intrinsic: etas_std::StdIntrinsicId(required_u32(value, "intrinsic")?),
                parameter_types: required(value, "parameter_types")?
                    .as_array()
                    .ok_or_else(|| {
                        "machine pure intrinsic parameter_types must be an array".to_owned()
                    })?
                    .iter()
                    .map(|value| {
                        value
                            .as_u64()
                            .and_then(|value| u32::try_from(value).ok())
                            .map(etas_types::TypeId)
                            .ok_or_else(|| {
                                "machine pure intrinsic parameter type must be a u32".to_owned()
                            })
                    })
                    .collect::<Result<Vec<_>, _>>()?,
                result_type: etas_types::TypeId(required_u32(value, "result_type")?),
            },
        )),
        "std_intrinsic" => Ok(CallTarget::StdIntrinsic(
            crate::intrinsic::dispatch::CheckedStdIntrinsicCall {
                identity: crate::intrinsic::dispatch::StdIntrinsicIdentity {
                    intrinsic: etas_std::StdIntrinsicId(required_u32(value, "intrinsic")?),
                    dispatch: intrinsic_dispatch_from_name(required_str(value, "dispatch")?)?,
                },
                parameter_types: required(value, "parameter_types")?
                    .as_array()
                    .ok_or_else(|| {
                        "machine std intrinsic parameter_types must be an array".to_owned()
                    })?
                    .iter()
                    .map(|value| {
                        value
                            .as_u64()
                            .and_then(|value| u32::try_from(value).ok())
                            .map(etas_types::TypeId)
                            .ok_or_else(|| {
                                "machine std intrinsic parameter type must be a u32".to_owned()
                            })
                    })
                    .collect::<Result<Vec<_>, _>>()?,
                result_type: etas_types::TypeId(required_u32(value, "result_type")?),
            },
        )),
        "specialized" => Ok(CallTarget::Specialized {
            target: Box::new(call_target_from_snapshot_with_layout(
                required(value, "target")?,
                slots,
            )?),
            type_bindings: required(value, "type_bindings")?
                .as_array()
                .ok_or_else(|| "specialized call target type_bindings must be an array".to_owned())?
                .iter()
                .map(|binding| {
                    Ok((
                        required_str(binding, "name")?.to_owned(),
                        etas_types::TypeId(required_u32(binding, "type")?),
                    ))
                })
                .collect::<Result<Vec<_>, String>>()?,
        }),
        "limited" => Ok(CallTarget::Limited {
            target: Box::new(call_target_from_snapshot_with_layout(
                required(value, "target")?,
                slots,
            )?),
            limits: runtime_limits_from_snapshot(required(value, "limits")?)?,
        }),
        "composed" => Ok(CallTarget::Composed(
            call_targets_from_snapshot_with_layout(required(value, "targets")?, slots)?,
        )),
        other => Err(format!("unknown machine call target `{other}`")),
    }
}

pub(super) fn call_targets_from_snapshot(
    value: &Value,
    slots: Arc<SlotLayoutTable>,
) -> Result<Vec<CallTarget>, String> {
    value
        .as_array()
        .ok_or_else(|| "machine snapshot call targets must be an array".to_owned())?
        .iter()
        .map(|value| call_target_from_snapshot(value, slots.clone()))
        .collect()
}

fn call_targets_from_snapshot_with_layout(
    value: &Value,
    slots: Option<Arc<SlotLayoutTable>>,
) -> Result<Vec<CallTarget>, String> {
    value
        .as_array()
        .ok_or_else(|| "machine snapshot call targets must be an array".to_owned())?
        .iter()
        .map(|value| call_target_from_snapshot_with_layout(value, slots.clone()))
        .collect()
}
