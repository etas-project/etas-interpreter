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
    let children = target_array(value)?;
    let mut output = Vec::with_capacity(children.len());
    for child in children {
        output.push(decode(limits, child)?);
    }
    Ok(output)
}

enum PendingParent<'a, T: TargetValue> {
    Specialized(&'a Value),
    Limited(&'a Value),
    Composed {
        remaining: &'a [Value],
        values: Vec<T>,
    },
}

fn target_array(value: &Value) -> Result<&[Value], String> {
    value
        .as_array()
        .map(Vec::as_slice)
        .ok_or_else(|| "machine snapshot call targets must be an array".to_owned())
}

fn decode<T: TargetValue>(
    limits: &etas_host::StorageLimits,
    mut current: &Value,
) -> Result<T, String> {
    let mut pending = Vec::new();
    loop {
        let kind = required_str(current, "kind")?;
        let mut value = match kind {
            "specialized" | "limited" => {
                let child = required(current, "target")?;
                pending.push(if kind == "specialized" {
                    PendingParent::Specialized(current)
                } else {
                    PendingParent::Limited(current)
                });
                current = child;
                continue;
            }
            "composed" => {
                let children = target_array(required(current, "targets")?)?;
                if let Some((first, remaining)) = children.split_first() {
                    pending.push(PendingParent::Composed {
                        remaining,
                        values: Vec::with_capacity(children.len()),
                    });
                    current = first;
                    continue;
                }
                T::build(DecodedTarget::Composed(vec![]))
            }
            _ => T::build(decode_leaf::<T>(limits, current, kind)?),
        };
        loop {
            let target = match pending.pop() {
                Some(PendingParent::Specialized(parent)) => {
                    let type_bindings = required(parent, "type_bindings")?
                        .as_array()
                        .ok_or_else(|| {
                            "specialized call target type_bindings must be an array".to_owned()
                        })?
                        .iter()
                        .map(|binding| {
                            Ok((
                                required_str(binding, "name")?.to_owned(),
                                etas_types::TypeId(required_u32(binding, "type")?),
                            ))
                        })
                        .collect::<Result<Vec<_>, String>>()?;
                    DecodedTarget::Specialized {
                        target: value,
                        type_bindings,
                    }
                }
                Some(PendingParent::Limited(parent)) => {
                    let limits = runtime_limits_from_snapshot(required(parent, "limits")?)?;
                    DecodedTarget::Limited {
                        target: value,
                        limits,
                    }
                }
                Some(PendingParent::Composed {
                    remaining,
                    mut values,
                }) => {
                    values.push(value);
                    if let Some((first, remaining)) = remaining.split_first() {
                        pending.push(PendingParent::Composed { remaining, values });
                        current = first;
                        break;
                    }
                    DecodedTarget::Composed(values)
                }
                None => return Ok(value),
            };
            value = T::build(target);
        }
    }
}

fn decode_leaf<T: TargetValue>(
    limits: &etas_host::StorageLimits,
    value: &Value,
    kind: &str,
) -> Result<DecodedTarget<T::Frame, T>, String> {
    Ok(match kind {
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
        other => return Err(format!("unknown machine call target `{other}`")),
    })
}
