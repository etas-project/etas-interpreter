use super::{ContinuationTokenValue, SchedulerHandleValue, TaskHandleValue};
use crate::value::{InterpValue as V, RecordValue};
use etas_frontend::CheckedProject;
use etas_types::{Type, TypeId, source_execution};

fn record(ty: TypeId, fields: Vec<(&str, V)>) -> V {
    V::Nominal {
        ty,
        value: V::Record(RecordValue::new(
            fields
                .into_iter()
                .map(|(name, value)| (name.into(), value))
                .collect(),
        ))
        .into(),
    }
}

pub(crate) fn completion_value(
    ty: TypeId,
    event: crate::driver::scheduler::TaskCompletion,
    scheduler: &crate::driver::scheduler::TaskScheduler,
) -> V {
    let handle = crate::driver::scheduler::TaskHandle {
        owner: scheduler.task_scope(event.task),
        task: event.task,
    };
    let completed = matches!(
        scheduler
            .outcome(handle)
            .expect("completion event refers to its retained task"),
        Some(crate::driver::scheduler::TaskOutcome::Completed(_))
    );
    record(
        ty,
        vec![
            ("task", V::u64(event.task.0)),
            ("order", V::u64(event.order)),
            ("completed", V::Bool(completed)),
        ],
    )
}

pub(crate) fn task_value(
    checked: &CheckedProject,
    ty: TypeId,
    handle: TaskHandleValue,
) -> Result<V, &'static str> {
    if !checked.hir.source_standard_types
        && matches!(checked.type_store.get(ty), Some(Type::Task(_)))
    {
        return Ok(V::Task(handle));
    }
    if !matches!(checked.type_store.get(ty), Some(Type::Applied { constructor, .. })
        if checked.types.known_std_types.execution.task == Some(TypeId(constructor.0)))
    {
        return Err("task start result differs from the selected source Task declaration");
    }
    source_execution::task(&checked.type_store, &checked.types.known_std_types, ty)
        .ok_or("task start has no selected source Task result type")?;
    Ok(record(
        ty,
        vec![
            ("handle", V::Task(handle)),
            ("id", V::u64(handle.handle.task.0)),
        ],
    ))
}

pub(crate) fn scheduler_value(
    checked: &CheckedProject,
    handle: SchedulerHandleValue,
) -> Result<V, &'static str> {
    if !checked.hir.source_standard_types {
        return Ok(V::Scheduler(handle));
    }
    let ty = checked
        .types
        .known_std_types
        .execution
        .scheduler
        .ok_or("scope has no selected source Scheduler declaration")?;
    Ok(record(ty, vec![("state", V::Scheduler(handle))]))
}

pub(crate) fn continuation_value(
    checked: &CheckedProject,
    ty: TypeId,
    token: ContinuationTokenValue,
) -> Result<V, &'static str> {
    if !checked.hir.source_standard_types
        && matches!(checked.type_store.get(ty), Some(Type::Continuation(_)))
    {
        return Ok(V::Continuation(token));
    }
    if !matches!(checked.type_store.get(ty), Some(Type::Applied { constructor, .. })
        if checked.types.known_std_types.execution.continuation == Some(TypeId(constructor.0)))
    {
        return Err("handler result differs from the selected source Continuation declaration");
    }
    source_execution::continuation_output(&checked.type_store, &checked.types.known_std_types, ty)
        .ok_or("handler has no selected source Continuation type")?;
    Ok(record(ty, vec![("state", V::Continuation(token))]))
}

fn field(value: &V, name: &str) -> Option<V> {
    let V::Nominal { value, .. } = value else {
        return None;
    };
    let V::Record(fields) = value.as_ref() else {
        return None;
    };
    fields.get(name)
}

pub(crate) fn task(value: &V, checked: &CheckedProject) -> Option<TaskHandleValue> {
    if !checked.hir.source_standard_types {
        return match value {
            V::Task(handle) => Some(*handle),
            _ => None,
        };
    }
    let V::Nominal { ty, .. } = value else {
        return None;
    };
    source_execution::task(&checked.type_store, &checked.types.known_std_types, *ty)?;
    match field(value, "handle")? {
        V::Task(handle) => Some(handle),
        _ => None,
    }
}

pub(crate) fn scheduler(value: &V, checked: &CheckedProject) -> Option<SchedulerHandleValue> {
    if !checked.hir.source_standard_types {
        return match value {
            V::Scheduler(handle) => Some(*handle),
            _ => None,
        };
    }
    let V::Nominal { ty, .. } = value else {
        return None;
    };
    if checked.types.known_std_types.execution.scheduler != Some(*ty) {
        return None;
    }
    match field(value, "state")? {
        V::Scheduler(handle) => Some(handle),
        _ => None,
    }
}

pub(crate) fn continuation(value: &V, checked: &CheckedProject) -> Option<ContinuationTokenValue> {
    if !checked.hir.source_standard_types {
        return match value {
            V::Continuation(token) => Some(*token),
            _ => None,
        };
    }
    let V::Nominal { ty, .. } = value else {
        return None;
    };
    source_execution::continuation_output(
        &checked.type_store,
        &checked.types.known_std_types,
        *ty,
    )?;
    match field(value, "state")? {
        V::Continuation(token) => Some(token),
        _ => None,
    }
}
