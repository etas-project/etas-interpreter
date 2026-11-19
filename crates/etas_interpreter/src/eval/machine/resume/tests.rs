use super::*;
use crate::{
    api::RunOptions,
    control::Continuation,
    eval::{EvalContextInput, machine::frame::EvalFrame},
    testing::allocation::measure,
    value::InterpValue,
};

#[test]
fn retry_gate_does_not_copy_unrelated_payloads_or_stack_frames() {
    let checked = crate::testing::project::checked_project(
        "module app.main; flow main() -> unit { return; }",
    );
    let plan = crate::Interpreter
        .plan(&checked, crate::api::PlanOptions)
        .plan
        .unwrap();
    let options = RunOptions::default();
    let span = crate::diagnostics::item_span(&checked, checked.entry.unwrap());
    let (program, mut run_state) = EvalContextInput {
        replay_keys: crate::eval::replay::RunReplayKeys::start(&crate::api::RunOptions::default())
            .unwrap(),
        storage_limits: Default::default(),
        code_definitions: None,
        syntax_limits: Default::default(),
        event_observer: None,
        execution: etas_host::execution::ExecutionScope::new_owned(),
        checked: &checked,
        plan: &plan,
        host_context: options.host_context,
        model_policy: options.model_policy,
        execution_limits: options.execution_limits,
        cpu_threads: options.cpu_threads,
        runtime: options.runtime.clone(),
        cpu_min_work: options.cpu_min_work,
        consumed_steps: 0,
        current_session: None,
        entry_item: checked.entry.unwrap(),
        entry_args: &[],
    }
    .into_parts();
    let mut eval = EvalContext::new(program, &mut run_state);
    for count in [1000, 2000, 4000] {
        for wide_payload in [true, false] {
            let mut machine = EvalMachine::new();
            if wide_payload {
                machine.push_frame(EvalFrame::from_continuation(
                    Continuation::MemoryClearDeleteNext {
                        region_stable_id: "region".into(),
                        path: vec!["entry".into()],
                        remaining_keys: (0..count)
                            .map(|_| InterpValue::String("x".repeat(1024).into()))
                            .collect(),
                        next_index: 0,
                        span,
                    },
                ));
            } else {
                for _ in 0..count {
                    machine.push_frame(EvalFrame::from_continuation(Continuation::Return));
                }
            }
            let message = "provider failed".to_owned();
            let (result, cost) = measure(|| {
                machine.retry_boundary_failure(&mut eval, Continuation::BlockValue, span, message)
            });
            assert!(result.is_none());
            assert_eq!(machine.frames().len(), if wide_payload { 1 } else { count });
            eprintln!("retry gate payload={wide_payload} n={count}: {cost:?}");
            assert!(
                cost.bytes < 4096 && cost.count < 8,
                "retry gate copied unrelated state: {cost:?}"
            );
        }
    }
}

#[test]
fn boundary_failure_survives_machine_snapshot_and_json_restore() {
    let checked = crate::testing::project::checked_project(
        "module app.main; flow main() -> unit { return; }",
    );
    let plan = crate::Interpreter
        .plan(&checked, crate::api::PlanOptions)
        .plan
        .unwrap();
    let span = crate::diagnostics::item_span(&checked, checked.entry.unwrap());
    let mut machine = EvalMachine::new();
    machine.resume(retry_failure(
        Continuation::BlockValue,
        span,
        "transient failure".into(),
    ));
    let snapshot = machine.snapshot().unwrap();
    let snapshot = crate::api::codec::checkpoint::input::roundtrip_machine(&snapshot, &checked);
    let mut restored = EvalMachine::from_snapshot(
        &snapshot,
        &checked,
        plan.slots.clone(),
        &plan.dispatch,
        &plan.closures,
        &crate::api::HostExecutionContext::default(),
        &etas_host::StorageLimits::default(),
    )
    .unwrap();
    let Some(MachineInput::Signal(ControlSignal::Apply(pending))) = restored.input.take() else {
        panic!("restored failed boundary input")
    };
    assert!(matches!(pending.continuation, Continuation::BlockValue));
    let ContinuationInput::RetryFailure {
        span: restored_span,
        message,
    } = pending.input
    else {
        panic!("restored retry failure")
    };
    assert_eq!(restored_span, span);
    assert_eq!(message, "transient failure");
}
