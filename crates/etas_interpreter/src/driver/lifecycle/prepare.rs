use super::BodyResult;
use crate::{
    api::{EntryPoint, InterpValue, RunFailure, RunOptions, RunOutcome},
    diagnostics::{self, item_span},
    driver, eval,
    host::{self, HostServices},
    orchestration,
};
use etas_frontend::CheckedProject;
use etas_hir::{HirItem, HirItemId};

pub(crate) async fn run_checked_inner(
    project: &CheckedProject,
    entry: EntryPoint,
    args: Vec<InterpValue>,
    host: &dyn HostServices,
    options: RunOptions,
    execution: etas_host::execution::ExecutionScope,
) -> BodyResult {
    let mut diagnostics = Vec::new();
    let mut events = Vec::new();
    let mut checkpoints = Vec::new();
    let profile = options.profile.clone();
    if let Err(message) = options
        .execution_limits
        .validate()
        .and_then(|()| options.storage_limits.validate().map_err(|e| e.message))
    {
        diagnostics.push(diagnostics::invalid_arguments(
            item_span(project, entry.item),
            message,
        ));
        return BodyResult {
            outcome: RunOutcome::Failed(RunFailure::PreparationRejected {
                origin: item_span(project, entry.item),
            }),
            diagnostics,
            events,
            checkpoints,
        };
    }
    let plan_span = profile.span("interpreter.plan", "interpreter");
    let plan_result = crate::Interpreter.plan(project, options.plan);
    plan_span.finish_ok();
    diagnostics.extend(plan_result.diagnostics);
    let Some(plan) = plan_result.plan else {
        return BodyResult {
            outcome: RunOutcome::Failed(RunFailure::PreparationRejected {
                origin: item_span(project, entry.item),
            }),
            diagnostics,
            events,
            checkpoints,
        };
    };
    if plan.entry != entry {
        diagnostics.push(diagnostics::invalid_arguments(
            item_span(project, entry.item),
            "requested interpreter entry does not match the planned checked-project entry",
        ));
        return BodyResult {
            outcome: RunOutcome::Failed(RunFailure::PreparationRejected {
                origin: item_span(project, entry.item),
            }),
            diagnostics,
            events,
            checkpoints,
        };
    }
    if let Some(expected) = entry_arity(project, entry.item)
        && expected != args.len()
    {
        diagnostics.push(diagnostics::invalid_arguments(
            item_span(project, entry.item),
            format!(
                "entry expects {expected} argument(s) but received {}",
                args.len()
            ),
        ));
    }
    diagnostics.extend(host::validate_host_readiness(
        &plan,
        host.availability(),
        item_span(project, entry.item),
    ));
    if diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity == etas_core::Severity::Error)
    {
        return BodyResult {
            outcome: RunOutcome::Failed(RunFailure::PreparationRejected {
                origin: item_span(project, entry.item),
            }),
            diagnostics,
            events,
            checkpoints,
        };
    }
    let prepare_span = profile.span("interpreter.prepare", "interpreter");
    let mut eval = eval::EvalContext::new(eval::EvalContextInput {
        storage_limits: options.storage_limits,
        event_observer: options.event_observer.clone(),
        execution,
        checked: project,
        plan: &plan,
        host_context: options.host_context,
        model_policy: options.model_policy,
        execution_limits: options.execution_limits,
        consumed_steps: 0,
        current_session: options.current_session,
        entry_item: entry.item,
        entry_args: &args,
    });
    prepare_span.finish_ok();
    let signal = eval.execute_entry_signal(entry.item);
    let mut eval_span = profile
        .span("interpreter.eval", "interpreter")
        .unfinished_status(etas_utils::ProfileSpanStatus::Abandoned);
    let outcome = driver::execute_entry(&mut eval, signal, host)
        .await
        .into_outcome();
    eval_span.finish(outcome_profile_status(&outcome));
    diagnostics.extend(eval.diagnostics);
    events.extend(eval.events.into_events());
    checkpoints.append(&mut eval.checkpoints);
    BodyResult {
        outcome,
        diagnostics,
        events,
        checkpoints,
    }
}

pub(crate) async fn resume_checkpoint_inner(
    project: &CheckedProject,
    checkpoint: &orchestration::InterpreterCheckpoint,
    host: &dyn HostServices,
    options: RunOptions,
    execution: etas_host::execution::ExecutionScope,
) -> BodyResult {
    let mut diagnostics = Vec::new();
    let mut events = Vec::new();
    let mut checkpoints = Vec::new();
    let profile = options.profile.clone();
    if let Err(message) = checkpoint
        .compilation
        .validate_for_project(project, checkpoint.entry_item)
    {
        diagnostics.push(diagnostics::invalid_arguments(
            item_span(project, checkpoint.entry_item),
            format!("checkpoint compilation identity mismatch: {message}"),
        ));
        return BodyResult {
            outcome: RunOutcome::Failed(RunFailure::RestoreRejected {
                origin: item_span(project, checkpoint.entry_item),
            }),
            diagnostics,
            events,
            checkpoints,
        };
    }
    if let Err(message) = options
        .execution_limits
        .validate()
        .and_then(|()| options.storage_limits.validate().map_err(|e| e.message))
    {
        diagnostics.push(diagnostics::invalid_arguments(
            item_span(project, checkpoint.entry_item),
            message,
        ));
        return BodyResult {
            outcome: RunOutcome::Failed(RunFailure::RestoreRejected {
                origin: item_span(project, checkpoint.entry_item),
            }),
            diagnostics,
            events,
            checkpoints,
        };
    }
    let plan_span = profile.span("interpreter.plan", "interpreter");
    let plan_result = crate::Interpreter.plan(project, options.plan);
    plan_span.finish_ok();
    diagnostics.extend(plan_result.diagnostics);
    let Some(plan) = plan_result.plan else {
        return BodyResult {
            outcome: RunOutcome::Failed(RunFailure::RestoreRejected {
                origin: item_span(project, checkpoint.entry_item),
            }),
            diagnostics,
            events,
            checkpoints,
        };
    };
    if let Err(message) = eval::machine::snapshot::SnapshotValidator::new(
        project,
        &plan.slots,
        &plan.dispatch,
        &options.storage_limits,
    )
    .validate_checkpoint(checkpoint)
    {
        diagnostics.push(diagnostics::invalid_arguments(
            item_span(project, checkpoint.entry_item),
            format!("checkpoint state validation failed: {message}"),
        ));
        return BodyResult {
            outcome: RunOutcome::Failed(RunFailure::RestoreRejected {
                origin: item_span(project, checkpoint.entry_item),
            }),
            diagnostics,
            events,
            checkpoints,
        };
    }
    diagnostics.extend(host::validate_host_readiness(
        &plan,
        host.availability(),
        item_span(project, checkpoint.entry_item),
    ));
    if diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity == etas_core::Severity::Error)
    {
        return BodyResult {
            outcome: RunOutcome::Failed(RunFailure::RestoreRejected {
                origin: item_span(project, checkpoint.entry_item),
            }),
            diagnostics,
            events,
            checkpoints,
        };
    }
    let execution_limits = checkpoint
        .execution_progress
        .original_limits
        .stricter(options.execution_limits);
    let mut host_context = options.host_context;
    host_context.budget = match checkpoint
        .host_state
        .budget
        .resume_under(&host_context.budget)
    {
        Ok(budget) => budget,
        Err(error) => {
            diagnostics.push(diagnostics::invalid_arguments(
                item_span(project, checkpoint.entry_item),
                format!("checkpoint budget cannot be resumed: {error}"),
            ));
            return BodyResult {
                outcome: RunOutcome::Failed(RunFailure::RestoreRejected {
                    origin: item_span(project, checkpoint.entry_item),
                }),
                diagnostics,
                events,
                checkpoints,
            };
        }
    };
    let parent_trace = checkpoint.host_state.trace.trace_id;
    let resumed_trace = match (|| {
        loop {
            let trace_id = etas_host::TraceId::generate()?;
            if trace_id != parent_trace {
                return Ok::<_, etas_host::HostError>(etas_host::TraceContext::resumed(
                    trace_id,
                    parent_trace,
                ));
            }
        }
    })() {
        Ok(trace) => trace,
        Err(error) => {
            diagnostics.push(diagnostics::invalid_arguments(
                item_span(project, checkpoint.entry_item),
                error.to_string(),
            ));
            return BodyResult {
                outcome: RunOutcome::Failed(RunFailure::RestoreRejected {
                    origin: item_span(project, checkpoint.entry_item),
                }),
                diagnostics,
                events,
                checkpoints,
            };
        }
    };
    host_context.trace = resumed_trace;
    let prepare_span = profile.span("interpreter.prepare", "interpreter");
    let mut eval = eval::EvalContext::new(eval::EvalContextInput {
        storage_limits: options.storage_limits,
        event_observer: options.event_observer.clone(),
        execution,
        checked: project,
        plan: &plan,
        host_context,
        model_policy: options.model_policy,
        execution_limits,
        consumed_steps: checkpoint.execution_progress.consumed_steps,
        current_session: checkpoint
            .current_session
            .clone()
            .or(options.current_session),
        entry_item: checkpoint.entry_item,
        entry_args: &checkpoint.args,
    });
    prepare_span.finish_ok();
    let signal = eval.execute_from_checkpoint_signal(checkpoint);
    let mut eval_span = profile
        .span("interpreter.eval", "interpreter")
        .unfinished_status(etas_utils::ProfileSpanStatus::Abandoned);
    let outcome = driver::execute_entry_from_snapshot(&mut eval, signal, host, &checkpoint.machine)
        .await
        .into_outcome();
    eval_span.finish(outcome_profile_status(&outcome));
    diagnostics.extend(eval.diagnostics);
    events.extend(eval.events.into_events());
    checkpoints.append(&mut eval.checkpoints);
    BodyResult {
        outcome,
        diagnostics,
        events,
        checkpoints,
    }
}
fn outcome_profile_status(outcome: &RunOutcome) -> etas_utils::ProfileSpanStatus {
    match outcome {
        RunOutcome::Completed(_) => etas_utils::ProfileSpanStatus::Ok,
        RunOutcome::Cancelled(_) => etas_utils::ProfileSpanStatus::Cancelled,
        RunOutcome::Failed(_) => etas_utils::ProfileSpanStatus::Error,
    }
}

fn entry_arity(project: &CheckedProject, item: HirItemId) -> Option<usize> {
    match project.hir.items.get(item)? {
        HirItem::Flow(flow) => Some(flow.params.len()),
        HirItem::Agent(agent) => Some(agent.params.len()),
        _ => None,
    }
}
