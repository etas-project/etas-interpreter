use super::BodyResult;
use crate::{
    api::{EntryPoint, InterpValue, RunFailure, RunOptions, RunOutcome},
    diagnostics::{self, item_span},
    driver, eval,
    host::{self, HostServices},
};
use etas_frontend::CheckedProject;

pub(crate) async fn run_checked_inner(
    project: &CheckedProject,
    entry: EntryPoint,
    args: Vec<InterpValue>,
    prepared: Option<&std::sync::Arc<crate::api::code::PreparedCodeRevision>>,
    host: &dyn HostServices,
    mut options: RunOptions,
    execution: etas_host::execution::ExecutionScope,
) -> BodyResult {
    let mut diagnostics = Vec::new();
    let mut events = Vec::new();
    let mut checkpoints = Vec::new();
    let profile = options.profile.clone();
    if let Err(message) = super::revision::bind_options(project, entry.item, prepared, &mut options)
    {
        diagnostics.push(diagnostics::missing_checked_fact(
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
    if let Err(message) = options
        .execution_limits
        .validate()
        .and_then(|()| options.storage_limits.validate().map_err(|e| e.message))
        .and_then(|()| validate_code_definitions(project, &options))
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
    let plan_result;
    let plan = if let Some(revision) = prepared {
        let plan = revision.plan().as_ref();
        diagnostics.extend(plan.diagnostics.iter().cloned());
        plan
    } else {
        let plan_span = profile.span("interpreter.plan", "interpreter");
        plan_result = crate::Interpreter.plan(project, options.plan);
        plan_span.finish_ok();
        diagnostics.extend(plan_result.diagnostics.iter().cloned());
        let Some(plan) = &plan_result.plan else {
            return BodyResult {
                outcome: RunOutcome::Failed(RunFailure::PreparationRejected {
                    origin: item_span(project, entry.item),
                }),
                diagnostics,
                events,
                checkpoints,
            };
        };
        plan
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
    if let Err(message) = crate::value::contract::runtime::validate_arguments(
        project,
        plan,
        entry.item,
        &args,
        &options.storage_limits,
    ) {
        diagnostics.push(diagnostics::invalid_arguments(
            item_span(project, entry.item),
            message,
        ));
    }
    diagnostics.extend(host::validate_host_readiness(
        plan,
        host.availability(),
        item_span(project, entry.item),
    ));
    if plan.dispatch.reflection_required() && options.code_definitions.is_none() {
        diagnostics.push(diagnostics::missing_checked_fact(
            item_span(project, entry.item),
            "entry-reachable reflection requires prepared canonical definition origins",
        ));
    }
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
    let (program, mut run_state) = eval::EvalContextInput {
        code_definitions: options.code_definitions.clone(),
        syntax_limits: options.syntax_limits,
        storage_limits: options.storage_limits,
        event_observer: options.event_observer.clone(),
        execution,
        checked: project,
        plan,
        host_context: options.host_context,
        model_policy: options.model_policy,
        execution_limits: options.execution_limits,
        consumed_steps: 0,
        current_session: options.current_session,
        entry_item: entry.item,
        entry_args: &args,
    }
    .into_parts();
    let mut eval = eval::EvalContext::new(program, &mut run_state);
    eval.revision = prepared;
    prepare_span.finish_ok();
    let signal = eval.execute_entry_signal(entry.item);
    let mut eval_span = profile
        .span("interpreter.eval", "interpreter")
        .unfinished_status(etas_utils::ProfileSpanStatus::Abandoned);
    let outcome = driver::execute_entry(&mut eval, signal, host)
        .await
        .into_outcome();
    eval_span.finish(outcome_profile_status(&outcome));
    diagnostics.extend(run_state.diagnostics);
    events.extend(run_state.events.into_events());
    checkpoints.append(&mut run_state.checkpoints);
    BodyResult {
        outcome,
        diagnostics,
        events,
        checkpoints,
    }
}

pub(super) fn validate_code_definitions(
    project: &CheckedProject,
    options: &RunOptions,
) -> Result<(), String> {
    if options
        .code_definitions
        .as_ref()
        .is_some_and(|definitions| !definitions.is_for(project))
    {
        return Err("prepared code definitions belong to a different checked program".into());
    }
    Ok(())
}
pub(super) fn outcome_profile_status(outcome: &RunOutcome) -> etas_utils::ProfileSpanStatus {
    match outcome {
        RunOutcome::Completed(_) => etas_utils::ProfileSpanStatus::Ok,
        RunOutcome::Cancelled(_) => etas_utils::ProfileSpanStatus::Cancelled,
        RunOutcome::Failed(_) => etas_utils::ProfileSpanStatus::Error,
    }
}
