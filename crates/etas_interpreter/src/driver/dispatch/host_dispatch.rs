use std::future::Future;

use etas_host::{
    ApprovalDecision, BrowserProtocolResponse, CommandResponse, FilesystemResponse, HostError,
    HostErrorCode, HostOutcome, HostRequestId, HostRequestKind, MemoryResponse, ModelResponse,
    PolicyResponse, SecretResponse, SessionResponse, StreamFailure, StreamResponse,
    TcpConnectResponse, TlsConnectResponse, ToolResponse, TraceContext, TraceEvent,
    console::ConsoleResponse,
};

use crate::{eval::EvalContext, orchestration::WorkflowEvent};

pub(super) struct HostDispatch;

impl HostDispatch {
    pub(super) async fn execute<Response, Call>(
        eval: &mut EvalContext<'_>,
        request_id: HostRequestId,
        kind: HostRequestKind,
        authority: etas_host::AuthorityContext,
        trace: TraceContext,
        call: Call,
    ) -> Result<Response, HostError>
    where
        Response: TraceableHostResponse,
        Call: Future<Output = Result<Response, HostError>>,
    {
        eval.events
            .push(WorkflowEvent::HostTrace(TraceEvent::HostRequestStarted {
                id: request_id,
                kind,
                authority: Box::new(authority),
                trace,
            }));
        let result = call.await;
        let (response_id, outcome) = match &result {
            Ok(response) => (response.response_id(request_id), response.outcome()),
            Err(error) => (request_id, HostOutcome::Failed(error.clone())),
        };
        eval.events
            .push(WorkflowEvent::HostTrace(TraceEvent::HostRequestFinished {
                id: response_id,
                outcome,
            }));
        result
    }

    pub(super) async fn execute_approval<Call>(
        eval: &mut EvalContext<'_>,
        request: etas_host::ApprovalRequest,
        authority: etas_host::AuthorityContext,
        call: Call,
    ) -> Result<ApprovalDecision, HostError>
    where
        Call: Future<Output = Result<ApprovalDecision, HostError>>,
    {
        let id = request.id;
        let trace = request.trace.clone();
        eval.events
            .push(WorkflowEvent::HostTrace(TraceEvent::ApprovalRequested {
                request,
            }));
        Self::execute(eval, id, HostRequestKind::Approval, authority, trace, call).await
    }
}

pub(super) trait TraceableHostResponse {
    fn response_id(&self, fallback: HostRequestId) -> HostRequestId;
    fn outcome(&self) -> HostOutcome;
}

macro_rules! result_response {
    ($response:ty) => {
        impl TraceableHostResponse for $response {
            fn response_id(&self, _fallback: HostRequestId) -> HostRequestId {
                self.id
            }

            fn outcome(&self) -> HostOutcome {
                match &self.result {
                    Ok(_) => HostOutcome::Succeeded,
                    Err(error) => HostOutcome::Failed(error.clone()),
                }
            }
        }
    };
}

result_response!(ToolResponse);
result_response!(MemoryResponse);
result_response!(SessionResponse);
result_response!(FilesystemResponse);
result_response!(CommandResponse);
result_response!(TcpConnectResponse);
result_response!(TlsConnectResponse);
result_response!(SecretResponse);
result_response!(BrowserProtocolResponse);

impl TraceableHostResponse for ModelResponse {
    fn response_id(&self, _fallback: HostRequestId) -> HostRequestId {
        self.id
    }

    fn outcome(&self) -> HostOutcome {
        HostOutcome::Succeeded
    }
}

impl TraceableHostResponse for ConsoleResponse {
    fn response_id(&self, _fallback: HostRequestId) -> HostRequestId {
        self.id
    }

    fn outcome(&self) -> HostOutcome {
        HostOutcome::Succeeded
    }
}

impl TraceableHostResponse for PolicyResponse {
    fn response_id(&self, _fallback: HostRequestId) -> HostRequestId {
        self.id
    }

    fn outcome(&self) -> HostOutcome {
        HostOutcome::Succeeded
    }
}

impl TraceableHostResponse for ApprovalDecision {
    fn response_id(&self, fallback: HostRequestId) -> HostRequestId {
        fallback
    }

    fn outcome(&self) -> HostOutcome {
        HostOutcome::Succeeded
    }
}

impl TraceableHostResponse for StreamResponse {
    fn response_id(&self, _fallback: HostRequestId) -> HostRequestId {
        self.id
    }

    fn outcome(&self) -> HostOutcome {
        match &self.result {
            Ok(_) => HostOutcome::Succeeded,
            Err(StreamFailure::Cancelled) => HostOutcome::Cancelled {
                reason: "stream operation was cancelled".to_owned(),
            },
            Err(StreamFailure::TimedOut) => HostOutcome::Failed(HostError::new(
                HostErrorCode::TimedOut,
                "stream operation timed out",
            )),
            Err(StreamFailure::Closed) => {
                HostOutcome::Failed(HostError::new(HostErrorCode::Closed, "stream is closed"))
            }
            Err(StreamFailure::Interrupted) => HostOutcome::Failed(HostError::new(
                HostErrorCode::Interrupted,
                "stream operation was interrupted",
            )),
            Err(StreamFailure::LimitExceeded { limit_bytes }) => HostOutcome::Failed(
                HostError::new(
                    HostErrorCode::InvalidResponse,
                    "stream read exceeded the configured byte limit",
                )
                .with_detail("limit_bytes", limit_bytes.to_string()),
            ),
            Err(StreamFailure::Host(error)) => HostOutcome::Failed(error.clone()),
        }
    }
}
