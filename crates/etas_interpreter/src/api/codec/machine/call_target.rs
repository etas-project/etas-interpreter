use std::sync::Arc;

use etas_hir::{HirExprId, SymbolId};
use serde_json::{Value, json};

use crate::{control::CallTarget, plan::SlotLayoutTable};

use super::{
    continuation::{frame_snapshot, runtime_limit_snapshot, runtime_limits_from_snapshot},
    frame::{frame_from_artifact_snapshot, frame_from_snapshot},
    intrinsic::{std_callable_from_snapshot, std_callable_snapshot},
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
        CallTarget::StdCallable(callable) => json!({
            "kind": "std",
            "callable": std_callable_snapshot(callable),
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
        "std" => Ok(CallTarget::StdCallable(std_callable_from_snapshot(
            required(value, "callable")?,
        )?)),
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
