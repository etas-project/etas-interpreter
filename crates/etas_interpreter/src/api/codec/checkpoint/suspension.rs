use super::*;
use crate::orchestration::{
    ActionInvocationSnapshot, ActionOriginSnapshot, SuspendedProgramSnapshot,
};

pub(super) fn encode(
    node: &SuspendedProgramSnapshot,
    budget: &mut snapshot::EncodingBudget,
) -> Result<Value, InterpreterCodecError> {
    let metadata_bytes = etas_core::memory::MemoryFootprint::of(&node.parent).estimated_bytes();
    budget.metadata(
        1 + node.frames.len() + node.handlers.handlers.len() + node.retries.attempts.len(),
        usize::try_from(metadata_bytes).map_err(|_| {
            InterpreterCodecError::new("suspended program metadata exceeds encoding limits")
        })?,
    )?;
    let mut value = json!({
        "parent": node.parent, "parent_scoped": node.parent_scoped, "parent_handler_floor": node.parent_handler_floor,
        "boundary": null, "frames": [], "continuation": null,
        "handlers": node.handlers.handlers.iter().map(|handler| json!({
            "id": handler.id.0, "handled_actions": handler.handled_actions, "handlers": handler.handlers.iter().map(handler_arm_json).collect::<Vec<_>>(), "span": span_json(handler.span),
        })).collect::<Vec<_>>(),
        "retries": node.retries.attempts.iter().map(|retry| json!({"id": retry.id.0, "ordinal": retry.ordinal})).collect::<Vec<_>>(),
        "model_policy": machine::model_policy_snapshot(&node.model_policy), "span": span_json(node.span),
        "action": null
    });
    value["boundary"] = machine::program_return_json(&node.boundary, budget)?;
    for frame in &node.frames {
        let mut frame = machine_frame_json(frame, budget)?;
        frame["program_owner"] = json!(0);
        value["frames"]
            .as_array_mut()
            .ok_or_else(|| InterpreterCodecError::new("suspended frame array is missing"))?
            .push(frame);
    }
    value["continuation"] = snapshot::continuation_json_with_budget(&node.continuation, budget)?;
    value["action"] = match &node.action {
        ActionInvocationSnapshot::FromSuspension { id } => json!({"kind":"forwarded", "id":id}),
        ActionInvocationSnapshot::Native { native, origin } => {
            budget.metadata(
                1,
                usize::try_from(etas_core::memory::MemoryFootprint::of(native).estimated_bytes())
                    .map_err(|_| {
                    InterpreterCodecError::new("action metadata exceeds encoding limits")
                })?,
            )?;
            let origin = match origin {
                ActionOriginSnapshot::Source { expr, bindings } => {
                    budget.metadata(
                        bindings.len(),
                        bindings.iter().map(|(name, _)| name.len()).sum(),
                    )?;
                    json!({"kind":"source", "expr": expr.0, "bindings": bindings})
                }
                ActionOriginSnapshot::Standard { call } => {
                    let mut origin = json!({"kind":"standard", "call": null});
                    origin["call"] = snapshot::call_target_json_with_budget(call, budget)?;
                    origin
                }
            };
            json!({"kind":"native", "native":native, "origin":origin})
        }
    };
    Ok(value)
}

pub(super) fn decode(
    limits: &etas_host::StorageLimits,
    value: &Value,
    revisions: Option<&crate::api::code::PreparedRevisionSet>,
) -> Result<SuspendedProgramSnapshot, InterpreterCodecError> {
    let revisions = revisions.ok_or_else(|| {
        InterpreterCodecError::new("suspended program requires freshly prepared revisions")
    })?;
    let parent =
        serde_json::from_value(required_obj(value, "parent")?.clone()).map_err(|error| {
            InterpreterCodecError::new(format!("invalid suspended parent identity: {error}"))
        })?;
    let execution = revisions
        .execution_snapshot(&parent)
        .map_err(InterpreterCodecError::new)?;
    let boundary = machine::program_return_from_json(
        limits,
        required_obj(value, "boundary")?,
        &execution.checked,
    )
    .map_err(InterpreterCodecError::new)?;
    let native = crate::eval::machine::snapshot::program::resolve_destination(
        &boundary,
        &execution.checked,
        Some(revisions),
        limits,
    )
    .map_err(InterpreterCodecError::new)?;
    if required_array(value, "frames")?
        .iter()
        .any(|frame| frame.get("kind").and_then(Value::as_str) == Some("program_return"))
    {
        return Err(InterpreterCodecError::new(
            "suspended segment contains another program boundary",
        ));
    }
    let frames = machine_frames_from_json(limits, value, &native.checked, Some(revisions))?;
    let action = decode_action(limits, required_obj(value, "action")?)?;
    Ok(SuspendedProgramSnapshot {
        parent,
        parent_scoped: required_obj(value, "parent_scoped")?
            .as_bool()
            .ok_or_else(|| InterpreterCodecError::new("suspended parent_scoped must be boolean"))?,
        parent_handler_floor: required_usize(value, "parent_handler_floor")?,
        boundary: Box::new(boundary),
        frames: frames.frames,
        continuation: machine::continuation_from_snapshot(
            limits,
            required_obj(value, "continuation")?,
            &native.checked,
        )
        .map_err(InterpreterCodecError::new)?,
        handlers: HandlerSnapshot {
            handlers: required_array(value, "handlers")?
                .iter()
                .map(active_handler_from_json)
                .collect::<Result<_, _>>()?,
        },
        retries: RetrySnapshot {
            attempts: required_array(value, "retries")?
                .iter()
                .map(|retry| {
                    Ok(RetryAttemptRecord {
                        id: RetryAttemptId(required_u32(retry, "id")?),
                        ordinal: required_u32(retry, "ordinal")?,
                    })
                })
                .collect::<Result<_, InterpreterCodecError>>()?,
        },
        model_policy: machine::model_policy_from_snapshot(required_obj(value, "model_policy")?)
            .map_err(InterpreterCodecError::new)?,
        action,
        span: span_from_json(required_obj(value, "span")?)?,
    })
}

fn decode_action(
    limits: &etas_host::StorageLimits,
    action: &Value,
) -> Result<ActionInvocationSnapshot, InterpreterCodecError> {
    match required_str(action, "kind")? {
        "forwarded" => {
            return Ok(ActionInvocationSnapshot::FromSuspension {
                id: required_u32(action, "id")?,
            });
        }
        "native" => {}
        _ => {
            return Err(InterpreterCodecError::new(
                "unknown suspended action evidence kind",
            ));
        }
    }
    let origin = required_obj(action, "origin")?;
    let origin = match required_str(origin, "kind")? {
        "source" => {
            if required_array(origin, "bindings")?.len() > limits.max_nodes {
                return Err(InterpreterCodecError::new(
                    "action binding table exceeds limits",
                ));
            }
            ActionOriginSnapshot::Source {
                expr: etas_hir::HirExprId(required_u32(origin, "expr")?),
                bindings: serde_json::from_value(required_obj(origin, "bindings")?.clone())
                    .map_err(|error| {
                        InterpreterCodecError::new(format!("invalid action type bindings: {error}"))
                    })?,
            }
        }
        "standard" => ActionOriginSnapshot::Standard {
            call: machine::call_target_snapshot_from_json(limits, required_obj(origin, "call")?)
                .map_err(InterpreterCodecError::new)?,
        },
        _ => return Err(InterpreterCodecError::new("unknown checked action origin")),
    };
    Ok(ActionInvocationSnapshot::Native {
        native: serde_json::from_value(required_obj(action, "native")?.clone()).map_err(
            |error| InterpreterCodecError::new(format!("invalid action native identity: {error}")),
        )?,
        origin,
    })
}
