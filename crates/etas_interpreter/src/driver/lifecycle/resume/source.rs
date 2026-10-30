use super::*;
use crate::api::code::{PreparedRevisionSet, SavedAgentRevision};
use etas_code::definition::{AgentDefinitionRef, SourceBundleLimits};
use etas_frontend::code::definition::{NativeDefinitionStore, StaticReflectionView};
use etas_host::{
    HostError, HostErrorCode,
    compiler::{CompilerOperation, CompilerRequest, CompilerResult},
    execution::{ExternalOutcome, OperationResponse},
};

pub(crate) async fn resume_source_inner(
    project: &CheckedProject,
    checkpoint: &SourceCheckpoint,
    host: &dyn HostServices,
    mut options: RunOptions,
    execution: etas_host::execution::ExecutionScope,
) -> BodyResult {
    let origin = diagnostics::item_span(project, checkpoint.header.entry);
    let mut state = None;
    let restored: Result<(InterpreterCheckpoint, ResumePrelude), String> = async {
        checkpoint.validate_project(project)?;
        options.execution_limits.validate()?;
        options
            .storage_limits
            .validate()
            .map_err(|error| error.message)?;
        validate_code_definitions(project, &options)?;
        let replay_keys =
            eval::replay::RunReplayKeys::restore(&options, checkpoint.header.replay_key_id)?;
        let planned = crate::Interpreter.plan(project, options.plan.clone());
        let plan = Arc::new(planned.plan.ok_or_else(|| {
            format!(
                "source checkpoint has no checked execution plan: {:?}",
                planned.diagnostics
            )
        })?);
        if plan.entry.item != checkpoint.header.entry {
            return Err("checkpoint source entry differs from the current checked entry".into());
        }
        let host_context = resume_host_context(&checkpoint.header, &options)?;
        let (program, runtime) = eval::EvalContextInput {
            replay_keys,
            code_definitions: options.code_definitions.clone(),
            syntax_limits: options.syntax_limits,
            storage_limits: options.storage_limits.clone(),
            event_observer: options.event_observer.clone(),
            execution: execution.clone(),
            checked: project,
            plan: &plan,
            host_context,
            model_policy: options.model_policy.clone(),
            execution_limits: checkpoint
                .header
                .progress
                .original_limits
                .stricter(options.execution_limits),
            consumed_steps: checkpoint.header.progress.consumed_steps,
            current_session: checkpoint
                .header
                .session
                .clone()
                .or(options.current_session.clone()),
            entry_item: checkpoint.header.entry,
            entry_args: &[],
        }
        .into_parts();
        state = Some(runtime);
        let runtime = state
            .as_mut()
            .ok_or("source resume lost its run-owned state")?;
        runtime.diagnostics.extend(planned.diagnostics);
        let mut eval = eval::EvalContext::new(program, runtime);
        eval.seed_resume_counters(&checkpoint.header.trace)?;
        if !checkpoint.sources.is_empty() {
            let definitions = options
                .code_definitions
                .as_ref()
                .ok_or("source restoration requires current canonical definition origins")?
                .definitions()
                .clone();
            let mut revisions = PreparedRevisionSet::new(project.clone());
            for saved in &checkpoint.sources {
                let draft = restore_draft(&mut eval, definitions.clone(), saved.clone(), origin)
                    .await
                    .map_err(|error| error.to_string())?;
                let request = CompilerRequest {
                    id: eval.next_host_request_id(),
                    operation: CompilerOperation::Compile,
                    definition: draft,
                    authority: eval.host_authority(),
                    trace: eval.host_trace(),
                    budget: eval.host_budget(),
                };
                let reply = crate::driver::dispatch::compiler::execute_request(
                    &mut eval,
                    host,
                    request,
                    definitions.clone(),
                )
                .await
                .map_err(|error| error.to_string())?;
                match reply.response.result {
                    Ok(CompilerResult::Compiled(_)) => {}
                    other => {
                        return Err(format!(
                            "checkpoint source recompilation did not produce native code: {other:?}"
                        ));
                    }
                }
                let revision = reply
                    .prepared
                    .ok_or("checkpoint source compilation has no checked native owner")?;
                revision
                    .validate_restored_source(saved, SourceBundleLimits::default())
                    .map_err(|error| error.to_string())?;
                revisions.insert(revision)?;
            }
            options.checkpoint_revisions = Some(Arc::new(revisions));
        }
        let checkpoint = checkpoint
            .decode(
                project,
                &options.storage_limits,
                options.checkpoint_revisions.as_ref(),
            )
            .map_err(|error| error.to_string())?;
        let state = state
            .take()
            .ok_or("source resume lost its completed prelude state")?;
        Ok((checkpoint, ResumePrelude { state, plan }))
    }
    .await;
    match restored {
        Ok((checkpoint, prelude)) => {
            resume(
                project,
                &checkpoint,
                None,
                host,
                options,
                execution,
                Some(prelude),
            )
            .await
        }
        Err(message) => {
            let mut diagnostics = vec![diagnostics::invalid_arguments(origin, message)];
            let outcome = match execution.signal().and_then(|signal| signal.cause()) {
                Ok(Some(cause)) => RunOutcome::Cancelled(cause),
                Ok(None) => RunOutcome::Failed(RunFailure::RestoreRejected { origin }),
                Err(error) => {
                    diagnostics.push(diagnostics::missing_checked_fact(origin, error.to_string()));
                    RunOutcome::Failed(RunFailure::RestoreRejected { origin })
                }
            };
            finish(state, outcome, diagnostics)
        }
    }
}

struct RestoredDraft(AgentDefinitionRef);
impl OperationResponse for RestoredDraft {
    fn external_outcome(&self) -> ExternalOutcome {
        ExternalOutcome::Confirmed
    }
}

async fn restore_draft(
    eval: &mut eval::EvalContext<'_>,
    definitions: Arc<NativeDefinitionStore>,
    saved: SavedAgentRevision,
    origin: etas_core::Span,
) -> Result<AgentDefinitionRef, HostError> {
    let control = eval
        .code_syntax_control(origin)
        .map_err(|error| HostError::new(HostErrorCode::InvalidRequest, error.message))?;
    let budget = eval.host_budget();
    let registration = eval.execution.register(None, None, eval.host_trace())?;
    registration.begin_dispatch()?;
    let result = registration
        .context()
        .run_blocking(budget, move |_| {
            let expected = checked_source_view(&definitions, &saved)?;
            definitions
                .restore_source_draft_controlled(
                    &expected,
                    &saved.source,
                    SourceBundleLimits::default(),
                    &control,
                )
                .map(RestoredDraft)
                .map_err(|error| HostError::new(HostErrorCode::InvalidRequest, error.to_string()))
        })
        .await;
    registration.complete(ExternalOutcome::Confirmed, vec![])?;
    result.map(|draft| draft.0)
}

fn checked_source_view(
    definitions: &NativeDefinitionStore,
    saved: &SavedAgentRevision,
) -> Result<AgentDefinitionRef, HostError> {
    let catalog = definitions.catalog();
    let checked = catalog.checked_program();
    let mut views = Vec::new();
    for (expr, fact) in &checked.types.reflections {
        match fact.target {
            etas_types::CheckedReflectionTarget::Agent { .. } => {
                let header = catalog
                    .static_reflection(*expr)
                    .map_err(|error| invalid(error.to_string()))?;
                if matches!(header, StaticReflectionView::Agent(header) if header.identity == saved.source.target)
                {
                    views.push((*expr, None));
                }
            }
            etas_types::CheckedReflectionTarget::AgentValue {
                component,
                method: None,
                ..
            } if catalog.agent_component(&saved.source.target) == Some(component) => {
                for (item, signature) in &checked.types.item_signatures {
                    if matches!(signature, etas_types::ItemSignature::Agent(signature) if signature.component == component)
                        && catalog.native_agent_default(*item)
                            == Some(&saved.source.revision.definition)
                    {
                        views.push((*expr, Some(*item)));
                    }
                }
            }
            _ => {}
        }
    }
    views.sort();
    for (expr, item) in views {
        let reflected = match item {
            Some(item) => definitions.reflect_agent_value(expr, item),
            None => definitions.reflect_agent(expr),
        }
        .map_err(|error| invalid(error.to_string()))?;
        let view = definitions
            .resolve(&reflected)
            .map_err(|error| invalid(error.to_string()))?;
        if view.identity() == &saved.source.target
            && view.revision().inputs().environment == saved.source.revision.environment
            && view.revision().inputs().contract == saved.source.revision.contract
            && view.revision().inputs().compilation == saved.source.revision.compilation
        {
            return Ok(reflected);
        }
    }
    Err(invalid(
        "saved source has no matching current checked reflection view",
    ))
}

fn invalid(message: impl Into<String>) -> HostError {
    HostError::new(HostErrorCode::InvalidRequest, message)
}
