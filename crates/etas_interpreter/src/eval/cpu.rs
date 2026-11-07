use std::sync::atomic::Ordering;

use crate::control::{CallPurity, CallTarget, Continuation, ControlSignal, PendingCall};
use crate::driver::cpu::{
    CpuBudgetDelta, CpuCallableId, CpuContractViolation, CpuExecutionFailure, CpuTaskCapsule,
    CpuTaskCompletion, CpuTaskMetrics, FrozenValue,
};
use crate::eval::machine::{EvalMachine, MachinePoll, PendingBoundary};

pub(crate) fn execute(
    capsule: CpuTaskCapsule,
    worker: crate::orchestration::CpuWorkerId,
) -> CpuTaskCompletion {
    let started = std::time::Instant::now();
    let dispatch_sequence = capsule.dispatch_sequence;
    let job = capsule.job;
    let mut timings = capsule.timings.clone();
    timings.queue_wait = capsule
        .submitted
        .map_or(std::time::Duration::ZERO, |submitted| submitted.elapsed());
    let result = execute_inner(&capsule, &mut timings);
    CpuTaskCompletion {
        job,
        consumed_budget: CpuBudgetDelta {
            steps: result.steps,
        },
        result: result.value,
        metrics: CpuTaskMetrics {
            worker,
            dispatch_sequence,
            thread: Some(std::thread::current().id()),
            elapsed: started.elapsed(),
            finished_at: std::time::Instant::now(),
            timings,
        },
    }
}

struct CpuRunResult {
    value: Result<FrozenValue, CpuExecutionFailure>,
    steps: u64,
}

fn execute_inner(
    capsule: &CpuTaskCapsule,
    timings: &mut crate::api::CpuTaskTimings,
) -> CpuRunResult {
    if capsule.cancellation.load(Ordering::Acquire) {
        return failed(CpuExecutionFailure::Cancelled(
            etas_host::execution::CancellationReason::Requested,
        ));
    }
    let CpuCallableId::Flow(item) = capsule.callable;
    let started = std::time::Instant::now();
    let args = capsule
        .arguments
        .clone()
        .into_iter()
        .map(FrozenValue::thaw)
        .collect::<Vec<_>>();
    timings.input_thaw = started.elapsed();
    let started = std::time::Instant::now();
    let contract = &capsule.contract;
    let (program, mut run_state) = super::EvalContextInput {
        replay_keys: contract.replay_keys.clone(),
        code_definitions: None,
        syntax_limits: contract.syntax_limits,
        storage_limits: contract.storage_limits.clone(),
        event_observer: None,
        execution: contract.execution.clone(),
        checked: &capsule.program.checked,
        plan: &capsule.program.plan,
        host_context: contract.host_context.clone(),
        model_policy: contract.model_policy.clone(),
        execution_limits: contract.execution_limits,
        cpu_threads: std::num::NonZeroUsize::MIN,
        cpu_min_work: 10_000,
        consumed_steps: 0,
        current_session: None,
        entry_item: contract.entry_item,
        entry_args: &args,
    }
    .into_parts();
    run_state.shared.safe_points =
        super::safe_point::ExecutionSafePointScheduler::with_ledger(capsule.budget.steps.clone());
    let mut eval = super::EvalContext::new(program, &mut run_state);
    let span = capsule.call_span;
    let mut machine = EvalMachine::new();
    machine.resume(ControlSignal::pending_call(PendingCall {
        purity: CallPurity::Ordinary,
        site: None,
        target: CallTarget::FlowItem(item),
        args: args.clone(),
        span,
        continuation: Continuation::BlockValue,
    }));
    timings.bootstrap = started.elapsed();
    let started = std::time::Instant::now();
    let value = loop {
        if capsule.cancellation.load(Ordering::Acquire) {
            break Err(CpuExecutionFailure::Cancelled(
                etas_host::execution::CancellationReason::Requested,
            ));
        }
        match machine.run_until_yield(&mut eval) {
            MachinePoll::Complete(value) => {
                let freeze_start = std::time::Instant::now();
                let frozen = FrozenValue::freeze(&value).ok_or_else(|| {
                    CpuExecutionFailure::ContractViolation(CpuContractViolation::NonFreezableResult)
                });
                timings.output_freeze = freeze_start.elapsed();
                break frozen;
            }
            MachinePoll::CooperativeYield | MachinePoll::ProgramChanged => continue,
            MachinePoll::Cancelled(cause) => {
                break Err(CpuExecutionFailure::Cancelled(cause.reason().clone()));
            }
            MachinePoll::Fault(fault) => {
                break Err(CpuExecutionFailure::ExecutionFault(fault));
            }
            MachinePoll::Yield(boundary) => {
                break Err(CpuExecutionFailure::ContractViolation(
                    CpuContractViolation::EffectBoundary(boundary_kind(&boundary)),
                ));
            }
        }
    };
    timings.execution = started.elapsed().saturating_sub(timings.output_freeze);
    drop(eval);
    CpuRunResult {
        value,
        steps: run_state.shared.safe_points.local_consumed_steps(),
    }
}

fn failed(failure: CpuExecutionFailure) -> CpuRunResult {
    CpuRunResult {
        value: Err(failure),
        steps: 0,
    }
}

fn boundary_kind(boundary: &PendingBoundary) -> &'static str {
    match boundary {
        PendingBoundary::Perform(_) => "effect",
        PendingBoundary::Memory(_) => "memory",
        PendingBoundary::Session(_) => "session",
        PendingBoundary::Console(_) => "console",
        PendingBoundary::Command(_) => "command",
        PendingBoundary::Model(_) => "model",
        PendingBoundary::Tool(_) => "tool",
        PendingBoundary::Host(_) => "host",
        PendingBoundary::Task(_) => "task",
    }
}
