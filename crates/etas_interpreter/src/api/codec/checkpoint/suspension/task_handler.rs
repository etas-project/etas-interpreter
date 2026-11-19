use super::*;
use crate::orchestration::{
    ActionEvidenceSnapshot, ActionProjectionSnapshot, SuspendedTaskHandlerSnapshot,
    TaskHandlerWaitSnapshot as Wait,
};

pub(in crate::api::codec) fn encode(
    node: &SuspendedTaskHandlerSnapshot,
    budget: &mut snapshot::EncodingBudget,
) -> Result<Value, InterpreterCodecError> {
    let mut value = json!({
        "child": node.child, "owner_task": node.owner_task, "handler": node.handler,
        "owner": node.owner, "owner_scoped": node.owner_scoped,
        "owner_handler_floor": node.owner_handler_floor,
        "handler_base": node.handler_base, "retry_base": node.retry_base,
        "active_scoped": node.active_scoped, "frames": [], "wait": null,
        "handlers": node.handlers.handlers.iter().map(|handler| json!({
            "id": handler.id.0, "handled_actions": handler.handled_actions,
            "handlers": handler.handlers.iter().map(handler_arm_json).collect::<Vec<_>>(),
            "span": span_json(handler.span),
        })).collect::<Vec<_>>(),
        "retries": node.retries.attempts.iter().map(|retry| json!({"id": retry.id.0, "ordinal": retry.ordinal})).collect::<Vec<_>>(),
        "model_policy": machine::model_policy_snapshot(&node.model_policy),
        "span": span_json(node.span),
        "projection": {
            "source": {"origin": null, "projections": node.projection.source.projections},
            "steps": node.projection.steps,
        },
    });
    budget.json_metadata(&value)?;
    let frames = MachineSnapshot::capture(node.frames.clone());
    for (index, frame) in frames.frames.iter().enumerate() {
        let mut frame = machine_frame_json(frame, budget)?;
        frame["program_owner"] = json!(frames.program_owner(index));
        value["frames"]
            .as_array_mut()
            .expect("task frames array")
            .push(frame);
    }
    value["projection"]["source"]["origin"] =
        encode_action(&node.projection.source.origin, budget)?;
    let mut wait = match &node.wait {
        Wait::Completion {
            scope,
            result_type,
            span,
            ..
        } => {
            json!({"kind":"completion", "scope":scope, "result_type":result_type, "span":span_json(*span), "continuation":null})
        }
        Wait::Ready(_) => json!({"kind":"ready", "input":null}),
        Wait::Await { target, .. } => json!({"kind":"await", "target":target, "continuation":null}),
        Wait::Run { scope } => json!({"kind":"run", "scope":scope}),
        Wait::Transfer {
            target,
            token,
            output,
            span,
            ..
        } => json!({
            "kind":"transfer", "target":target, "token":token, "output":output,
            "span":span_json(*span), "continuation":null,
        }),
    };
    budget.json_metadata(&wait)?;
    match &node.wait {
        Wait::Ready(input) => wait["input"] = input::encode(input, budget)?,
        Wait::Await { continuation, .. }
        | Wait::Transfer { continuation, .. }
        | Wait::Completion { continuation, .. } => {
            wait["continuation"] = snapshot::continuation_json_with_budget(continuation, budget)?;
        }
        Wait::Run { .. } => (),
    }
    value["wait"] = wait;
    Ok(value)
}

pub(crate) fn decode(
    limits: &etas_host::StorageLimits,
    value: &Value,
    revisions: &crate::api::code::PreparedRevisionSet,
) -> Result<SuspendedTaskHandlerSnapshot, InterpreterCodecError> {
    fn field<T: serde::de::DeserializeOwned>(
        value: &Value,
        name: &'static str,
    ) -> Result<T, InterpreterCodecError> {
        serde_json::from_value(required_obj(value, name)?.clone()).map_err(|error| {
            InterpreterCodecError::new(format!("invalid task Handler {name}: {error}"))
        })
    }
    let owner = field(value, "owner")?;
    let owner_program = revisions
        .execution_snapshot(&owner)
        .map_err(InterpreterCodecError::new)?;
    let frames = machine_frames_from_json(limits, value, &owner_program.checked, Some(revisions))?;
    let mut active = owner_program;
    for frame in &frames.frames {
        if let MachineFrameSnapshot::ProgramReturn(boundary) = frame {
            active = crate::eval::machine::snapshot::program::resolve_destination(
                boundary,
                &active.checked,
                Some(revisions),
            )
            .map_err(InterpreterCodecError::new)?;
        }
    }
    let wait = required_obj(value, "wait")?;
    let continuation = || {
        machine::continuation_from_snapshot(
            limits,
            required_obj(wait, "continuation")?,
            &active.checked,
        )
        .map_err(InterpreterCodecError::new)
    };
    let wait = match required_str(wait, "kind")? {
        "completion" => Wait::Completion {
            scope: field(wait, "scope")?,
            continuation: continuation()?,
            result_type: field(wait, "result_type")?,
            span: span_from_json(required_obj(wait, "span")?)?,
        },
        "ready" => Wait::Ready(Box::new(input::decode(
            limits,
            required_obj(wait, "input")?,
            &active.checked,
        )?)),
        "await" => Wait::Await {
            target: field(wait, "target")?,
            continuation: continuation()?,
        },
        "run" => Wait::Run {
            scope: field(wait, "scope")?,
        },
        "transfer" => Wait::Transfer {
            target: field(wait, "target")?,
            token: field(wait, "token")?,
            continuation: continuation()?,
            output: field(wait, "output")?,
            span: span_from_json(required_obj(wait, "span")?)?,
        },
        kind => {
            return Err(InterpreterCodecError::new(format!(
                "unknown task Handler wait {kind}"
            )));
        }
    };
    let projection = required_obj(value, "projection")?;
    let source = required_obj(projection, "source")?;
    Ok(SuspendedTaskHandlerSnapshot {
        child: field(value, "child")?,
        owner_task: field(value, "owner_task")?,
        handler: field(value, "handler")?,
        owner,
        owner_scoped: field(value, "owner_scoped")?,
        owner_handler_floor: required_usize(value, "owner_handler_floor")?,
        handler_base: required_usize(value, "handler_base")?,
        retry_base: required_usize(value, "retry_base")?,
        active_scoped: field(value, "active_scoped")?,
        frames: frames.frames,
        wait,
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
        projection: ActionProjectionSnapshot {
            source: ActionEvidenceSnapshot {
                origin: decode_action(limits, required_obj(source, "origin")?)?,
                projections: field(source, "projections")?,
            },
            steps: field(projection, "steps")?,
        },
        span: span_from_json(required_obj(value, "span")?)?,
    })
}

#[cfg(test)]
pub(crate) fn roundtrip(
    node: &SuspendedTaskHandlerSnapshot,
    revisions: &crate::api::code::PreparedRevisionSet,
) -> Result<SuspendedTaskHandlerSnapshot, InterpreterCodecError> {
    let value = encode(node, &mut snapshot::EncodingBudget::default())?;
    let bytes = serde_json::to_vec(&value).unwrap();
    let value = serde_json::from_slice(&bytes).unwrap();
    decode(&Default::default(), &value, revisions)
}
