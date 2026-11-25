use super::*;

impl TaskScheduler {
    pub(crate) fn configure_resource_scope(
        &mut self,
        scope: TaskScopeId,
        report_type: etas_types::TypeId,
        span: etas_core::Span,
    ) {
        let group = self.groups.get_mut(&scope).expect("new resource scope");
        group.resource_report = Some((report_type, span));
        let owner = group.owner.expect("resource scope has its body owner");
        let task = self.tasks.get_mut(&owner).expect("resource scope body");
        task.local.resource_owner = Some((
            TaskHandle {
                owner: scope,
                task: owner,
            },
            task.local.execution.clone(),
        ));
    }

    pub(super) fn wait_for_resource_children(
        &mut self,
        handle: TaskHandle,
        outcome: &TaskOutcome,
    ) -> Result<bool, SchedulerError> {
        let group = &self.groups[&handle.owner];
        if group.resource_report.is_none()
            || group.owner != Some(handle.task)
            || group
                .tasks
                .iter()
                .filter(|id| **id != handle.task)
                .all(|id| self.tasks[id].state.is_terminal())
        {
            return Ok(false);
        }
        let task = self.tasks.get_mut(&handle.task).expect("resource owner");
        task.pending_outcome = Some(outcome.clone());
        task.state = TaskState::Waiting(TaskWait::ResourceChildren);
        task.machine = EvalMachine::new();
        task.waiting_continuation = None;
        let active = std::mem::replace(&mut task.active_slot, false);
        if active {
            self.change_active(handle.owner, false);
        }
        self.release_task_resources(handle.task);
        Ok(true)
    }

    pub(super) fn finish_resource_owner(
        &mut self,
        scope: TaskScopeId,
    ) -> Result<(), SchedulerError> {
        let group = &self.groups[&scope];
        let Some(owner) = group.owner else {
            return Ok(());
        };
        if group.resource_report.is_none()
            || self.tasks[&owner].state != TaskState::Waiting(TaskWait::ResourceChildren)
            || group
                .tasks
                .iter()
                .filter(|id| **id != owner)
                .any(|id| !self.tasks[id].state.is_terminal())
        {
            return Ok(());
        }
        let outcome = self
            .tasks
            .get_mut(&owner)
            .unwrap()
            .pending_outcome
            .take()
            .expect("resource owner has body outcome");
        self.terminal(
            TaskHandle {
                owner: scope,
                task: owner,
            },
            outcome,
        )
    }
}

pub(super) fn completion(
    (report_type, _span): (etas_types::TypeId, etas_core::Span),
    report: &etas_host::execution::TerminationReport,
    program: &crate::value::CheckedExecutionProgram,
    outcome: TaskOutcome,
) -> Result<TaskOutcome, SchedulerError> {
    let TaskOutcome::Completed(value) = outcome else {
        return Ok(outcome);
    };
    let failures = report
        .operations()
        .iter()
        .filter(|operation| operation.scope() == report.scope())
        .flat_map(|operation| operation.cleanup_errors())
        .map(|error| {
            etas_host::HostValue::Record(vec![
                (
                    "resource_kind".into(),
                    etas_host::HostValue::String("native".into()),
                ),
                (
                    "code".into(),
                    etas_host::HostValue::String(error.code.as_str().into()),
                ),
                (
                    "message".into(),
                    etas_host::HostValue::String("resource cleanup failed".into()),
                ),
            ])
        })
        .collect::<Vec<_>>();
    let invalid = |reason: String| {
        SchedulerError::Host(etas_host::HostError::new(
            etas_host::HostErrorCode::InvalidResponse,
            reason,
        ))
    };
    let fields =
        etas_types::record_fields_with_applied_params(&program.checked.type_store, report_type)
            .map_err(|error| invalid(error.to_string()))?
            .ok_or_else(|| invalid("resource scope has no checked report fields".into()))?;
    let failure_type = fields
        .fields
        .iter()
        .find(|field| field.name == "failures")
        .ok_or_else(|| invalid("resource scope report has no failures field".into()))?
        .ty;
    let failures = crate::eval::decode_checked_host_value(
        etas_host::HostValue::List(failures),
        failure_type,
        &program.checked,
    )
    .map_err(invalid)?;
    Ok(TaskOutcome::Completed(InterpValue::Nominal {
        ty: report_type,
        value: InterpValue::Record(crate::value::RecordValue::new(vec![
            ("value".into(), value),
            ("failures".into(), failures),
        ]))
        .into(),
    }))
}
