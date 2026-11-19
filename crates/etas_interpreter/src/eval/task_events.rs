use super::*;

impl EvalContext<'_> {
    pub(crate) fn take_terminal_task_events(
        &mut self,
    ) -> Vec<(
        crate::driver::scheduler::TaskHandle,
        crate::driver::scheduler::TaskState,
    )> {
        let cpu_jobs = &self.runtime.cpu_jobs;
        self.scheduler
            .take_terminal_events(|handle| !cpu_jobs.values().any(|job| job.task == handle))
    }

    pub(super) fn execute_source_scheduler_control(
        &mut self,
        item: HirItemId,
        role: etas_std::StdExecutionRole,
        args: &[InterpValue],
        bindings: &crate::control::GenericBindings,
        span: Span,
    ) -> ControlSignal {
        let Some(scheduler) = args
            .first()
            .and_then(|value| crate::value::task::source::scheduler(value, self.checked))
        else {
            return ControlSignal::missing_checked_fact(
                "scheduler control has no checked source Scheduler",
                span,
            );
        };
        if scheduler.owner != self.task.task_scope {
            return ControlSignal::runtime_fault(
                "scheduler control belongs to a different task scope",
                span,
            );
        }
        if role == etas_std::StdExecutionRole::TaskNextCompletion {
            let etas_types::ItemSignature::Flow(signature) =
                &self.checked.types.item_signatures[&item]
            else {
                unreachable!()
            };
            let result_type = match self.materialized_type(signature.output, bindings, span) {
                Ok(ty) => ty,
                Err(fault) => return ControlSignal::Fault(Box::new(fault)),
            };
            return ControlSignal::pending_task(
                crate::control::PendingTaskControl::NextCompletion {
                    scope: scheduler.owner,
                    result_type,
                    span,
                },
            );
        }
        let Some(task) = args
            .get(1)
            .and_then(|value| crate::value::task::source::task(value, self.checked))
        else {
            return ControlSignal::missing_checked_fact(
                "cancellation has no checked source Task",
                span,
            );
        };
        if task.handle.owner != scheduler.owner {
            return ControlSignal::runtime_fault(
                "cancelled task belongs to a different scope",
                span,
            );
        }
        match self.scheduler.cancel(
            task.handle,
            etas_host::execution::CancellationReason::Requested,
        ) {
            Ok(()) => ControlSignal::Value(InterpValue::Unit),
            Err(error) => ControlSignal::runtime_fault(error.to_string(), span),
        }
    }
}
