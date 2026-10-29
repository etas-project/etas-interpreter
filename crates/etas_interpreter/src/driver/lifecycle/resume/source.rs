use super::*;
use crate::api::code::{PreparedRevisionSet, SavedCodeRevision};
use etas_code::definition::{DefinitionRef, SourceBundleLimits};
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

struct RestoredDraft(DefinitionRef);
impl OperationResponse for RestoredDraft {
    fn external_outcome(&self) -> ExternalOutcome {
        ExternalOutcome::Confirmed
    }
}

async fn restore_draft(
    eval: &mut eval::EvalContext<'_>,
    definitions: Arc<NativeDefinitionStore>,
    saved: SavedCodeRevision,
    origin: etas_core::Span,
) -> Result<DefinitionRef, HostError> {
    let control = eval
        .code_syntax_control(origin)
        .map_err(|error| HostError::new(HostErrorCode::InvalidRequest, error.message))?;
    let budget = eval.host_budget();
    let registration = eval.execution.register(None, None, eval.host_trace())?;
    registration.begin_dispatch()?;
    let result = registration
        .context()
        .run_blocking(budget, move |_| {
            let expected = checked_source_view(&definitions, &saved, &control)?;
            definitions
                .restore_source_draft_controlled(
                    &expected,
                    &saved.source,
                    SourceBundleLimits::default(),
                    &control,
                )
                .map(RestoredDraft)
                .map_err(source_restore_error)
        })
        .await;
    registration.complete(ExternalOutcome::Confirmed, vec![])?;
    result.map(|draft| draft.0)
}

fn checked_source_view(
    definitions: &NativeDefinitionStore,
    saved: &SavedCodeRevision,
    control: &dyn etas_utils::PipelineExecutionControl,
) -> Result<DefinitionRef, HostError> {
    let checkpoint = || control.checkpoint().map_err(interrupted);
    checkpoint()?;
    let catalog = definitions.catalog();
    let checked = catalog.checked_program();
    #[derive(PartialEq, Eq, PartialOrd, Ord)]
    enum SourceView {
        Agent(etas_hir::HirExprId, Option<etas_hir::HirItemId>),
        Flow(etas_hir::HirExprId, Option<etas_hir::HirItemId>),
    }
    let mut views = Vec::new();
    for (expr, fact) in &checked.types.reflections {
        checkpoint()?;
        match fact.target {
            etas_types::CheckedReflectionTarget::Agent { .. } => {
                let header = catalog
                    .static_reflection(*expr)
                    .map_err(|error| invalid(error.to_string()))?;
                if matches!(header, StaticReflectionView::Agent(header) if header.identity == saved.source.target)
                {
                    views.push(SourceView::Agent(*expr, None));
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
                        views.push(SourceView::Agent(*expr, Some(*item)));
                    }
                }
            }
            etas_types::CheckedReflectionTarget::Callable { .. }
            | etas_types::CheckedReflectionTarget::ExternalCallable { .. }
                if saved.source.target.kind() == etas_code::DefinitionKind::Flow =>
            {
                let view = catalog
                    .static_reflection(*expr)
                    .map_err(|error| invalid(error.to_string()))?;
                if matches!(view, StaticReflectionView::Callable(header)
                    if header.id == saved.source.revision.definition
                        && header.id.as_declaration() == Some(&saved.source.target))
                {
                    views.push(SourceView::Flow(*expr, None));
                }
            }
            etas_types::CheckedReflectionTarget::CallableValue { .. }
                if saved.source.target.kind() == etas_code::DefinitionKind::Flow =>
            {
                if let Some(item) = catalog.item_owner(&saved.source.revision.definition) {
                    match catalog.compatible_callable_value_reflection(*expr, item) {
                        Ok(Some(header)) if header.id.as_declaration() == Some(&saved.source.target) => {
                            views.push(SourceView::Flow(*expr, Some(item)));
                        }
                        Ok(_) | Err(etas_frontend::code::descriptor::ProjectionError::InaccessibleDefinition) => {}
                        Err(error) => return Err(invalid(error.to_string())),
                    }
                }
            }
            _ => {}
        }
    }
    views.sort();
    for view in views {
        checkpoint()?;
        let reflected = match view {
            SourceView::Agent(expr, Some(item)) => definitions.reflect_agent_value(expr, item),
            SourceView::Agent(expr, None) => definitions.reflect_agent(expr),
            SourceView::Flow(expr, Some(item)) => definitions.reflect_flow_value(expr, item),
            SourceView::Flow(expr, None) => definitions.reflect_flow(expr),
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
    if saved.source.target.kind() == etas_code::DefinitionKind::Flow
        && let Some(item) = catalog.item_owner(&saved.source.revision.definition)
    {
        let applications = checked
            .checked_callable_applications_controlled(item, control)
            .map_err(|error| match error {
                etas_frontend::CallableApplicationError::Interrupted(interruption) => {
                    interrupted(interruption)
                }
                error => invalid(error.to_string()),
            })?;
        let mut expressions = checked
            .types
            .reflections
            .iter()
            .filter_map(|(expr, fact)| {
                matches!(
                    fact.target,
                    etas_types::CheckedReflectionTarget::CallableValue { .. }
                )
                .then_some(*expr)
            })
            .collect::<Vec<_>>();
        expressions.sort();
        for expr in expressions {
            for application in &applications {
                checkpoint()?;
                let reflected = match definitions.reflect_callable_application(expr, application, Default::default()) {
                    Ok(reflected) => reflected,
                    Err(etas_frontend::code::definition::NativeDefinitionError::CandidateInputMismatch) => continue,
                    Err(error) => return Err(invalid(error.to_string())),
                };
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
        }
    }
    Err(invalid(
        "saved source has no matching current checked reflection view",
    ))
}

fn invalid(message: impl Into<String>) -> HostError {
    HostError::new(HostErrorCode::InvalidRequest, message)
}

fn source_restore_error(
    error: etas_frontend::code::definition::NativeDefinitionError,
) -> HostError {
    use etas_code::{definition::DefinitionOperationError, syntax::SyntaxOperationError};
    use etas_frontend::code::{definition::NativeDefinitionError, descriptor::ProjectionError};
    match error {
        NativeDefinitionError::Operation(DefinitionOperationError::Syntax(
            SyntaxOperationError::Interrupted(interruption),
        ))
        | NativeDefinitionError::Projection(ProjectionError::Syntax(
            SyntaxOperationError::Interrupted(interruption),
        )) => interrupted(interruption),
        error => invalid(error.to_string()),
    }
}

fn interrupted(interruption: etas_utils::PipelineInterruption) -> HostError {
    let code = match interruption.kind {
        etas_utils::PipelineInterruptionKind::Cancelled => HostErrorCode::Cancelled,
        etas_utils::PipelineInterruptionKind::ResourceLimit => HostErrorCode::BudgetExceeded,
        _ => HostErrorCode::ProviderUnavailable,
    };
    HostError::new(code, interruption.reason)
}

#[cfg(test)]
mod tests {
    use super::*;
    use etas_code::{definition::DefinitionOperationError, syntax::SyntaxOperationError};
    use etas_frontend::code::{definition::NativeDefinitionError, descriptor::ProjectionError};
    use etas_utils::{PipelineInterruption, PipelineInterruptionKind};

    #[test]
    fn restore_preserves_interruption_kind_from_operations_and_projections() {
        for (kind, expected) in [
            (
                PipelineInterruptionKind::Cancelled,
                HostErrorCode::Cancelled,
            ),
            (
                PipelineInterruptionKind::ResourceLimit,
                HostErrorCode::BudgetExceeded,
            ),
            (
                PipelineInterruptionKind::Unavailable,
                HostErrorCode::ProviderUnavailable,
            ),
        ] {
            for projection in [false, true] {
                let interruption = PipelineInterruption {
                    kind,
                    reason: "stopped".into(),
                };
                let syntax = SyntaxOperationError::Interrupted(interruption);
                let error = if projection {
                    NativeDefinitionError::Projection(ProjectionError::Syntax(syntax))
                } else {
                    NativeDefinitionError::Operation(DefinitionOperationError::Syntax(syntax))
                };
                let actual = source_restore_error(error);
                assert_eq!(actual.code, expected);
                assert_eq!(actual.message, "stopped");
            }
        }
        assert_eq!(
            source_restore_error(NativeDefinitionError::MissingBodySource).code,
            HostErrorCode::InvalidRequest
        );
    }
}
