use super::super::{
    continuation::runtime_limits_from_snapshot,
    intrinsic::intrinsic_dispatch_from_name,
    value::{required, required_str, required_u32},
};
use super::target::{DecodedTarget, TargetValue};
use crate::{control::CallTarget, orchestration::CallTargetSnapshot};
use etas_hir::{HirExprId, SymbolId};
use serde_json::Value;

pub(crate) fn call_target_from_artifact_snapshot(
    limits: &etas_host::StorageLimits,
    value: &Value,
) -> Result<CallTarget, String> {
    decode(limits, value)
}

pub(crate) fn call_target_snapshot_from_json(
    limits: &etas_host::StorageLimits,
    value: &Value,
) -> Result<CallTargetSnapshot, String> {
    decode(limits, value)
}

pub(in crate::api::codec::machine) fn call_target_snapshots_from_json(
    limits: &etas_host::StorageLimits,
    value: &Value,
) -> Result<Vec<CallTargetSnapshot>, String> {
    decode_many(limits, value)
}

fn decode_many<T: TargetValue>(
    limits: &etas_host::StorageLimits,
    value: &Value,
) -> Result<Vec<T>, String> {
    value
        .as_array()
        .ok_or_else(|| "machine snapshot call targets must be an array".to_owned())?
        .iter()
        .map(|value| decode(limits, value))
        .collect()
}

fn decode<T: TargetValue>(limits: &etas_host::StorageLimits, value: &Value) -> Result<T, String> {
    Ok(T::build(match required_str(value, "kind")? {
        "flow" => DecodedTarget::FlowItem(etas_hir::HirItemId(required_u32(value, "item")?)),
        "agent" => DecodedTarget::AgentItem(etas_hir::HirItemId(required_u32(value, "item")?)),
        "tool" => DecodedTarget::ToolItem(etas_hir::HirItemId(required_u32(value, "item")?)),
        "spec_impl_method" => {
            DecodedTarget::SpecImplMethod(SymbolId(required_u32(value, "symbol")?))
        }
        "lambda" => DecodedTarget::Lambda {
            expr: HirExprId(required_u32(value, "expr")?),
            captured: T::frame(limits, required(value, "captured")?)?,
        },
        "enum_variant" => DecodedTarget::EnumVariant(SymbolId(required_u32(value, "symbol")?)),
        "nominal_constructor" => {
            DecodedTarget::NominalConstructor(etas_types::TypeId(required_u32(value, "ty")?))
        }
        "pure_intrinsic" => DecodedTarget::PureIntrinsic {
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
        "std_intrinsic" => DecodedTarget::StdIntrinsic {
            intrinsic: etas_std::StdIntrinsicId(required_u32(value, "intrinsic")?),
            dispatch: intrinsic_dispatch_from_name(required_str(value, "dispatch")?)?,
            parameter_types: required(value, "parameter_types")?
                .as_array()
                .ok_or_else(|| "machine std intrinsic parameter_types must be an array".to_owned())?
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
        "specialized" => DecodedTarget::Specialized {
            target: Box::new(decode(limits, required(value, "target")?)?),
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
        },
        "limited" => DecodedTarget::Limited {
            target: Box::new(decode(limits, required(value, "target")?)?),
            limits: runtime_limits_from_snapshot(required(value, "limits")?)?,
        },
        "composed" => DecodedTarget::Composed(decode_many(limits, required(value, "targets")?)?),
        other => return Err(format!("unknown machine call target `{other}`")),
    }))
}
