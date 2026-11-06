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
            StdTaskIntrinsic::Run => self.execute_task_run(checked_call, call_args, span),
            StdTaskIntrinsic::Start => self.execute_task_start(call_args, span),
            StdTaskIntrinsic::Await => self.execute_task_await(checked_call, call_args, span),
            StdTaskIntrinsic::Requeue => self.execute_task_requeue(checked_call, call_args, span),
            StdTaskIntrinsic::Park => self.execute_task_park(call_args, span),
            StdTaskIntrinsic::JoinTuple | StdTaskIntrinsic::JoinArray => {
                self.execute_task_join(checked_call, call_args, span)
            }
            StdTaskIntrinsic::TryJoin => self.execute_task_job_array(
                checked_call,
                call_args,
                crate::driver::scheduler::TaskGroupPolicy::TryJoin,
                "try_join",
                span,
            ),
            StdTaskIntrinsic::Collect => self.execute_task_job_array(
                checked_call,
                call_args,
                crate::driver::scheduler::TaskGroupPolicy::Collect,
                "collect",
                span,
            ),
            StdTaskIntrinsic::Race => self.execute_task_job_array(
                checked_call,
                call_args,
                crate::driver::scheduler::TaskGroupPolicy::Race,
                "race",
                span,
            ),
            StdTaskIntrinsic::MapConcurrent => self.execute_task_map(
                checked_call,
                call_args,
                crate::driver::scheduler::TaskGroupPolicy::Map,
                "map_concurrent",
                span,
            ),
            StdTaskIntrinsic::MapConcurrentCollect => self.execute_task_map(
                checked_call,
                call_args,
                crate::driver::scheduler::TaskGroupPolicy::MapCollect,
                "map_concurrent_collect",
                span,
            ),
        }
    }

    fn execute_task_join(
        &mut self,
        checked_call: &crate::intrinsic::dispatch::CheckedStdIntrinsicCall,
        call_args: Vec<InterpValue>,
        span: Span,
    ) -> ControlSignal {
        let [jobs]: [InterpValue; 1] = match call_args.try_into() {
            Ok(args) => args,
            Err(args) => {
                return ControlSignal::invalid_arguments(
                    format!("std.task.join expects one aggregate, got {}", args.len()),
                    span,
                );
            }
        };
        let (jobs, policy) = match jobs {
            InterpValue::Tuple(jobs) => (
                jobs.into_values(),
                crate::driver::scheduler::TaskGroupPolicy::JoinTuple,
            ),
            InterpValue::Array(jobs) => (
                jobs.into_values(),
                crate::driver::scheduler::TaskGroupPolicy::JoinArray,
            ),
            _ => {
                return ControlSignal::invalid_arguments(
                    "std.task.join requires a tuple or array of jobs",
                    span,
                );
            }
        };
        let calls = match job_calls(jobs, "join", span) {
            Ok(calls) => calls,
            Err(signal) => return signal,
        };
        self.start_task_group(checked_call, calls, policy, span)
    }

    fn execute_task_job_array(
        &mut self,
        checked_call: &crate::intrinsic::dispatch::CheckedStdIntrinsicCall,
        call_args: Vec<InterpValue>,
        policy: crate::driver::scheduler::TaskGroupPolicy,
        name: &str,
        span: Span,
    ) -> ControlSignal {
        let [jobs]: [InterpValue; 1] = match call_args.try_into() {
            Ok(args) => args,
            Err(args) => {
                return ControlSignal::invalid_arguments(
                    format!("std.task.{name} expects one array, got {}", args.len()),
                    span,
                );
            }
        };
        let InterpValue::Array(jobs) = jobs else {
            return ControlSignal::invalid_arguments(
                format!("std.task.{name} requires an array of jobs"),
                span,
            );
        };
        if policy == crate::driver::scheduler::TaskGroupPolicy::Race && jobs.borrow().is_empty() {
            return ControlSignal::invalid_arguments(
                "std.task.race requires at least one job",
                span,
            );
        }
        let calls = match job_calls(jobs.into_values(), name, span) {
            Ok(calls) => calls,
            Err(signal) => return signal,
        };
        self.start_task_group(checked_call, calls, policy, span)
    }

    fn execute_task_map(
        &mut self,
        checked_call: &crate::intrinsic::dispatch::CheckedStdIntrinsicCall,
        call_args: Vec<InterpValue>,
        policy: crate::driver::scheduler::TaskGroupPolicy,
        name: &str,
        span: Span,
    ) -> ControlSignal {
        let [inputs, mapper]: [InterpValue; 2] = match call_args.try_into() {
            Ok(args) => args,
            Err(args) => {
                return ControlSignal::invalid_arguments(
                    format!(
                        "std.task.{name} expects an array and mapper, got {}",
                        args.len()
                    ),
                    span,
                );
            }
        };
        let InterpValue::Array(inputs) = inputs else {
            return ControlSignal::invalid_arguments(
                format!("std.task.{name} requires an input array"),
                span,
            );
        };
        let InterpValue::Callable(mapper) = mapper else {
            return ControlSignal::invalid_arguments(
                format!("std.task.{name} mapper is not callable"),
                span,
            );
        };
        let calls = inputs
            .into_values()
            .into_iter()
            .map(|input| (mapper.clone(), vec![input]))
            .collect();
        self.start_task_group(checked_call, calls, policy, span)
    }

    fn start_task_group(
        &mut self,
        checked_call: &crate::intrinsic::dispatch::CheckedStdIntrinsicCall,
        calls: Vec<(crate::control::CallTarget, Vec<InterpValue>)>,
        policy: crate::driver::scheduler::TaskGroupPolicy,
        span: Span,
    ) -> ControlSignal {
        let concurrency = checked_concurrency(self.checked, checked_call);
        let parent_scope = self.task.task_scope;
        let scope = self
            .scheduler
            .create_scope_with_policy(parent_scope, concurrency, policy);
        let program = self.checked_execution_program();
        for (index, (target, args)) in calls.into_iter().enumerate() {
            let mut machine = crate::eval::machine::EvalMachine::new();
            machine.resume(ControlSignal::pending_call(PendingCall {
                purity: crate::control::CallPurity::Ordinary,
                site: None,
                target,
                args,
                span,
                continuation: Continuation::BlockValue,
            }));
            let source_index = (span.range.start.0 as usize).saturating_add(index);
            let handle = match self.scheduler.spawn_in_scope(
                self.task,
                scope,
                machine,
                Some(program.clone()),
                self.handler_floor,
                source_index,
            ) {
                Ok(handle) => handle,
                Err(error) => return ControlSignal::runtime_fault(error.to_string(), span),
            };
            self.events
                .push(crate::orchestration::WorkflowEvent::TaskStarted {
                    scope: handle.owner.0,
                    task: handle.task.0,
                    source_index,
                });
        }
        self.events
            .push(crate::orchestration::WorkflowEvent::ScopeEntered {
                scope: scope.0,
                parent: parent_scope.0,
                concurrency: u32::try_from(concurrency.get())
                    .expect("checked concurrency limit fits trace ABI"),
            });
        self.events
            .push(crate::orchestration::WorkflowEvent::ScopePolicy {
                scope: scope.0,
                policy: policy.as_str().to_owned(),
            });
        ControlSignal::pending_task(crate::control::PendingTaskControl::Run { scope, span })
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
        if scheduler.owner != self.task.task_scope || token.owner != scheduler.owner {
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
        if scheduler.owner != self.task.task_scope
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

    fn execute_task_run(
        &mut self,
        checked_call: &crate::intrinsic::dispatch::CheckedStdIntrinsicCall,
        call_args: Vec<InterpValue>,
        span: Span,
    ) -> ControlSignal {
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
        let concurrency = checked_concurrency(self.checked, checked_call);
        let parent_scope = self.task.task_scope;
        let scope = self.scheduler.create_scope(parent_scope, concurrency);
        let mut machine = crate::eval::machine::EvalMachine::new();
        machine.resume(ControlSignal::pending_call(PendingCall {
            purity: crate::control::CallPurity::Ordinary,
            site: None,
            target,
            args: vec![InterpValue::Scheduler(
                crate::value::SchedulerHandleValue::new(scope),
            )],
            span,
            continuation: Continuation::BlockValue,
        }));
        let program = self.checked_execution_program();
        let owner = match self.scheduler.spawn_in_scope(
            self.task,
            scope,
            machine,
            Some(program),
            self.handler_floor,
            span.range.start.0 as usize,
        ) {
            Ok(owner) => owner,
            Err(error) => return ControlSignal::runtime_fault(error.to_string(), span),
        };
        if let Err(error) = self.scheduler.set_scope_owner(scope, owner) {
            return ControlSignal::runtime_fault(error.to_string(), span);
        }
        self.events
            .push(crate::orchestration::WorkflowEvent::ScopeEntered {
                scope: scope.0,
                parent: parent_scope.0,
                concurrency: u32::try_from(concurrency.get())
                    .expect("checked concurrency limit fits trace ABI"),
            });
        self.events
            .push(crate::orchestration::WorkflowEvent::ScopePolicy {
                scope: scope.0,
                policy: crate::driver::scheduler::TaskGroupPolicy::Run
                    .as_str()
                    .to_owned(),
            });
        self.events
            .push(crate::orchestration::WorkflowEvent::TaskStarted {
                scope: owner.owner.0,
                task: owner.task.0,
                source_index: span.range.start.0 as usize,
            });
        ControlSignal::pending_task(crate::control::PendingTaskControl::Run { scope, span })
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
            Ok(handle) => {
                self.events
                    .push(crate::orchestration::WorkflowEvent::TaskStarted {
                        scope: handle.owner.0,
                        task: handle.task.0,
                        source_index: span.range.start.0 as usize,
                    });
                ControlSignal::Value(InterpValue::Task(crate::value::TaskHandleValue::new(
                    handle,
                )))
            }
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

fn job_calls(
    jobs: Vec<InterpValue>,
    name: &str,
    span: Span,
) -> Result<Vec<(crate::control::CallTarget, Vec<InterpValue>)>, ControlSignal> {
    jobs.into_iter()
        .map(|job| match job {
            InterpValue::Callable(target) => Ok((target, Vec::new())),
            _ => Err(ControlSignal::invalid_arguments(
                format!("std.task.{name} requires callable jobs"),
                span,
            )),
        })
        .collect()
}

fn checked_concurrency(
    checked: &etas_frontend::CheckedProject,
    checked_call: &crate::intrinsic::dispatch::CheckedStdIntrinsicCall,
) -> std::num::NonZeroUsize {
    let concurrency = checked
        .concurrency
        .sites
        .get(&checked_call.origin)
        .and_then(|site| site.concurrency_limit)
        .map(|limit| usize::try_from(limit).expect("u32 concurrency limit fits usize"))
        .unwrap_or(crate::driver::scheduler::MAX_CONCURRENCY);
    std::num::NonZeroUsize::new(concurrency).expect("checked concurrency limit is non-zero")
}
