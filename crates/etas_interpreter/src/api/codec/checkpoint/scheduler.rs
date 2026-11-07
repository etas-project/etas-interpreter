use super::*;
use crate::driver::scheduler::{
    ContinuationArenaSnapshot, ContinuationEntrySnapshot, ContinuationId, ContinuationState,
    TaskIdentity, TaskSchedulerMetadataSnapshot, TaskScopeId,
};

pub(in crate::api::codec) fn scheduler_metadata_json(
    scheduler: &TaskSchedulerMetadataSnapshot,
    budget: &mut snapshot::EncodingBudget,
) -> Result<Value, InterpreterCodecError> {
    let mut value = json!({
        "next_scope": scheduler.next_scope,
        "next_task": scheduler.next_task,
        "groups": scheduler.groups,
        "ready": scheduler.ready,
        "wakeups": scheduler.wakeups,
        "resources": scheduler.resources,
        "continuations": null,
    });
    let mut pending = vec![&value];
    while let Some(node) = pending.pop() {
        let bytes = match node {
            Value::Object(object) => {
                pending.extend(object.values());
                object.keys().map(String::len).sum()
            }
            Value::Array(array) => {
                pending.extend(array);
                0
            }
            Value::String(text) => text.len(),
            _ => 0,
        };
        budget.metadata(1, bytes)?;
    }
    value["continuations"] = continuation_arena_json(&scheduler.continuations, budget)?;
    Ok(value)
}

pub(in crate::api::codec) fn scheduler_metadata_from_json(
    limits: &etas_host::StorageLimits,
    value: &Value,
    checked: &etas_frontend::CheckedProject,
) -> Result<TaskSchedulerMetadataSnapshot, InterpreterCodecError> {
    fn field<T: serde::de::DeserializeOwned>(
        value: &Value,
        field: &'static str,
    ) -> Result<T, InterpreterCodecError> {
        serde_json::from_value(required_obj(value, field)?.clone()).map_err(|error| {
            InterpreterCodecError::new(format!("invalid checkpoint scheduler {field}: {error}"))
        })
    }
    Ok(TaskSchedulerMetadataSnapshot {
        next_scope: required_u64(value, "next_scope")?,
        next_task: required_u64(value, "next_task")?,
        groups: field(value, "groups")?,
        ready: field(value, "ready")?,
        wakeups: field(value, "wakeups")?,
        resources: field(value, "resources")?,
        continuations: continuation_arena_from_json(
            limits,
            required_obj(value, "continuations")?,
            checked,
        )?,
    })
}

pub(in crate::api::codec) fn continuation_arena_json(
    arena: &ContinuationArenaSnapshot,
    budget: &mut snapshot::EncodingBudget,
) -> Result<Value, InterpreterCodecError> {
    arena.validate().map_err(InterpreterCodecError::new)?;
    let entries = arena
        .entries
        .iter()
        .map(|(id, entry)| {
            budget.metadata(7, 0)?;
            Ok(json!({
                "id": id.0,
                "owner": entry.owner.0,
                "origin": entry.origin,
                "output": entry.output.0,
                "state": entry.state,
                "continuation": entry.continuation,
                "program": entry.program,
                "binder": entry.binder,
            }))
        })
        .collect::<Result<Vec<_>, InterpreterCodecError>>()?;
    Ok(json!({ "next": arena.next, "entries": entries }))
}

pub(crate) fn continuation_arena_from_json(
    _limits: &etas_host::StorageLimits,
    value: &Value,
    _checked: &etas_frontend::CheckedProject,
) -> Result<ContinuationArenaSnapshot, InterpreterCodecError> {
    let mut entries = std::collections::BTreeMap::new();
    for entry in required_array(value, "entries")? {
        let id = ContinuationId(required_u64(entry, "id")?);
        let origin: TaskIdentity = serde_json::from_value(required_obj(entry, "origin")?.clone())
            .map_err(|error| {
            InterpreterCodecError::new(format!("invalid continuation origin: {error}"))
        })?;
        let state: ContinuationState =
            serde_json::from_value(required_obj(entry, "state")?.clone()).map_err(|error| {
                InterpreterCodecError::new(format!("invalid continuation state: {error}"))
            })?;
        let saved = ContinuationEntrySnapshot {
            binder: optional_u32(entry, "binder")?.map(etas_hir::SymbolId),
            program: serde_json::from_value(required_obj(entry, "program")?.clone()).map_err(
                |error| {
                    InterpreterCodecError::new(format!("invalid continuation program: {error}"))
                },
            )?,
            owner: TaskScopeId(required_u64(entry, "owner")?),
            origin,
            output: etas_types::TypeId(required_u32(entry, "output")?),
            state,
            continuation: optional_u32(entry, "continuation")?,
        };
        if entries.insert(id, saved).is_some() {
            return Err(InterpreterCodecError::new(
                "duplicate checkpoint continuation token",
            ));
        }
    }
    let arena = ContinuationArenaSnapshot {
        next: required_u64(value, "next")?,
        entries,
    };
    arena.validate().map_err(InterpreterCodecError::new)?;
    Ok(arena)
}

#[cfg(test)]
pub(crate) fn roundtrip_continuation_arena(
    arena: &ContinuationArenaSnapshot,
    continuations: &[ContinuationSnapshot],
) -> (ContinuationArenaSnapshot, Vec<ContinuationSnapshot>) {
    let checked = crate::testing::project::checked_project(
        "module app.main; flow main() -> unit { return; }",
    );
    let value = continuation_arena_json(arena, &mut snapshot::EncodingBudget::default()).unwrap();
    let bytes = serde_json::to_vec(&value).unwrap();
    let value = serde_json::from_slice(&bytes).unwrap();
    let limits = etas_host::StorageLimits::default();
    let arena = continuation_arena_from_json(&limits, &value, &checked).unwrap();
    let continuations = continuations
        .iter()
        .map(|continuation| {
            let encoded = snapshot::continuation_json(continuation);
            let bytes = serde_json::to_vec(&encoded).unwrap();
            let encoded = serde_json::from_slice(&bytes).unwrap();
            machine::continuation_from_snapshot(&limits, &encoded, &checked).unwrap()
        })
        .collect();
    (arena, continuations)
}

#[cfg(test)]
pub(crate) fn roundtrip_scheduler_metadata(
    scheduler: &TaskSchedulerMetadataSnapshot,
) -> TaskSchedulerMetadataSnapshot {
    let checked = crate::testing::project::checked_project(
        "module app.main; flow main() -> unit { return; }",
    );
    let value =
        scheduler_metadata_json(scheduler, &mut snapshot::EncodingBudget::default()).unwrap();
    let bytes = serde_json::to_vec(&value).unwrap();
    let value = serde_json::from_slice(&bytes).unwrap();
    scheduler_metadata_from_json(&etas_host::StorageLimits::default(), &value, &checked).unwrap()
}
