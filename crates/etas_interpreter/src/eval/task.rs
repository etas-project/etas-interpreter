use etas_core::Span;
use etas_hir::{HirEffectRef, ResolveResult, ResolvedActionRef};
use etas_std::StdTaskIntrinsic;

use super::*;

impl EvalContext<'_> {
    pub(super) fn execute_task_callable(
        &mut self,
        operation: StdTaskIntrinsic,
        checked_call: &crate::intrinsic::dispatch::CheckedStdIntrinsicCall,
        call_args: Vec<InterpValue>,
        span: Span,
    ) -> ControlSignal {
        match operation {
            StdTaskIntrinsic::Run => self.execute_task_run(call_args, span),
            StdTaskIntrinsic::Start => self.execute_task_start(call_args, span),
            StdTaskIntrinsic::Await => self.execute_task_await(checked_call, call_args, span),
            StdTaskIntrinsic::Requeue => self.execute_task_requeue(checked_call, call_args, span),
            StdTaskIntrinsic::Park => self.execute_task_park(call_args, span),
            StdTaskIntrinsic::JoinTuple
            | StdTaskIntrinsic::JoinArray
            | StdTaskIntrinsic::TryJoin
            | StdTaskIntrinsic::Collect
            | StdTaskIntrinsic::Race
            | StdTaskIntrinsic::MapConcurrent
            | StdTaskIntrinsic::MapConcurrentCollect => ControlSignal::missing_checked_fact(
                "task combinator is not registered in the current standard task ABI",
                span,
            ),
        }
    }

    fn execute_task_requeue(
        &mut self,
        checked_call: &crate::intrinsic::dispatch::CheckedStdIntrinsicCall,
        call_args: Vec<InterpValue>,
        span: Span,
    ) -> ControlSignal {
        let [scheduler, continuation, value]: [InterpValue; 3] = match call_args.try_into() {
            Ok(args) => args,
            Err(args) => {
                return ControlSignal::invalid_arguments(
                    format!(
                        "std.task.scheduler.requeue expects three arguments, got {}",
                        args.len()
                    ),
                    span,
                );
            }
        };
        let InterpValue::Scheduler(scheduler) = scheduler else {
            return ControlSignal::invalid_arguments("requeue requires a Scheduler", span);
        };
        let InterpValue::Continuation(token) = continuation else {
            return ControlSignal::invalid_arguments("requeue requires a continuation", span);
        };
        if scheduler.owner != self.scheduler.owner() || token.owner != scheduler.owner {
            return ControlSignal::runtime_fault(
                "requeue scheduler and continuation owners do not match",
                span,
            );
        }
        let Some(expected) = checked_call.parameter_types.get(2).copied() else {
            return ControlSignal::missing_checked_fact(
                "requeue is missing its checked value type",
                span,
            );
        };
        if let Err(reason) = crate::value::contract::runtime::validate_value(
            self.checked,
            self.plan,
            &value,
            expected,
            &self.storage_limits,
        ) {
            return ControlSignal::runtime_fault(
                format!("requeue value violates its checked continuation type: {reason}"),
                span,
            );
        }
        ControlSignal::pending_task(crate::control::PendingTaskControl::Requeue {
            token,
            value,
            span,
        })
    }

    fn execute_task_park(&mut self, call_args: Vec<InterpValue>, span: Span) -> ControlSignal {
        let [scheduler, task, continuation]: [InterpValue; 3] = match call_args.try_into() {
            Ok(args) => args,
            Err(args) => {
                return ControlSignal::invalid_arguments(
                    format!(
                        "std.task.scheduler.park expects three arguments, got {}",
                        args.len()
                    ),
                    span,
                );
            }
        };
        let InterpValue::Scheduler(scheduler) = scheduler else {
            return ControlSignal::invalid_arguments("park requires a Scheduler", span);
        };
        let InterpValue::Task(task) = task else {
            return ControlSignal::invalid_arguments("park requires a Task", span);
        };
        let InterpValue::Continuation(token) = continuation else {
            return ControlSignal::invalid_arguments("park requires a continuation", span);
        };
        if scheduler.owner != self.scheduler.owner()
            || task.handle.owner != scheduler.owner
            || token.owner != scheduler.owner
        {
            return ControlSignal::runtime_fault(
                "park scheduler, task, and continuation owners do not match",
                span,
            );
        }
        ControlSignal::pending_task(crate::control::PendingTaskControl::Park {
            task: task.handle,
            token,
            span,
        })
    }

    fn execute_task_run(&mut self, call_args: Vec<InterpValue>, span: Span) -> ControlSignal {
        let [body]: [InterpValue; 1] = match call_args.try_into() {
            Ok(args) => args,
            Err(args) => {
                return ControlSignal::invalid_arguments(
                    format!(
                        "std.task.run expects one scheduler callback, got {}",
                        args.len()
                    ),
                    span,
                );
            }
        };
        let InterpValue::Callable(target) = body else {
            return ControlSignal::invalid_arguments("std.task.run callback is not callable", span);
        };
        ControlSignal::pending_call(PendingCall {
            purity: crate::control::CallPurity::Ordinary,
            site: None,
            target,
            args: vec![InterpValue::Scheduler(
                crate::value::SchedulerHandleValue::new(self.scheduler.owner()),
            )],
            span,
            continuation: Continuation::BlockValue,
        })
    }

    fn execute_task_start(&mut self, call_args: Vec<InterpValue>, span: Span) -> ControlSignal {
        let [job]: [InterpValue; 1] = match call_args.try_into() {
            Ok(args) => args,
            Err(args) => {
                return ControlSignal::invalid_arguments(
                    format!("std.task.start expects one job, got {}", args.len()),
                    span,
                );
            }
        };
        let InterpValue::Callable(target) = job else {
            return ControlSignal::invalid_arguments("std.task.start job is not callable", span);
        };
        let mut machine = crate::eval::machine::EvalMachine::new();
        machine.resume(ControlSignal::pending_call(PendingCall {
            purity: crate::control::CallPurity::Ordinary,
            site: None,
            target,
            args: Vec::new(),
            span,
            continuation: Continuation::BlockValue,
        }));
        let program = self.checked_execution_program();
        match self.scheduler.spawn(
            self.task,
            machine,
            Some(program),
            self.handler_floor,
            span.range.start.0 as usize,
        ) {
            Ok(handle) => ControlSignal::Value(InterpValue::Task(
                crate::value::TaskHandleValue::new(handle),
            )),
            Err(error) => ControlSignal::runtime_fault(error.to_string(), span),
        }
    }

    fn execute_task_await(
        &mut self,
        checked_call: &crate::intrinsic::dispatch::CheckedStdIntrinsicCall,
        call_args: Vec<InterpValue>,
        span: Span,
    ) -> ControlSignal {
        let [task]: [InterpValue; 1] = match call_args.try_into() {
            Ok(args) => args,
            Err(args) => {
                return ControlSignal::invalid_arguments(
                    format!("std.task.await expects one task, got {}", args.len()),
                    span,
                );
            }
        };
        if !matches!(task, InterpValue::Task(_)) {
            return ControlSignal::invalid_arguments("std.task.await requires a Task", span);
        }
        let checked_action = match crate::control::CheckedActionInvocation::standard(
            self.checked_execution_program(),
            checked_call,
        ) {
            Ok(action) => action,
            Err(reason) => return ControlSignal::missing_checked_fact(reason, span),
        };
        ControlSignal::pending_perform(PendingPerform {
            checked_action: Some(checked_action),
            default_boundary: None,
            forwarded_call: None,
            inference: None,
            expr: None,
            action: ResolvedActionRef {
                effect: HirEffectRef {
                    path: etas_hir::unresolved_path_from_segments(&["Async"], span),
                    args: Vec::new(),
                    span,
                },
                action: "await".to_owned(),
                action_symbol: ResolveResult::Unresolved,
                span,
            },
            error_type: None,
            args: vec![task],
            span,
            continuation: Continuation::BlockValue,
            residual: None,
        })
    }
}
