use std::{future::Future, time::Instant};

use etas_host::{
    ApprovalResponse, BrowserProtocolResponse, CommandResponse, FilesystemResponse, HostError,
    HostErrorCode, HostOutcome, HostRequestId, HostRequestKind, HostTraceMetadata,
    HostTracePayload, HostTraceRequest, MemoryResponse, ModelResponse, PolicyResponse,
    SecretResponse, SessionResponse, StreamFailure, StreamResponse, TcpConnectResponse,
    TlsConnectResponse, ToolResponse, TraceContext, TraceEvent, console::ConsoleResponse,
};

use crate::{eval::EvalContext, orchestration::WorkflowEvent};

pub(super) struct HostDispatch;

impl HostDispatch {
    pub(super) async fn execute<Response, Call>(
        eval: &mut EvalContext<'_>,
        request_id: HostRequestId,
        kind: HostRequestKind,
        trace_payload: HostTracePayload,
        authority: etas_host::AuthorityContext,
        trace: TraceContext,
        call: Call,
    ) -> Result<Response, HostError>
    where
        Response: TraceableHostResponse,
        Call: Future<Output = Result<Response, HostError>>,
    {
        let metadata =
            HostTraceMetadata::from_payload(&trace_payload, eval.host_trace_digest_key()?)?;
        let started_at_unix_micros = unix_timestamp_micros()?;
        let started = Instant::now();
        eval.events
            .push(WorkflowEvent::HostTrace(TraceEvent::HostRequestStarted {
                id: request_id,
                kind,
                metadata,
                authority: Box::new(authority),
                trace,
                started_at_unix_micros,
            }));
        let result = match call.await {
            Ok(response) => {
                let response_id = response.response_id();
                if response_id != request_id {
                    Err(HostError::new(
                        HostErrorCode::InvalidResponse,
                        "host response id does not match the originating request id",
                    )
                    .with_detail("request_id", request_id.0.to_string())
                    .with_detail("response_id", response_id.0.to_string()))
                } else {
                    Ok(response)
                }
            }
            Err(error) => Err(error),
        };
        let finished_at_unix_micros = unix_timestamp_micros()?;
        let duration_micros = u64::try_from(started.elapsed().as_micros()).map_err(|_| {
            HostError::new(
                HostErrorCode::InvalidResponse,
                "host request duration exceeded trace ABI range",
            )
        })?;
        let outcome = match &result {
            Ok(response) => response.outcome(),
            Err(error) => HostOutcome::Failed(error.clone()),
        };
        eval.events
            .push(WorkflowEvent::HostTrace(TraceEvent::HostRequestFinished {
                id: request_id,
                outcome,
                finished_at_unix_micros,
                duration_micros,
            }));
        result
    }

    pub(super) async fn execute_approval<Call>(
        eval: &mut EvalContext<'_>,
        request: etas_host::ApprovalRequest,
        authority: etas_host::AuthorityContext,
        call: Call,
    ) -> Result<ApprovalResponse, HostError>
    where
        Call: Future<Output = Result<ApprovalResponse, HostError>>,
    {
        let id = request.id;
        let trace = request.trace.clone();
        let trace_payload = request.trace_payload();
        let metadata =
            HostTraceMetadata::from_payload(&trace_payload, eval.host_trace_digest_key()?)?;
        eval.events
            .push(WorkflowEvent::HostTrace(TraceEvent::ApprovalRequested {
                id,
                metadata,
                trace: trace.clone(),
            }));
        let expected = request;
        let checked_call = async move {
            let response = call.await?;
            validate_approval_response(&expected, &response)?;
            Ok(response)
        };
        Self::execute(
            eval,
            id,
            HostRequestKind::Approval,
            trace_payload,
            authority,
            trace,
            checked_call,
        )
        .await
    }
}

fn unix_timestamp_micros() -> Result<u64, HostError> {
    let elapsed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| {
            HostError::new(
                HostErrorCode::ProviderUnavailable,
                "system clock is earlier than the Unix epoch required by host tracing",
            )
        })?;
    u64::try_from(elapsed.as_micros()).map_err(|_| {
        HostError::new(
            HostErrorCode::ProviderUnavailable,
            "system clock timestamp exceeds host trace ABI range",
        )
    })
}

pub(super) trait TraceableHostResponse {
    fn response_id(&self) -> HostRequestId;
    fn outcome(&self) -> HostOutcome;
}

macro_rules! result_response {
    ($response:ty) => {
        impl TraceableHostResponse for $response {
            fn response_id(&self) -> HostRequestId {
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
    fn response_id(&self) -> HostRequestId {
        self.id
    }

    fn outcome(&self) -> HostOutcome {
        HostOutcome::Succeeded
    }
}

impl TraceableHostResponse for ConsoleResponse {
    fn response_id(&self) -> HostRequestId {
        self.id
    }

    fn outcome(&self) -> HostOutcome {
        HostOutcome::Succeeded
    }
}

impl TraceableHostResponse for PolicyResponse {
    fn response_id(&self) -> HostRequestId {
        self.id
    }

    fn outcome(&self) -> HostOutcome {
        HostOutcome::Succeeded
    }
}

impl TraceableHostResponse for ApprovalResponse {
    fn response_id(&self) -> HostRequestId {
        self.id
    }

    fn outcome(&self) -> HostOutcome {
        HostOutcome::Succeeded
    }
}

impl TraceableHostResponse for StreamResponse {
    fn response_id(&self) -> HostRequestId {
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

fn validate_approval_response(
    request: &etas_host::ApprovalRequest,
    response: &ApprovalResponse,
) -> Result<(), HostError> {
    if response.id != request.id {
        return Err(HostError::new(
            HostErrorCode::InvalidResponse,
            "approval response id does not match the originating request id",
        )
        .with_detail("request_id", request.id.0.to_string())
        .with_detail("response_id", response.id.0.to_string()));
    }
    let etas_host::ApprovalDecision::Approved { grant } = &response.decision else {
        return Ok(());
    };
    if grant.id != request.id {
        return Err(HostError::new(
            HostErrorCode::InvalidResponse,
            "approval grant id does not match the originating request id",
        )
        .with_detail("request_id", request.id.0.to_string())
        .with_detail("grant_id", grant.id.0.to_string()));
    }
    let mut accepted: Vec<&etas_host::HostActionGrant> = Vec::new();
    for grant in &grant.grants {
        if accepted.contains(&grant) {
            return Err(HostError::new(
                HostErrorCode::InvalidResponse,
                "approval response contains a duplicate authority grant",
            ));
        }
        if !request.requested_grants.contains(grant) {
            return Err(HostError::new(
                HostErrorCode::InvalidResponse,
                "approval response grants authority outside the request",
            ));
        }
        accepted.push(grant);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approval_validation_rejects_replayed_grant_id() {
        let requested = etas_host::HostActionGrant::allow("Console", "stdout_write");
        let request = etas_host::ApprovalRequest {
            id: HostRequestId(12),
            reason: "approve console".to_owned(),
            requested_grants: vec![requested.clone()],
            trace: TraceContext::root(etas_host::TraceId(1)),
        };
        let response = ApprovalResponse {
            id: request.id,
            decision: etas_host::ApprovalDecision::Approved {
                grant: etas_host::ApprovalGrant {
                    id: HostRequestId(11),
                    grants: vec![requested],
                },
            },
        };

        let error = validate_approval_response(&request, &response)
            .expect_err("a grant from another approval request must be rejected");
        assert_eq!(error.code, HostErrorCode::InvalidResponse);
        assert!(error.message.contains("approval grant id does not match"));
    }
}
