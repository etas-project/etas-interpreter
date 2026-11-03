use super::*;
use crate::api::{RunFailure, RunOutcome};
use etas_host::execution::CancellationReason;
use std::time::Duration;

pub(super) fn assert_incomplete(result: &api::RunResult, kind: &str) {
    assert!(result.value().is_none());
    assert!(
        result.diagnostics.iter().any(|diagnostic| {
            diagnostic.code
                == DiagnosticCode::Analysis(AnalysisDiagnosticCode::TraceSpecResidualIncomplete)
                && diagnostic.message.contains("residual scope is incomplete")
                && diagnostic.message.contains(kind)
        }),
        "expected an incomplete {kind} scope: {:#?}",
        result.diagnostics
    );
}

fn instance(
    checked: &etas_frontend::CheckedProject,
    log: &std::rc::Rc<crate::control::residual::ResidualRunLog>,
    counter: u64,
) -> crate::control::residual::SharedResidualScope {
    let plan = crate::plan::build_plan(checked, Default::default())
        .plan
        .unwrap();
    let mut identity = [0; 16];
    identity[0] = b's';
    identity[8..].copy_from_slice(&counter.to_be_bytes());
    std::rc::Rc::new(std::cell::RefCell::new(
        crate::control::residual::ResidualScopeInstance::activate(
            plan.residual.scope(checked.entry.unwrap()).unwrap().clone(),
            plan.residual
                .scope(checked.entry.unwrap())
                .unwrap()
                .applications[0]
                .clone(),
            checked.clone(),
            etas_code::residual::monitor::ScopeInvocationId::from_bytes(identity),
            log,
            Default::default(),
        )
        .unwrap(),
    ))
}

#[test]
fn captured_scopes_count_toward_the_active_ceiling_without_mirror_entries() {
    use crate::control::residual::{ResidualExitKind, ResidualRunLog};
    let checked = residual_checked(ORDERED_HANDLED);
    let log = ResidualRunLog::new(8);
    let captured = instance(&checked, &log, 1);
    log.register(&captured, 1).unwrap();
    assert!(log.check_activation_capacity(1).is_err());
    captured
        .borrow_mut()
        .finish(ResidualExitKind::Return)
        .unwrap();
    log.check_activation_capacity(1).unwrap();
    let next = instance(&checked, &log, 2);
    log.register(&next, 1).unwrap();
    assert!(log.check_activation_capacity(1).is_err());
    assert!(log.finish_active(ResidualExitKind::Cancellation).is_empty());
    let exits = log.exits();
    assert_eq!(exits.len(), 2);
    assert_eq!(exits[1].kind, ResidualExitKind::Cancellation);
    assert_eq!(
        exits[1].result,
        etas_code::residual::monitor::ScopeCheckResult::Incomplete
    );
}

#[test]
fn dropping_an_abandoned_scope_is_not_hidden_by_later_normal_exits() {
    use crate::control::residual::{ResidualExitKind, ResidualRunLog};
    let checked = residual_checked(ORDERED_HANDLED);
    let log = ResidualRunLog::new(1);
    let abandoned = instance(&checked, &log, 1);
    log.register(&abandoned, 1).unwrap();
    drop(abandoned);
    log.check_activation_capacity(1).unwrap();
    for counter in 2..=5 {
        let scope = instance(&checked, &log, counter);
        log.register(&scope, 1).unwrap();
        scope.borrow_mut().finish(ResidualExitKind::Return).unwrap();
    }
    assert_eq!(log.exits().len(), 1);
    assert_eq!(log.exits()[0].kind, ResidualExitKind::Return);
    let diagnostics = log.take_diagnostics();
    assert_eq!(diagnostics.len(), 1);
    assert!(diagnostics[0].message.contains("Abort"));
    assert!(
        diagnostics[0]
            .notes
            .iter()
            .any(|note| note.contains("unsatisfied scopes: 1"))
    );
    assert_eq!(
        diagnostics[0].primary.span,
        checked
            .hir
            .items
            .get(checked.entry.unwrap())
            .unwrap()
            .span()
    );
    assert!(log.take_diagnostics().is_empty());
}

#[test]
fn bounded_exit_history_checkpoint_preserves_an_evicted_failure() {
    use crate::control::residual::{ResidualExitKind, ResidualRunLog};
    let checked = residual_checked(ORDERED_HANDLED);
    let plan = crate::plan::build_plan(&checked, Default::default())
        .plan
        .unwrap();
    let log = ResidualRunLog::new(1);
    instance(&checked, &log, 1)
        .borrow_mut()
        .finish(ResidualExitKind::Fault)
        .unwrap();
    instance(&checked, &log, 2)
        .borrow_mut()
        .finish(ResidualExitKind::Return)
        .unwrap();
    let history = log.snapshot(&plan.residual).unwrap();
    history.validate(2).unwrap();
    assert_eq!(history.unsatisfied, 1);
    assert_eq!(history.omitted_exits, 1);
    assert_eq!(history.records.len(), 1);
    assert_eq!(
        history.records[0].exit.kind,
        crate::orchestration::ResidualExitKindSnapshot::Fault
    );
    let restored = ResidualRunLog::new(1);
    restored
        .restore_history(&history, &plan.residual, &checked)
        .unwrap();
    let diagnostics = restored.take_diagnostics();
    assert_eq!(diagnostics.len(), 1);
    assert!(diagnostics[0].message.contains("Fault"));
    let obligation = format!(
        "{:?}",
        plan.residual
            .scope(checked.entry.unwrap())
            .unwrap()
            .obligation_ids()[0]
    );
    assert!(
        diagnostics[0]
            .notes
            .iter()
            .any(|note| note.contains(&obligation)),
        "{diagnostics:#?}"
    );
}

#[test]
fn terminal_unwind_reports_busy_scope_without_panicking_or_forgetting_it() {
    use crate::control::residual::{ResidualExitKind, ResidualRunLog};
    let checked = residual_checked(ORDERED_HANDLED);
    let log = ResidualRunLog::new(8);
    let scope = instance(&checked, &log, 1);
    log.register(&scope, 1).unwrap();
    let borrowed = scope.borrow_mut();
    assert_eq!(log.finish_active(ResidualExitKind::Fault).len(), 1);
    assert!(log.check_activation_capacity(1).is_err());
    drop(borrowed);
    assert!(log.finish_active(ResidualExitKind::Fault).is_empty());
    assert_eq!(log.exits().len(), 1);
    assert_eq!(log.exits()[0].kind, ResidualExitKind::Fault);
}

async fn cancel_cpu(source: &str) -> api::RunResult {
    let checked = residual_checked(source);
    let host = FakeHost::new(availability(&[]));
    let invocation = Interpreter.create_run(
        &checked,
        EntryPoint {
            item: checked.entry.unwrap(),
        },
        vec![],
        &host,
        RunOptions::default(),
    );
    let control = invocation.control();
    let (result, ()) = tokio::time::timeout(Duration::from_secs(3), async {
        tokio::join!(invocation.execute(), async {
            tokio::task::yield_now().await;
            control.stop(CancellationReason::Interrupt).unwrap();
        })
    })
    .await
    .expect("CPU cancellation must settle on the current-thread executor");
    let result = result.unwrap();
    assert!(
        matches!(&result.outcome, RunOutcome::Cancelled(cause) if cause.reason() == &CancellationReason::Interrupt)
    );
    result
}

const LOOP: &str = "while true limit std.runtime.limits.Iterations(1000000) {}";

#[tokio::test(flavor = "current_thread")]
async fn cancelled_cpu_scope_keeps_cancellation_and_reports_incomplete() {
    let source = ORDERED_HANDLED.replace(
        "        perform Gate.send();",
        &format!("        {LOOP}\n        perform Gate.send();"),
    );
    let result = cancel_cpu(&source).await;
    assert_incomplete(&result, "Cancellation");
}

#[tokio::test(flavor = "current_thread")]
async fn cancelled_scope_captured_by_source_handler_is_not_lost() {
    let source = ORDERED_HANDLED.replace(
        "Gate.check() => { resume (); }",
        &format!("Gate.check() => {{ {LOOP} resume (); }}"),
    );
    let result = cancel_cpu(&source).await;
    assert_incomplete(&result, "Cancellation");
    assert!(
        result
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("unknown outcomes: 1")),
        "{:#?}",
        result.diagnostics
    );
}

#[tokio::test(flavor = "current_thread")]
async fn cancelled_host_wait_never_releases_the_guarded_target() {
    let checked = residual_checked(super::outcomes::HOST_GUARD);
    let mut host = FakeHost::new(availability(&[HostRequirementKind::Console]));
    let gate = host.pause_console_completion();
    let invocation = Interpreter.create_run(
        &checked,
        EntryPoint {
            item: checked.entry.unwrap(),
        },
        vec![],
        &host,
        RunOptions::default(),
    );
    let control = invocation.control();
    let (result, ()) = tokio::time::timeout(Duration::from_secs(3), async {
        tokio::join!(invocation.execute(), async {
            gate.started.notified().await;
            control.stop(CancellationReason::Requested).unwrap();
            gate.release.add_permits(1);
        })
    })
    .await
    .expect("Host completion and cancellation must settle");
    let result = result.unwrap();
    assert!(matches!(result.outcome, RunOutcome::Cancelled(_)));
    assert_incomplete(&result, "Cancellation");
    assert_eq!(host.console_call_count(), 1);
    assert_eq!(host.stdout_text(), "guard");
}

#[tokio::test(flavor = "current_thread")]
async fn fuel_exhaustion_preserves_the_primary_fault_and_incomplete_scope() {
    let source = ORDERED_HANDLED.replace(
        "        perform Gate.send();",
        &format!("        {LOOP}\n        perform Gate.send();"),
    );
    let checked = residual_checked(&source);
    let host = FakeHost::new(availability(&[]));
    let mut options = RunOptions::default();
    options.execution_limits.max_steps = std::num::NonZeroU64::new(1000);
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            vec![],
            &host,
            options,
        )
        .await
        .unwrap();
    assert!(
        matches!(&result.outcome, RunOutcome::Failed(RunFailure::ExecutionFault(fault)) if fault.message.contains("step")),
        "{:?}",
        result.outcome
    );
    assert_incomplete(&result, "Fault");
}

#[test]
fn aggregate_component_ceiling_applies_across_all_plans_of_one_scope() {
    use crate::control::residual::{ResidualRunLog, ResidualScopeInstance, ResidualStateLimits};
    use crate::plan::residual::{PreparedResidualScope, PreparedTracePlan};
    let checked = residual_checked(ORDERED_HANDLED);
    let plan = crate::plan::build_plan(&checked, Default::default())
        .plan
        .unwrap();
    let prepared = plan.residual.scope(checked.entry.unwrap()).unwrap();
    let duplicate = || {
        let first = &prepared.plans[0];
        PreparedTracePlan {
            checked: first.checked.clone(),
            scope: first.scope,
            activation: first.activation,
            bound: first.bound,
        }
    };
    let prepared = std::sync::Arc::new(PreparedResidualScope {
        approval_target: None,
        item: prepared.item,
        revision: prepared.revision,
        plans: vec![duplicate(), duplicate()],
        applications: prepared.applications.clone(),
    });
    let limits = ResidualStateLimits {
        max_components: 1,
        ..Default::default()
    };
    let log = ResidualRunLog::new(8);
    let result = ResidualScopeInstance::activate(
        prepared.clone(),
        prepared.applications[0].clone(),
        checked,
        etas_code::residual::monitor::ScopeInvocationId::from_bytes([1; 16]),
        &log,
        limits,
    );
    assert!(result.unwrap_err().contains("aggregate component ceiling"));
    assert!(log.exits().is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn restored_pending_scope_is_explicitly_settled_on_fuel_exhaustion() {
    let checked = residual_checked(&super::checkpoint::checkpoint_source(true));
    let checkpoint = super::checkpoint::first_checkpoint(&checked).await;
    let mut options = RunOptions::default();
    options.execution_limits.max_steps = std::num::NonZeroU64::new(1);
    let result = Interpreter
        .resume_checkpoint(
            &checked,
            &checkpoint,
            &FakeHost::new(availability(&[HostRequirementKind::Checkpoint])),
            options,
        )
        .await
        .unwrap();
    assert!(
        matches!(&result.outcome, RunOutcome::Failed(RunFailure::ExecutionFault(fault)) if fault.message.contains("step")),
        "{:?}",
        result.outcome
    );
    assert_incomplete(&result, "Fault");
    assert!(
        result
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("unknown outcomes: 1")),
        "{:#?}",
        result.diagnostics
    );
}
