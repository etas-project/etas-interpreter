use serde_json::{Value, json};

use super::{
    frame::{span_from_snapshot, span_snapshot},
    value::*,
};
use crate::{
    api::codec::{InterpreterCodecError, snapshot},
    orchestration::ProgramReturnSnapshot,
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
        "continuation": null,
    });
    value["call"] = snapshot::call_target_json_with_budget(&frame.call, budget)?;
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
        call: super::call_target_snapshot_from_json(limits, required(value, "call")?)?,
        call_site: call_site_from_json(limits, required(value, "call_site")?)?,
        handler_floor: required_usize(value, "handler_floor")?,
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

pub(in crate::api::codec) fn call_site_from_json(
    limits: &etas_host::StorageLimits,
    value: &Value,
) -> Result<crate::orchestration::CallSiteSnapshot, String> {
    let origin = required(value, "origin")?;
    let kind = required_str(origin, "kind")?;
    let fields = match kind {
        "expr" | "pipeline" => &["kind", "expr"][..],
        "pipeline_stage" => &["kind", "expr", "index"][..],
        _ => return Err("unknown checked invocation origin kind".into()),
    };
    let object = origin
        .as_object()
        .ok_or("checked invocation origin must be an object")?;
    if object.keys().any(|key| !fields.contains(&key.as_str())) {
        return Err("unknown checked invocation origin field".into());
    }
    let expr = etas_hir::HirExprId(required_u32(origin, "expr")?);
    let kind = match kind {
        "expr" => crate::control::CallSiteKind::Expr { expr },
        "pipeline" => crate::control::CallSiteKind::Pipeline { expr },
        "pipeline_stage" => crate::control::CallSiteKind::PipelineStage {
            expr,
            index: required_usize(origin, "index")?,
        },
        _ => return Err("unknown checked invocation origin kind".into()),
    };
    Ok(crate::orchestration::CallSiteSnapshot {
        kind,
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
