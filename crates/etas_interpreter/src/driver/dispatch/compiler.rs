use super::{
    host_dispatch::{HostCompletion, HostDispatch, PendingHostOperation, TraceableHostResponse},
    policy::authorize_before_boundary,
};
use crate::{
    control::{ControlSignal, PendingPerform},
    eval::EvalContext,
    host::HostServices,
};
use etas_host::{
    HostError, HostErrorCode, HostRequestKind, HostTraceRequest, HostValue, PolicySubject,
};

type CompilerCall<'host> = std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<PreparedCompilerReply, HostError>> + 'host>,
>;

pub(in crate::driver) enum CompilerDispatchStart<'host> {
    NotCompiler(Box<PendingPerform>),
    Complete(ControlSignal),
    Pending(Box<PendingCompilerDispatch<'host>>),
}

pub(in crate::driver) struct PendingCompilerDispatch<'host> {
    invocation: crate::eval::CompilerInvocation,
    perform: PendingPerform,
    pending: PendingHostOperation<PreparedCompilerReply, CompilerCall<'host>>,
}

pub(in crate::driver) struct CompletedCompilerDispatch {
    invocation: crate::eval::CompilerInvocation,
    perform: PendingPerform,
    completion: HostCompletion<PreparedCompilerReply>,
}

pub(in crate::driver) async fn begin<'host>(
    eval: &mut EvalContext<'_>,
    host: &'host dyn HostServices,
    perform: PendingPerform,
) -> CompilerDispatchStart<'host> {
    let invocation = match eval.compiler_invocation(&perform) {
        Ok(Some(invocation)) => invocation,
        Ok(None) => return CompilerDispatchStart::NotCompiler(Box::new(perform)),
        Err(fault) => {
            return CompilerDispatchStart::Complete(ControlSignal::Fault(Box::new(fault)));
        }
    };
    if !host.availability().compiler {
        return CompilerDispatchStart::Complete(ControlSignal::fault(
            etas_core::AnalysisDiagnosticCode::MissingHostHandler,
            perform.span,
            "missing Phase 1 host service for Compiler support",
        ));
    }
    match begin_request(
        eval,
        host,
        invocation.request.clone(),
        invocation.definitions.clone(),
    )
    .await
    {
        Ok(pending) => CompilerDispatchStart::Pending(Box::new(PendingCompilerDispatch {
            invocation,
            perform,
            pending,
        })),
        Err(error) => CompilerDispatchStart::Complete(compiler_error(eval, perform, error)),
    }
}

pub(in crate::driver) async fn execute_request(
    eval: &mut EvalContext<'_>,
    host: &dyn HostServices,
    request: etas_host::compiler::CompilerRequest,
    definitions: std::sync::Arc<etas_frontend::code::definition::NativeDefinitionStore>,
) -> Result<PreparedCompilerReply, HostError> {
    let pending = begin_request(eval, host, request, definitions).await?;
    let completion = HostDispatch::poll(pending).await;
    HostDispatch::finish(eval, completion)
}

async fn begin_request<'host>(
    eval: &mut EvalContext<'_>,
    host: &'host dyn HostServices,
    request: etas_host::compiler::CompilerRequest,
    definitions: std::sync::Arc<etas_frontend::code::definition::NativeDefinitionStore>,
) -> Result<PendingHostOperation<PreparedCompilerReply, CompilerCall<'host>>, HostError> {
    if !host.availability().compiler {
        return Err(HostError::new(
            HostErrorCode::ProviderUnavailable,
            "missing host service for Compiler support",
        ));
    }
    let subject = PolicySubject {
        kind: "compiler".into(),
        attributes: vec![
            (
                "operation".into(),
                HostValue::String(request.operation.action().into()),
            ),
            (
                "effect_action".into(),
                HostValue::String(format!("Code.{}", request.operation.action())),
            ),
            (
                "definition".into(),
                HostValue::String(request.definition.identity().fingerprint().to_string()),
            ),
            (
                "revision".into(),
                HostValue::String(
                    request
                        .definition
                        .revision()
                        .key()
                        .fingerprint()
                        .to_string(),
                ),
            ),
        ],
    };
    async {
        request.budget.check_time()?;
        authorize_before_boundary(eval, host, eval.boundary_policy_ref(), subject).await?;
        // Approval may add a scoped grant; only the current invocation authority
        // is used, never authority retained by a definition or compiler worker.
        let mut request = request;
        request.authority = eval.host_authority();
        if !request.authority.allows(&request.action()) {
            return Err(HostError::new(
                HostErrorCode::AuthorityDenied,
                "compiler action is not authorized",
            ));
        }
        let expected = request.clone();
        HostDispatch::begin(
            eval,
            request.id,
            HostRequestKind::Compiler,
            request.trace_payload(),
            request.authority.clone(),
            request.trace.clone(),
            |operation| {
                Box::pin(async move {
                    let response = host.compiler(operation, request).await?;
                    prepare_response(host, &expected, definitions, response)
                }) as CompilerCall<'host>
            },
        )
    }
    .await
}

pub(in crate::driver) fn finish(
    eval: &mut EvalContext<'_>,
    completed: CompletedCompilerDispatch,
) -> ControlSignal {
    let CompletedCompilerDispatch {
        invocation,
        perform,
        completion,
    } = completed;
    match HostDispatch::finish(eval, completion) {
        Ok(response) => eval.resume_compiler_result(
            &invocation,
            perform,
            response.response.result,
            response.prepared,
        ),
        Err(error) => compiler_error(eval, perform, error),
    }
}

fn compiler_error(
    eval: &mut EvalContext<'_>,
    perform: PendingPerform,
    error: HostError,
) -> ControlSignal {
    eval.cancellation_signal(perform.span).unwrap_or_else(|| {
        ControlSignal::runtime_fault(
            format!(
                "compiler boundary failed: {}: {}",
                error.code.as_str(),
                error.message
            ),
            perform.span,
        )
    })
}

impl PendingCompilerDispatch<'_> {
    pub(in crate::driver) fn operation_id(&self) -> etas_host::execution::OperationId {
        self.pending.operation_id()
    }

    pub(in crate::driver) async fn poll(self) -> CompletedCompilerDispatch {
        CompletedCompilerDispatch {
            invocation: self.invocation,
            perform: self.perform,
            completion: HostDispatch::poll(self.pending).await,
        }
    }
}

fn prepare_response(
    host: &dyn HostServices,
    expected: &etas_host::compiler::CompilerRequest,
    definitions: std::sync::Arc<etas_frontend::code::definition::NativeDefinitionStore>,
    response: etas_host::compiler::CompilerResponse,
) -> Result<PreparedCompilerReply, HostError> {
    use crate::api::code::CheckedReferencePublicationError;
    use etas_frontend::code::service::CandidateResolutionError;
    etas_host::compiler::validation::validate_compiler_response(
        expected,
        &response,
        Default::default(),
    )?;
    let prepared =
        if let Ok(etas_host::compiler::CompilerResult::Compiled(reference)) = &response.result {
            expected.budget.check_time()?;
            let resolver = host.checked_candidate_resolver().ok_or_else(|| {
                HostError::new(
                    HostErrorCode::InvalidResponse,
                    "compiler returned executable code without a native checked candidate resolver",
                )
            })?;
            let prepared = crate::api::code::InterpreterCandidatePublisher
                .publish_checked_reference(resolver, reference, definitions, &expected.definition)
                .map_err(|error| match error {
                    CheckedReferencePublicationError::Resolution(
                        CandidateResolutionError::Unavailable(reason),
                    ) => HostError::new(HostErrorCode::ProviderUnavailable, reason),
                    error => HostError::new(HostErrorCode::InvalidResponse, error.to_string()),
                })?;
            expected.budget.check_time()?;
            Some(prepared)
        } else {
            None
        };
    Ok(PreparedCompilerReply { response, prepared })
}

pub(in crate::driver) struct PreparedCompilerReply {
    pub response: etas_host::compiler::CompilerResponse,
    pub prepared: Option<std::sync::Arc<crate::api::code::PreparedCodeRevision>>,
}
impl etas_host::execution::OperationResponse for PreparedCompilerReply {
    fn external_outcome(&self) -> etas_host::execution::ExternalOutcome {
        self.response.external_outcome()
    }
}
impl TraceableHostResponse for PreparedCompilerReply {
    fn response_id(&self) -> etas_host::HostRequestId {
        self.response.id
    }
    fn outcome(&self) -> etas_host::HostOutcome {
        self.response.outcome()
    }
}
