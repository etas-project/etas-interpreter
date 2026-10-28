mod source;

use super::{
    BodyResult,
    prepare::{outcome_profile_status, validate_code_definitions},
};
use crate::{
    api::{
        RunFailure, RunOptions, RunOutcome,
        codec::{ResumeHeader, SourceCheckpoint},
    },
    diagnostics, driver, eval,
    host::{self, HostServices},
    orchestration::InterpreterCheckpoint,
};
use etas_frontend::CheckedProject;
use std::sync::Arc;

pub(crate) async fn resume_checkpoint_inner(
    project: &CheckedProject,
    checkpoint: &InterpreterCheckpoint,
    prepared: Option<&Arc<crate::api::code::PreparedAgentRevision>>,
    host: &dyn HostServices,
    options: RunOptions,
    execution: etas_host::execution::ExecutionScope,
) -> BodyResult {
    resume(
        project, checkpoint, prepared, host, options, execution, None,
    )
    .await
}

pub(crate) use source::resume_source_inner;

pub(super) struct ResumePrelude {
    pub state: eval::EvalRunState,
    pub plan: Arc<crate::plan::InterpreterPlan>,
}

pub(super) fn resume_host_context(
    header: &ResumeHeader,
    options: &RunOptions,
) -> Result<crate::api::HostExecutionContext, String> {
    let mut host = options.host_context.clone();
    host.budget = header
        .host
        .budget
        .resume_under(&host.budget)
        .map_err(|error| error.to_string())?;
    loop {
        let id = etas_host::TraceId::generate().map_err(|error| error.to_string())?;
        if id != header.host.trace.trace_id {
            host.trace = etas_host::TraceContext::resumed(id, header.host.trace.trace_id);
            return Ok(host);
        }
    }
}

pub(super) fn header(checkpoint: &InterpreterCheckpoint) -> ResumeHeader {
    ResumeHeader {
        entry: checkpoint.entry_item,
        host: checkpoint.host_state.clone(),
        progress: checkpoint.execution_progress,
        trace: checkpoint.trace.clone(),
        session: checkpoint.current_session.clone(),
    }
}

pub(super) async fn resume(
    project: &CheckedProject,
    checkpoint: &InterpreterCheckpoint,
    prepared: Option<&Arc<crate::api::code::PreparedAgentRevision>>,
    host: &dyn HostServices,
    mut options: RunOptions,
    execution: etas_host::execution::ExecutionScope,
    prelude: Option<ResumePrelude>,
) -> BodyResult {
    let (mut state, restored_plan) = match prelude {
        Some(prelude) => (Some(prelude.state), Some(prelude.plan)),
        None => (None, None),
    };
    let mut diagnostics = Vec::new();
    let origin = diagnostics::item_span(project, checkpoint.entry_item);
    let profile = options.profile.clone();
    // Retain prelude state on every error path too: denied compilation and
    // invalid restored snapshots must not erase already observed Host events.
    let result: Result<RunOutcome, String> = async {
        super::revision::validate_checkpoint(project, checkpoint, prepared)?;
        super::revision::bind_options(project, checkpoint.entry_item, prepared, &mut options)?;
        checkpoint
            .compilation
            .validate_for_project(project, checkpoint.entry_item)
            .map_err(|message| format!("checkpoint compilation identity mismatch: {message}"))?;
        options.execution_limits.validate()?;
        options
            .storage_limits
            .validate()
            .map_err(|error| error.message)?;
        validate_code_definitions(project, &options)?;
        let planned;
        let plan = if let Some(revision) = prepared {
            diagnostics.extend(revision.plan().diagnostics.iter().cloned());
            revision.plan().as_ref()
        } else if let Some(plan) = restored_plan.as_deref() {
            plan
        } else {
            let span = profile.span("interpreter.plan", "interpreter");
            planned = crate::Interpreter.plan(project, options.plan.clone());
            span.finish_ok();
            diagnostics.extend(planned.diagnostics.iter().cloned());
            planned
                .plan
                .as_ref()
                .ok_or("checkpoint has no valid checked execution plan")?
        };
        eval::machine::snapshot::SnapshotValidator::new(
            project,
            &plan.slots,
            &plan.dispatch,
            &plan.closures,
            &options.storage_limits,
        )
        .with_revisions(options.checkpoint_revisions.as_deref())
        .validate_checkpoint(checkpoint)
        .map_err(|message| format!("checkpoint state validation failed: {message}"))?;
        diagnostics.extend(host::validate_host_readiness(
            plan,
            host.availability(),
            origin,
        ));
        if diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == etas_core::Severity::Error)
        {
            return Err("checkpoint execution plan or Host readiness was rejected".into());
        }
        if plan.dispatch.reflection_required() && options.code_definitions.is_none() {
            return Err(
                "resumed entry-reachable reflection requires prepared canonical definition origins"
                    .into(),
            );
        }
        let ordering = crate::eval::machine::snapshot::ordering::OrderedRestoreChecks::default();
        let args = super::revision::restore_arguments(
            crate::value::CheckedExecutionProgram {
                checked: project.clone(),
                plan: Arc::new(plan.clone()),
                definitions: options.code_definitions.clone(),
                revision: prepared.cloned(),
            },
            checkpoint,
            options.checkpoint_revisions.clone(),
            ordering.clone(),
        )?;
        let span = profile.span("interpreter.prepare", "interpreter");
        if state.is_none() {
            let header = header(checkpoint);
            let host_context = resume_host_context(&header, &options)?;
            state = Some(
                eval::EvalContextInput {
                    code_definitions: options.code_definitions.clone(),
                    syntax_limits: options.syntax_limits,
                    storage_limits: options.storage_limits,
                    event_observer: options.event_observer.clone(),
                    execution,
                    checked: project,
                    plan,
                    host_context,
                    model_policy: options.model_policy.clone(),
                    execution_limits: header
                        .progress
                        .original_limits
                        .stricter(options.execution_limits),
                    consumed_steps: header.progress.consumed_steps,
                    current_session: header.session.or(options.current_session.clone()),
                    entry_item: checkpoint.entry_item,
                    entry_args: &args,
                }
                .into_parts()
                .1,
            );
        }
        let runtime = state.as_mut().ok_or("resume lost its run-owned state")?;
        let mut eval = eval::EvalContext::new(
            eval::EvalProgramInput {
                code_definitions: options.code_definitions.clone(),
                syntax_limits: options.syntax_limits,
                checked: project,
                plan,
                entry_item: checkpoint.entry_item,
                entry_args: &args,
            },
            runtime,
        );
        eval.revision = prepared;
        span.finish_ok();
        let mut span = profile
            .span("interpreter.eval", "interpreter")
            .unfinished_status(etas_utils::ProfileSpanStatus::Abandoned);
        let outcome = driver::execute_entry_from_snapshot(
            &mut eval,
            host,
            checkpoint,
            options.checkpoint_revisions,
            ordering,
        )
        .await
        .into_outcome();
        span.finish(outcome_profile_status(&outcome));
        Ok(outcome)
    }
    .await;
    let outcome = match result {
        Ok(outcome) => outcome,
        Err(message) => {
            diagnostics.push(diagnostics::invalid_arguments(origin, message));
            RunOutcome::Failed(RunFailure::RestoreRejected { origin })
        }
    };
    finish(state, outcome, diagnostics)
}

pub(super) fn finish(
    state: Option<eval::EvalRunState>,
    outcome: RunOutcome,
    mut diagnostics: Vec<etas_core::Diagnostic>,
) -> BodyResult {
    let (events, checkpoints) = if let Some(state) = state {
        diagnostics.extend(state.diagnostics);
        (state.events.into_events(), state.checkpoints)
    } else {
        (vec![], vec![])
    };
    BodyResult {
        outcome,
        diagnostics,
        events,
        checkpoints,
    }
}
