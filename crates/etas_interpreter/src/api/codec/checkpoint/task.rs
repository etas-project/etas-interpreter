use super::*;
use crate::orchestration::{
    RootTaskWaitSnapshot, ScheduledTaskSnapshot, TaskLocalSnapshot, TaskOutcomeSnapshot,
    TaskSchedulerSnapshot,
};

pub(in crate::api::codec) fn encode(
    scheduler: &TaskSchedulerSnapshot,
    budget: &mut snapshot::EncodingBudget,
) -> Result<Value, InterpreterCodecError> {
    let tasks = scheduler
        .tasks
        .iter()
        .map(|(id, task)| {
            let mut value = json!({
            "inherited_dynamic": task.inherited_dynamic,
                "id": id, "scope": task.scope, "program": task.program,
                "local": { "parent": task.local.parent,
                    "model_policy": machine::model_policy_snapshot(&task.local.model_policy),
                    "handlers": task.local.handlers, "retries": task.local.retries,
                    "cancellation": task.local.cancellation },
                "handler_floor": task.handler_floor, "state": task.state,
                "waiters": task.waiters, "waiting_continuation": task.waiting_continuation,
                "source_index": task.source_index, "active_slot": task.active_slot,
                "cpu_pending": task.cpu_pending, "resource_keys": task.resource_keys,
                "machine": null, "outcome": null,
            });
            budget.json_metadata(&value)?;
            value["machine"] = task
                .machine
                .as_ref()
                .map(|machine| machine_json_with_budget(machine, budget))
                .transpose()?
                .unwrap_or(Value::Null);
            value["start"] = task
                .start
                .as_ref()
                .map(|call| snapshot::call_target_json_with_budget(call, budget))
                .transpose()?
                .unwrap_or(Value::Null);
            value["outcome"] = task
                .outcome
                .as_ref()
                .map(|outcome| outcome_json(outcome, budget))
                .transpose()?
                .unwrap_or(Value::Null);
            Ok(value)
        })
        .collect::<Result<Vec<_>, InterpreterCodecError>>()?;
    let mut value = json!({"counters": scheduler.counters, "diagnostics": scheduler.diagnostics,
        "root_cancellation": scheduler.root_cancellation});
    budget.json_metadata(&value)?;
    value["metadata"] = scheduler::scheduler_metadata_json(&scheduler.metadata, budget)?;
    value["tasks"] = tasks.into();
    value["root_wait"] = scheduler
        .root_wait
        .as_ref()
        .map(|wait| root_wait_json(wait, budget))
        .transpose()?
        .unwrap_or(Value::Null);
    Ok(value)
}

fn outcome_json(
    outcome: &TaskOutcomeSnapshot,
    budget: &mut snapshot::EncodingBudget,
) -> Result<Value, InterpreterCodecError> {
    Ok(match outcome {
        TaskOutcomeSnapshot::Completed(value) => {
            json!({"kind":"completed", "value":snapshot::value_json_with_budget(value, budget)?})
        }
        TaskOutcomeSnapshot::Raised {
            error_type,
            payload,
            span,
        } => json!({"kind":"raised", "type":error_type,
            "value":snapshot::value_json_with_budget(payload, budget)?, "span":span_json(*span)}),
        TaskOutcomeSnapshot::Failed(failure) => {
            let value = json!({"kind":"failed", "failure":failure});
            budget.json_metadata(&value)?;
            value
        }
        TaskOutcomeSnapshot::Cancelled(cause) => json!({"kind":"cancelled", "cause":cause}),
    })
}

pub(in crate::api::codec) fn decode(
    limits: &etas_host::StorageLimits,
    value: &Value,
    source: &etas_frontend::CheckedProject,
    revisions: Option<&crate::api::code::PreparedRevisionSet>,
) -> Result<TaskSchedulerSnapshot, InterpreterCodecError> {
    let entries = required_array(value, "tasks")?;
    if entries.len() > limits.max_nodes {
        return Err(InterpreterCodecError::new(
            "checkpoint task count exceeds resource limits",
        ));
    }
    let mut tasks = std::collections::BTreeMap::new();
    for entry in entries {
        let id = field(entry, "id")?;
        let program: crate::orchestration::ExecutionProgramSnapshot = field(entry, "program")?;
        let checked = match revisions {
            Some(revisions) => revisions
                .checked_program(&program.program)
                .map_err(InterpreterCodecError::new)?,
            None if matches!(
                program.program,
                crate::orchestration::CheckpointProgramReference::Source
            ) =>
            {
                source.clone()
            }
            None => {
                return Err(InterpreterCodecError::new(
                    "checkpoint task requires prepared revisions",
                ));
            }
        };
        let local = required_obj(entry, "local")?;
        let task = ScheduledTaskSnapshot {
            inherited_dynamic: field(entry, "inherited_dynamic")?,
            start: match required_obj(entry, "start")? {
                Value::Null => None,
                call => Some(
                    machine::call_target_snapshot_from_json(limits, call)
                        .map_err(InterpreterCodecError::new)?,
                ),
            },
            scope: field(entry, "scope")?,
            program,
            local: TaskLocalSnapshot {
                parent: field(local, "parent")?,
                model_policy: machine::model_policy_from_snapshot(required_obj(
                    local,
                    "model_policy",
                )?)
                .map_err(InterpreterCodecError::new)?,
                handlers: field(local, "handlers")?,
                retries: field(local, "retries")?,
                cancellation: field(local, "cancellation")?,
            },
            machine: match required_obj(entry, "machine")? {
                Value::Null => None,
                value => Some(machine_from_json_with_revisions(
                    limits, value, &checked, revisions,
                )?),
            },
            handler_floor: required_usize(entry, "handler_floor")?,
            state: field(entry, "state")?,
            outcome: match required_obj(entry, "outcome")? {
                Value::Null => None,
                outcome => Some(outcome_from_json(limits, outcome)?),
            },
            waiters: field(entry, "waiters")?,
            waiting_continuation: field(entry, "waiting_continuation")?,
            source_index: required_usize(entry, "source_index")?,
            active_slot: required_bool(entry, "active_slot")?,
            cpu_pending: required_bool(entry, "cpu_pending")?,
            resource_keys: field(entry, "resource_keys")?,
        };
        if tasks.insert(id, task).is_some() {
            return Err(InterpreterCodecError::new("duplicate checkpoint task"));
        }
    }
    Ok(TaskSchedulerSnapshot {
        metadata: scheduler::scheduler_metadata_from_json(
            limits,
            required_obj(value, "metadata")?,
            source,
        )?,
        tasks,
        root_cancellation: field(value, "root_cancellation")?,
        root_wait: match required_obj(value, "root_wait")? {
            Value::Null => None,
            value => Some(root_wait_from_json(limits, value)?),
        },
        counters: field(value, "counters")?,
        diagnostics: field(value, "diagnostics")?,
    })
}

fn root_wait_json(
    wait: &RootTaskWaitSnapshot,
    budget: &mut snapshot::EncodingBudget,
) -> Result<Value, InterpreterCodecError> {
    Ok(match wait {
        RootTaskWaitSnapshot::Await {
            target,
            continuation,
        } => json!({"kind":"await", "target":target, "continuation":continuation}),
        RootTaskWaitSnapshot::Run { scope } => json!({"kind":"run", "scope":scope}),
        RootTaskWaitSnapshot::Transfer {
            target,
            token,
            continuation,
            output,
            span,
        } => json!({"kind":"transfer",
            "target":target, "token":token, "continuation":continuation, "output":output, "span":span_json(*span)}),
        RootTaskWaitSnapshot::Settling(outcome) => {
            json!({"kind":"settling", "outcome":outcome_json(outcome, budget)?})
        }
    })
}

fn root_wait_from_json(
    limits: &etas_host::StorageLimits,
    value: &Value,
) -> Result<RootTaskWaitSnapshot, InterpreterCodecError> {
    Ok(match required_str(value, "kind")? {
        "await" => RootTaskWaitSnapshot::Await {
            target: field(value, "target")?,
            continuation: required_u32(value, "continuation")?,
        },
        "run" => RootTaskWaitSnapshot::Run {
            scope: field(value, "scope")?,
        },
        "transfer" => RootTaskWaitSnapshot::Transfer {
            target: field(value, "target")?,
            token: field(value, "token")?,
            continuation: required_u32(value, "continuation")?,
            output: field(value, "output")?,
            span: span_from_json(required_obj(value, "span")?)?,
        },
        "settling" => RootTaskWaitSnapshot::Settling(outcome_from_json(
            limits,
            required_obj(value, "outcome")?,
        )?),
        kind => {
            return Err(InterpreterCodecError::new(format!(
                "unsupported checkpoint root wait `{kind}`"
            )));
        }
    })
}

fn outcome_from_json(
    limits: &etas_host::StorageLimits,
    value: &Value,
) -> Result<TaskOutcomeSnapshot, InterpreterCodecError> {
    Ok(match required_str(value, "kind")? {
        "completed" => TaskOutcomeSnapshot::Completed(value::snapshot_from_json_with_limits(
            limits,
            required_obj(value, "value")?,
        )?),
        "raised" => TaskOutcomeSnapshot::Raised {
            error_type: field(value, "type")?,
            payload: value::snapshot_from_json_with_limits(limits, required_obj(value, "value")?)?,
            span: span_from_json(required_obj(value, "span")?)?,
        },
        "failed" => TaskOutcomeSnapshot::Failed(field(value, "failure")?),
        "cancelled" => TaskOutcomeSnapshot::Cancelled(field(value, "cause")?),
        kind => {
            return Err(InterpreterCodecError::new(format!(
                "unsupported checkpoint task outcome `{kind}`"
            )));
        }
    })
}

fn field<T: serde::de::DeserializeOwned>(
    value: &Value,
    name: &'static str,
) -> Result<T, InterpreterCodecError> {
    serde_json::from_value(required_obj(value, name)?.clone()).map_err(|error| {
        InterpreterCodecError::new(format!("invalid checkpoint task {name}: {error}"))
    })
}

#[cfg(test)]
pub(crate) fn roundtrip(
    scheduler: &TaskSchedulerSnapshot,
    checked: &etas_frontend::CheckedProject,
) -> TaskSchedulerSnapshot {
    let encoded = encode(scheduler, &mut snapshot::EncodingBudget::default()).unwrap();
    let bytes = serde_json::to_vec(&encoded).unwrap();
    let value = serde_json::from_slice(&bytes).unwrap();
    decode(&etas_host::StorageLimits::default(), &value, checked, None).unwrap()
}
