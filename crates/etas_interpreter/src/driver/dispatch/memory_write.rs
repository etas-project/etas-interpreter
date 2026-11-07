use super::host_dispatch::{HostCompletion, HostDispatch, PendingHostOperation};
use crate::{
    eval::EvalContext,
    host::HostServices,
    orchestration::{StorageWriteRecord, WorkflowEvent},
};
use etas_host::memory::{
    MemoryMutation, MemoryMutationKind, MemoryNotCommitted, MemoryWriteOperation,
    MemoryWriteRequest, MemoryWriteResponse, MemoryWriteResult,
};
use etas_host::{
    HostError, HostErrorCode, HostRequestKind, HostTraceRequest, MemoryOperation, MemoryRequest,
    MemoryResult, WriteOutcome,
};

pub(super) enum WriteFailure {
    NotCommitted(HostError),
    Terminal(HostError),
}

pub(in crate::driver) struct PendingMemoryWrite<'host> {
    pending: PendingHostOperation<
        MemoryWriteResponse,
        crate::host::HostFuture<'host, Result<MemoryWriteResponse, HostError>>,
    >,
    request_id: etas_host::HostRequestId,
    expected: etas_host::StorageOperationRef,
}

pub(in crate::driver) struct CompletedMemoryWrite {
    completion: HostCompletion<MemoryWriteResponse>,
    request_id: etas_host::HostRequestId,
    expected: etas_host::StorageOperationRef,
}

impl PendingMemoryWrite<'_> {
    pub(in crate::driver) fn operation_id(&self) -> etas_host::execution::OperationId {
        self.pending.operation_id()
    }
    pub(in crate::driver) async fn poll(self) -> CompletedMemoryWrite {
        CompletedMemoryWrite {
            completion: HostDispatch::poll(self.pending).await,
            request_id: self.request_id,
            expected: self.expected,
        }
    }
}

pub(super) fn begin<'host>(
    eval: &mut EvalContext<'_>,
    host: &'host dyn HostServices,
    request: MemoryRequest,
) -> Result<PendingMemoryWrite<'host>, WriteFailure> {
    let mutation = match &request.operation {
        MemoryOperation::Put {
            key,
            value,
            condition,
        } => MemoryMutation::Put {
            key: key.clone(),
            value: value.clone(),
            condition: condition.clone(),
        },
        MemoryOperation::Delete { key, condition } => MemoryMutation::Delete {
            key: key.clone(),
            condition: condition.clone(),
        },
        _ => {
            return Err(WriteFailure::NotCommitted(invalid(
                "read request entered write dispatch",
            )));
        }
    };
    let kind = match mutation {
        MemoryMutation::Put { .. } => MemoryMutationKind::Put,
        MemoryMutation::Delete { .. } => MemoryMutationKind::Delete,
    };
    let target = mutation.target(&request.store);
    let key = eval
        .storage_identity
        .clone()
        .and_then(|identity| identity.derive(u64::from(request.id.0)))
        .map_err(WriteFailure::NotCommitted)?;
    let expected = mutation
        .operation_ref(&request.store, key.clone(), &eval.storage_limits.clone())
        .map_err(WriteFailure::NotCommitted)?;
    eval.bind_storage_operation(request.id, &expected)
        .map_err(WriteFailure::NotCommitted)?;
    let write = MemoryWriteRequest {
        id: request.id,
        store: request.store.clone(),
        operation: MemoryWriteOperation::Mutate { key, mutation },
        authority: request.authority.clone(),
        trace: request.trace.clone(),
        budget: request.budget.clone(),
    };
    let expected_response = expected.clone();
    let pending = HostDispatch::begin(
        eval,
        request.id,
        HostRequestKind::Memory,
        request.trace_payload(),
        request.authority,
        request.trace,
        |operation| {
            let call = host.memory_write(operation, write);
            let call: crate::host::HostFuture<'host, Result<MemoryWriteResponse, HostError>> =
                Box::pin(async move {
                    let response = call.await?;
                    if let Ok(result) = &response.result {
                        let operation = match result {
                            MemoryWriteResult::Outcome(WriteOutcome::Committed(receipt))
                                if receipt.change.kind() == kind && receipt.target == target =>
                            {
                                &receipt.operation
                            }
                            MemoryWriteResult::Outcome(
                                WriteOutcome::NotCommitted { operation, .. }
                                | WriteOutcome::Unknown { operation, .. },
                            ) => operation,
                            _ => {
                                return Err(invalid(
                                    "memory write returned an inconsistent target or result kind",
                                ));
                            }
                        };
                        if operation != &expected_response {
                            return Err(invalid(
                                "memory write receipt belongs to a different operation",
                            ));
                        }
                    }
                    Ok(response)
                });
            call
        },
    )
    .map_err(WriteFailure::NotCommitted)?;
    Ok(PendingMemoryWrite {
        pending,
        request_id: request.id,
        expected,
    })
}

pub(in crate::driver) fn finish(
    eval: &mut EvalContext<'_>,
    completed: CompletedMemoryWrite,
) -> Result<MemoryResult, WriteFailure> {
    let CompletedMemoryWrite {
        completion,
        request_id,
        expected,
    } = completed;
    let response = HostDispatch::finish(eval, completion);
    match response.and_then(|response| response.result) {
        Ok(MemoryWriteResult::Outcome(WriteOutcome::Committed(receipt))) => {
            Ok(match receipt.change {
                etas_host::memory::MemoryWriteChange::Written { version } => {
                    MemoryResult::Written { version }
                }
                etas_host::memory::MemoryWriteChange::Deleted { tombstone } => {
                    MemoryResult::Deleted { version: tombstone }
                }
            })
        }
        Ok(MemoryWriteResult::Outcome(WriteOutcome::NotCommitted { reason, .. })) => match reason {
            MemoryNotCommitted::Unchanged => Ok(MemoryResult::Unchanged),
            MemoryNotCommitted::Conflict {
                expected,
                actual,
                current_value,
            } => Ok(MemoryResult::Conflict(etas_host::MemoryConflict {
                expected,
                actual,
                current_value,
            })),
            MemoryNotCommitted::Rejected(error) => Err(WriteFailure::NotCommitted(error)),
        },
        Ok(MemoryWriteResult::Outcome(WriteOutcome::Unknown { error, .. })) => {
            Err(WriteFailure::Terminal(unknown(error)))
        }
        Err(error) => {
            if eval
                .storage_writes
                .iter()
                .any(|write| write.request == request_id.0)
            {
                return Err(WriteFailure::Terminal(HostError::new(
                    error.code,
                    format!(
                        "memory boundary interrupted; commit evidence is retained: {}",
                        error.message
                    ),
                )));
            }
            let evidence = etas_host::StorageWriteEvidence {
                operation: expected,
                status: etas_host::CommitStatus::Unknown,
            };
            eval.storage_writes.push(StorageWriteRecord {
                request: request_id.0,
                evidence: evidence.clone(),
            });
            eval.events.push(WorkflowEvent::StorageWrite {
                request: request_id,
                evidence,
            });
            Err(WriteFailure::Terminal(unknown(error)))
        }
        Ok(_) => Err(WriteFailure::Terminal(invalid(
            "memory write returned a read receipt",
        ))),
    }
}
fn invalid(message: &str) -> HostError {
    HostError::new(HostErrorCode::InvalidResponse, message)
}
fn unknown(error: HostError) -> HostError {
    HostError::new(
        error.code,
        format!(
            "memory write commit outcome is unknown; automatic retry is forbidden: {}",
            error.message
        ),
    )
}
