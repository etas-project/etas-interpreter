use serde_json::{Value, json};

use super::{
    frame::{span_from_snapshot, span_snapshot},
    value::*,
};
use crate::{
    api::codec::{InterpreterCodecError, snapshot},
    orchestration::{ExpressionCallSnapshot, ProgramCallSnapshot, ProgramReturnSnapshot},
};

pub(in crate::api::codec) fn program_return_json(
    frame: &ProgramReturnSnapshot,
    budget: &mut snapshot::EncodingBudget,
) -> Result<Value, InterpreterCodecError> {
    let mut value = json!({
        "kind": "program_return",
        "call": null,
        "call_site": null,
        "handler_floor": frame.handler_floor,
        "span": span_snapshot(frame.span),
        "retry_depth": frame.retry_depth,
        "parent_model_policy": super::model::model_policy_snapshot(&frame.parent_model_policy),
        "parent_agent_context": super::agent_context::request_json(frame.parent_agent_context.as_deref()),
        "continuation": null,
    });
    value["call"] = program_call_json(&frame.call, budget)?;
    value["call_site"] = snapshot::call_site_json_with_budget(&frame.call_site, budget)?;
    value["continuation"] = snapshot::continuation_json_with_budget(&frame.outer, budget)?;
    Ok(value)
}

pub(in crate::api::codec) fn program_return_from_json(
    limits: &etas_host::StorageLimits,
    value: &Value,
    checked: &etas_frontend::CheckedProject,
) -> Result<ProgramReturnSnapshot, String> {
    Ok(ProgramReturnSnapshot {
        call: program_call_from_json(limits, required(value, "call")?)?,
        call_site: call_site_from_json(limits, required(value, "call_site")?)?,
        handler_floor: required_usize(value, "handler_floor")?,
        parent_agent_context: super::agent_context::request_from_json(required(
            value,
            "parent_agent_context",
        )?)?,
        span: span_from_snapshot(required(value, "span")?)?,
        retry_depth: required_usize(value, "retry_depth")?,
        parent_model_policy: super::model::model_policy_from_snapshot(required(
            value,
            "parent_model_policy",
        )?)?,
        outer: super::continuation_from_snapshot(
            limits,
            required(value, "continuation")?,
            checked,
        )?,
    })
}

pub(in crate::api::codec) fn program_call_json(
    call: &ProgramCallSnapshot,
    budget: &mut snapshot::EncodingBudget,
) -> Result<Value, InterpreterCodecError> {
    budget.metadata(2, 0)?;
    Ok(match call {
        ProgramCallSnapshot::Callable(target) => {
            json!({"kind":"callable", "target":snapshot::call_target_json_with_budget(target, budget)?})
        }
        ProgramCallSnapshot::Expression(expression) => {
            json!({"kind":"expression", "invocation":expression_call_json(expression, budget)?})
        }
    })
}

pub(in crate::api::codec) fn program_call_from_json(
    limits: &etas_host::StorageLimits,
    value: &Value,
) -> Result<ProgramCallSnapshot, String> {
    match required_str(value, "kind")? {
        "callable" => Ok(ProgramCallSnapshot::Callable(
            super::call_target_snapshot_from_json(limits, required(value, "target")?)?,
        )),
        "expression" => Ok(ProgramCallSnapshot::Expression(Box::new(
            expression_call_from_json(limits, required(value, "invocation")?)?,
        ))),
        _ => Err("unknown program invocation kind".into()),
    }
}

pub(in crate::api::codec) fn expression_call_json(
    call: &ExpressionCallSnapshot,
    budget: &mut snapshot::EncodingBudget,
) -> Result<Value, InterpreterCodecError> {
    let unit = match &call.unit {
        crate::orchestration::ExpressionUnitSnapshot::Default(reference) => {
            json!({"kind":"default", "reference":reference})
        }
        crate::orchestration::ExpressionUnitSnapshot::Handler {
            caller,
            site,
            handler,
            body,
        } => json!({
            "kind":"handler", "caller":caller, "body":body,
            "site":snapshot::call_site_json_with_budget(site, budget)?,
            "handler":snapshot::value_json_with_budget(handler, budget)?,
        }),
    };
    let mut value = json!({"source":call.source, "unit":unit, "slots":call.slots, "site":null});
    budget.json_metadata(&value)?;
    value["site"] = snapshot::call_site_json_with_budget(&call.site, budget)?;
    Ok(value)
}

pub(in crate::api::codec) fn expression_call_from_json(
    limits: &etas_host::StorageLimits,
    value: &Value,
) -> Result<ExpressionCallSnapshot, String> {
    let slots = required(value, "slots")?
        .as_array()
        .ok_or("expression argument positions must be an array")?;
    if slots.len() > limits.max_nodes {
        return Err("expression argument positions exceed checkpoint limits".into());
    }
    Ok(ExpressionCallSnapshot {
        source: serde_json::from_value(required(value, "source")?.clone())
            .map_err(|error| error.to_string())?,
        unit: {
            let unit = required(value, "unit")?;
            match required_str(unit, "kind")? {
                "default" => crate::orchestration::ExpressionUnitSnapshot::Default(
                    serde_json::from_value(required(unit, "reference")?.clone())
                        .map_err(|error| error.to_string())?,
                ),
                "handler" => crate::orchestration::ExpressionUnitSnapshot::Handler {
                    caller: serde_json::from_value(required(unit, "caller")?.clone())
                        .map_err(|error| error.to_string())?,
                    site: call_site_from_json(limits, required(unit, "site")?)?,
                    handler: crate::orchestration::SnapshotBox::new(
                        crate::api::codec::value::snapshot_from_json_with_limits(
                            limits,
                            required(unit, "handler")?,
                        )
                        .map_err(|error| error.to_string())?,
                    ),
                    body: required(unit, "body")?
                        .as_bool()
                        .ok_or("Handler expression body flag must be boolean")?,
                },
                _ => return Err("unknown expression unit kind".into()),
            }
        },
        site: call_site_from_json(limits, required(value, "site")?)?,
        slots: slots
            .iter()
            .map(|slot| {
                slot.as_u64()
                    .and_then(|slot| u32::try_from(slot).ok())
                    .ok_or_else(|| "invalid expression argument position".to_owned())
            })
            .collect::<Result<_, _>>()?,
    })
}

pub(in crate::api::codec) fn call_site_from_json(
    limits: &etas_host::StorageLimits,
    value: &Value,
) -> Result<crate::orchestration::CallSiteSnapshot, String> {
    let origin = required(value, "origin")?;
    let kind = required_str(origin, "kind")?;
    let fields = match kind {
        "expr" | "pipeline" | "task_callback" => &["kind", "expr"][..],
        "pipeline_stage" => &["kind", "expr", "index"][..],
        "source_task_callback" => &["kind", "item"][..],
        _ => return Err("unknown checked invocation origin kind".into()),
    };
    let object = origin
        .as_object()
        .ok_or("checked invocation origin must be an object")?;
    if object.keys().any(|key| !fields.contains(&key.as_str())) {
        return Err("unknown checked invocation origin field".into());
    }
    let kind = if kind == "source_task_callback" {
        crate::control::CallSiteKind::SourceTaskCallback {
            item: etas_hir::HirItemId(required_u32(origin, "item")?),
        }
    } else {
        let expr = etas_hir::HirExprId(required_u32(origin, "expr")?);
        match kind {
            "expr" => crate::control::CallSiteKind::Expr { expr },
            "task_callback" => crate::control::CallSiteKind::TaskCallback { expr },
            "pipeline" => crate::control::CallSiteKind::Pipeline { expr },
            "pipeline_stage" => crate::control::CallSiteKind::PipelineStage {
                expr,
                index: required_usize(origin, "index")?,
            },
            _ => return Err("unknown checked invocation origin kind".into()),
        }
    };
    Ok(crate::orchestration::CallSiteSnapshot {
        kind,
        task_callback: match required(value, "task_callback")? {
            Value::Null => None,
            call => Some(Box::new(super::call_target_snapshot_from_json(
                limits, call,
            )?)),
        },
        frame: super::frame::locals_from_snapshot(limits, required(value, "frame")?)?,
        composition: match required(value, "composition")? {
            Value::Null => None,
            composition => {
                let path = required(composition, "path")?
                    .as_array()
                    .ok_or("composition path must be an array")?;
                if path.is_empty() || path.len() > limits.max_nodes {
                    return Err("checkpoint composition path exceeds limits or is empty".into());
                }
                let path = path
                    .iter()
                    .map(|index| {
                        index
                            .as_u64()
                            .and_then(|index| usize::try_from(index).ok())
                            .ok_or_else(|| "invalid checkpoint composition index".to_owned())
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                Some(crate::orchestration::ComposedInvocationSnapshot {
                    root: super::call_target_snapshot_from_json(
                        limits,
                        required(composition, "root")?,
                    )?
                    .into(),
                    path,
                })
            }
        },
    })
}
