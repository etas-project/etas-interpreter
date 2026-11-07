use std::sync::atomic::Ordering;

use crate::control::{CallPurity, CallTarget, Continuation, ControlSignal, PendingCall};
use crate::driver::cpu::{
    CpuBudgetDelta, CpuCallableId, CpuExecutionFailure, CpuTaskCapsule, CpuTaskCompletion,
    CpuTaskMetrics, FrozenValue,
};
use crate::eval::machine::{EvalMachine, MachinePoll, PendingBoundary};

pub(crate) fn execute(capsule: CpuTaskCapsule) -> CpuTaskCompletion {
    let started = std::time::Instant::now();
    let dispatch_sequence = capsule.dispatch_sequence;
    let job = capsule.job;
    let result = execute_inner(&capsule);
    CpuTaskCompletion {
        job,
        consumed_budget: CpuBudgetDelta {
            steps: result.steps,
        },
        result: result.value,
        metrics: CpuTaskMetrics {
            dispatch_sequence,
            thread: std::thread::current().id(),
            elapsed: started.elapsed(),
        },
    }
}

struct CpuRunResult {
    value: Result<FrozenValue, CpuExecutionFailure>,
    steps: u64,
}

fn execute_inner(capsule: &CpuTaskCapsule) -> CpuRunResult {
    if capsule.cancellation.load(Ordering::Acquire) {
        return failed(CpuExecutionFailure::Cancelled(
            etas_host::execution::CancellationReason::Requested,
        ));
    }
    let CpuCallableId::Flow(item) = capsule.callable;
    let args = capsule
        .arguments
        .clone()
        .into_iter()
        .map(FrozenValue::thaw)
        .collect::<Vec<_>>();
    let mut options = crate::api::RunOptions::default();
    options.execution_limits = capsule.budget.execution_limits;
    options.host_context.budget = capsule.budget.budget.clone();
    let replay_keys = match super::replay::RunReplayKeys::start(&options) {
        Ok(keys) => keys,
        Err(message) => return failed(CpuExecutionFailure::Fault(message)),
    };
    let execution = capsule.execution.clone();
    let (program, mut run_state) = super::EvalContextInput {
        replay_keys,
        code_definitions: None,
        syntax_limits: options.syntax_limits,
        storage_limits: options.storage_limits,
        event_observer: None,
        execution,
        checked: &capsule.program.checked,
        plan: &capsule.program.plan,
        host_context: options.host_context,
        model_policy: options.model_policy,
        execution_limits: options.execution_limits,
        cpu_threads: std::num::NonZeroUsize::MIN,
        consumed_steps: 0,
        current_session: None,
        entry_item: item,
        entry_args: &args,
    }
    .into_parts();
    run_state.shared.safe_points =
        super::safe_point::ExecutionSafePointScheduler::with_ledger(capsule.budget.steps.clone());
    let mut eval = super::EvalContext::new(program, &mut run_state);
    let span = crate::diagnostics::item_span(eval.checked, item);
    let mut machine = EvalMachine::new();
    machine.resume(ControlSignal::pending_call(PendingCall {
        purity: CallPurity::Ordinary,
        site: None,
        target: CallTarget::FlowItem(item),
        args: args.clone(),
        span,
        continuation: Continuation::BlockValue,
    }));
    let value = loop {
        if capsule.cancellation.load(Ordering::Acquire) {
            break Err(CpuExecutionFailure::Cancelled(
                etas_host::execution::CancellationReason::Requested,
            ));
        }
        match machine.run_until_yield(&mut eval) {
            MachinePoll::Complete(value) => {
                break FrozenValue::freeze(&value).ok_or_else(|| {
                    CpuExecutionFailure::Fault("CPU task produced a non-freezable value".to_owned())
                });
            }
            MachinePoll::CooperativeYield | MachinePoll::ProgramChanged => continue,
            MachinePoll::Cancelled(cause) => {
                break Err(CpuExecutionFailure::Cancelled(cause.reason().clone()));
            }
            MachinePoll::Fault(fault) => {
                break Err(CpuExecutionFailure::Fault(fault.message));
            }
            MachinePoll::Yield(boundary) => {
                break Err(CpuExecutionFailure::EffectBoundary(boundary_kind(
                    &boundary,
                )));
            }
        }
    };
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
