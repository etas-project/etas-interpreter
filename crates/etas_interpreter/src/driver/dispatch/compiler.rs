use super::{
    host_dispatch::{HostDispatch, TraceableHostResponse},
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

pub(in crate::driver) async fn dispatch(
    eval: &mut EvalContext<'_>,
    host: &dyn HostServices,
    perform: PendingPerform,
) -> Result<ControlSignal, Box<PendingPerform>> {
    let invocation = match eval.compiler_invocation(&perform) {
        Ok(Some(invocation)) => invocation,
        Ok(None) => return Err(Box::new(perform)),
        Err(fault) => return Ok(ControlSignal::Fault(Box::new(fault))),
    };
    if !host.availability().compiler {
        return Ok(ControlSignal::fault(
            etas_core::AnalysisDiagnosticCode::MissingHostHandler,
            perform.span,
            "missing Phase 1 host service for Compiler support",
        ));
    }
    let dispatched = execute_request(
        eval,
        host,
        invocation.request.clone(),
        invocation.definitions.clone(),
    )
    .await;
    Ok(match dispatched {
        Ok(response) => eval.resume_compiler_result(
            &invocation,
            perform,
            response.response.result,
            response.prepared,
        ),
        Err(error) => eval.cancellation_signal(perform.span).unwrap_or_else(|| {
            ControlSignal::runtime_fault(
                format!(
                    "compiler boundary failed: {}: {}",
                    error.code.as_str(),
                    error.message
                ),
                perform.span,
            )
        }),
    })
}

pub(in crate::driver) async fn execute_request(
    eval: &mut EvalContext<'_>,
    host: &dyn HostServices,
    request: etas_host::compiler::CompilerRequest,
    definitions: std::sync::Arc<etas_frontend::code::definition::NativeDefinitionStore>,
) -> Result<PreparedCompilerReply, HostError> {
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
        HostDispatch::execute(
            eval,
            request.id,
            HostRequestKind::Compiler,
            request.trace_payload(),
            request.authority.clone(),
            request.trace.clone(),
            |operation| async move {
                let response = host.compiler(operation, request).await?;
                prepare_response(host, &expected, definitions, response)
            },
        )
        .await
    }
    .await
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
    pub prepared: Option<std::sync::Arc<crate::api::code::PreparedAgentRevision>>,
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
