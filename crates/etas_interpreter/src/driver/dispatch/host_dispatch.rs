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
    pub(super) async fn execute<Response, Call, Dispatch>(
        eval: &mut EvalContext<'_>,
        request_id: HostRequestId,
        kind: HostRequestKind,
        trace_payload: HostTracePayload,
        authority: etas_host::AuthorityContext,
        trace: TraceContext,
        call: Dispatch,
    ) -> Result<Response, HostError>
    where
        Response: TraceableHostResponse,
        Call: Future<Output = Result<Response, HostError>>,
        Dispatch: FnOnce(etas_host::execution::OperationContext) -> Call,
    {
        let metadata =
            HostTraceMetadata::from_payload(&trace_payload, eval.host_trace_digest_key()?)?;
        let started_at_unix_micros = unix_timestamp_micros()?;
        let started = Instant::now();
        let operation = eval
            .execution
            .register(None, Some(request_id), trace.clone())?;
        if let Err(error) = operation.begin_dispatch() {
            operation.complete(
                etas_host::execution::ExternalOutcome::NotDispatched,
                Vec::new(),
            )?;
            return Err(error);
        }
        eval.events
            .push(WorkflowEvent::HostTrace(TraceEvent::HostRequestStarted {
                id: request_id,
                kind,
                metadata,
                authority: Box::new(authority),
                trace,
                started_at_unix_micros,
            }));
        let result = match call(operation.context().clone()).await {
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
        let outcome = match &result {
            Ok(response) => response.outcome(),
            Err(error) => HostOutcome::Failed(error.clone()),
        };
        let external = match &result {
            Ok(response) => response.external_outcome(),
            Err(_) => etas_host::execution::ExternalOutcome::Unknown,
        };
        if let etas_host::execution::ExternalOutcome::StorageWrite(evidence) = &external {
            eval.storage_writes
                .push(crate::orchestration::StorageWriteRecord {
                    request: request_id.0,
                    evidence: evidence.clone(),
                });
            eval.events.push(WorkflowEvent::StorageWrite {
                request: request_id,
                evidence: evidence.clone(),
            });
        }
        operation.complete(external, Vec::new())?;
        let finished_at_unix_micros = unix_timestamp_micros()?;
        let duration_micros = u64::try_from(started.elapsed().as_micros()).map_err(|_| {
            HostError::new(
                HostErrorCode::InvalidResponse,
                "host request duration exceeded trace ABI range",
            )
        })?;
        eval.events
            .push(WorkflowEvent::HostTrace(TraceEvent::HostRequestFinished {
                id: request_id,
                outcome,
                command_isolation: result
                    .as_ref()
                    .ok()
                    .and_then(|response| response.command_isolation()),
                finished_at_unix_micros,
                duration_micros,
            }));
        eval.execution.signal()?.check()?;
        result
    }

    pub(super) async fn execute_approval<Call, Dispatch>(
        eval: &mut EvalContext<'_>,
        request: etas_host::ApprovalRequest,
        authority: etas_host::AuthorityContext,
        call: Dispatch,
    ) -> Result<ApprovalResponse, HostError>
    where
        Call: Future<Output = Result<ApprovalResponse, HostError>>,
        Dispatch: FnOnce(etas_host::execution::OperationContext) -> Call,
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
        let budget = eval.host_budget();
        let checked_call = |operation| async move {
            let response = await_approval_response(call(operation), budget).await?;
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

async fn await_approval_response(
    call: impl Future<Output = Result<ApprovalResponse, HostError>>,
    budget: etas_host::ExecutionBudget,
) -> Result<ApprovalResponse, HostError> {
    budget.check_time()?;
    match budget.deadline()? {
        Some(deadline) => tokio::select! {
            biased;
            _ = tokio::time::sleep_until(deadline) => Err(HostError::new(
                HostErrorCode::BudgetExceeded,
                "approval input exceeded the run-owned time budget",
            )),
            response = call => response,
        },
        None => call.await,
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

pub(super) trait TraceableHostResponse: etas_host::execution::OperationResponse {
    fn response_id(&self) -> HostRequestId;
    fn outcome(&self) -> HostOutcome;
    fn command_isolation(&self) -> Option<etas_host::CommandIsolationReport> {
        None
    }
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
result_response!(TcpConnectResponse);
result_response!(TlsConnectResponse);
result_response!(SecretResponse);
result_response!(BrowserProtocolResponse);

impl TraceableHostResponse for etas_host::memory::MemoryWriteResponse {
    fn response_id(&self) -> HostRequestId {
        self.id
    }
    fn outcome(&self) -> HostOutcome {
        use etas_host::{
            WriteOutcome,
            memory::{MemoryNotCommitted, MemoryWriteResult},
        };
        match &self.result {
            Err(error)
            | Ok(MemoryWriteResult::Outcome(WriteOutcome::Unknown { error, .. }))
            | Ok(MemoryWriteResult::Outcome(WriteOutcome::NotCommitted {
                reason: MemoryNotCommitted::Rejected(error),
                ..
            })) => HostOutcome::Failed(error.clone()),
            Ok(_) => HostOutcome::Succeeded,
        }
    }
}

impl TraceableHostResponse for etas_host::session::SessionWriteResponse {
    fn response_id(&self) -> HostRequestId {
        self.id
    }
    fn outcome(&self) -> HostOutcome {
        use etas_host::{WriteOutcome, session::SessionWriteResult};
        match &self.result {
            Err(error)
            | Ok(SessionWriteResult::Context(WriteOutcome::Unknown { error, .. }))
            | Ok(SessionWriteResult::Context(WriteOutcome::NotCommitted {
                reason: etas_host::session::SessionContextRejection::Rejected(error),
                ..
            }))
            | Ok(SessionWriteResult::Outcome(WriteOutcome::Unknown { error, .. }))
            | Ok(SessionWriteResult::Outcome(WriteOutcome::NotCommitted {
                reason: error, ..
            })) => HostOutcome::Failed(error.clone()),
            Ok(_) => HostOutcome::Succeeded,
        }
    }
}

impl TraceableHostResponse for CommandResponse {
    fn response_id(&self) -> HostRequestId {
        self.id
    }

    fn outcome(&self) -> HostOutcome {
        match &self.result {
            Ok(_) => HostOutcome::Succeeded,
            Err(error) => HostOutcome::Failed(error.clone()),
        }
    }

    fn command_isolation(&self) -> Option<etas_host::CommandIsolationReport> {
        self.result
            .as_ref()
            .ok()
            .map(|output| output.isolation.clone())
    }
}

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

    #[tokio::test(flavor = "current_thread")]
    async fn pending_approval_obeys_the_run_owned_deadline() {
        let budget = etas_host::ExecutionBudget::start(etas_host::Budget {
            time: Some(etas_host::TimeBudget { max_millis: 20 }),
            ..Default::default()
        });
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            await_approval_response(std::future::pending(), budget),
        )
        .await
        .expect("approval future must not outlive its budget");
        assert_eq!(result.unwrap_err().code, HostErrorCode::BudgetExceeded);
    }

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
