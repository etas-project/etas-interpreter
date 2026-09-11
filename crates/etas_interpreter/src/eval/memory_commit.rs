use super::*;
use crate::intrinsic::dispatch::{CheckedStdIntrinsicCall, MemoryIntentCallable};
use etas_host::memory::{MemoryWriteOperation, MemoryWriteRequest, MemoryWriteResult};
use etas_host::{HostError, HostErrorCode};

impl EvalContext<'_> {
    pub(super) fn execute_memory_commit(
        &mut self,
        kind: MemoryIntentCallable,
        checked_call: &CheckedStdIntrinsicCall,
        args: Vec<InterpValue>,
        span: Span,
    ) -> ControlSignal {
        match self.memory_commit_request(kind, checked_call, &args) {
            Ok(request) => ControlSignal::pending_host(PendingHostBoundary {
                request: HostBoundaryRequest::MemoryWrite(request),
                decode: HostBoundaryDecode::Storage {
                    result_type: checked_call.result_type,
                },
                span,
                continuation: Continuation::BlockValue,
            }),
            Err(error) => {
                self.storage_error_with_continuation(error, span, Continuation::BlockValue)
            }
        }
    }

    fn memory_commit_request(
        &mut self,
        kind: MemoryIntentCallable,
        checked: &CheckedStdIntrinsicCall,
        args: &[InterpValue],
    ) -> Result<MemoryWriteRequest, HostError> {
        let limits = self.storage_limits.clone();
        match (kind, args) {
            (MemoryIntentCallable::Commit, [InterpValue::MemoryWriteIntent(value)]) => {
                value
                    .validate(self.checked, &self.storage_limits)
                    .map_err(invalid)?;
                if checked.parameter_types.as_slice() != [value.ty] {
                    return Err(invalid("commit intent does not match the checked call ABI"));
                }
                value.intent().clone().into_request(
                    self.next_host_request_id(),
                    self.host_authority(),
                    self.host_trace(),
                    self.host_budget(),
                    &limits,
                )
            }
            (
                MemoryIntentCallable::Reconcile,
                [
                    InterpValue::MemoryStore {
                        region_stable_id,
                        path,
                        key_type,
                        value_type,
                    },
                    reference,
                ],
            ) => {
                if !matches!(checked.parameter_types.first().and_then(|ty| self.checked.type_store.get(*ty)),
                    Some(etas_types::Type::Store { key, value }) if key == key_type && value == value_type)
                {
                    return Err(invalid(
                        "reconcile Store does not match the checked call ABI",
                    ));
                }
                let operation =
                    self.checked_operation_ref(reference, checked.parameter_types.get(1))?;
                Ok(MemoryWriteRequest {
                    id: self.next_host_request_id(),
                    store: StoreRef {
                        region: MemoryRegionRef {
                            stable_id: region_stable_id.clone(),
                            schema_fingerprint: None,
                        },
                        path: path.clone(),
                    },
                    operation: MemoryWriteOperation::Reconcile { operation },
                    authority: self.host_authority(),
                    trace: self.host_trace(),
                    budget: self.host_budget(),
                })
            }
            _ => Err(invalid("invalid checked storage call arguments")),
        }
    }

    pub(crate) fn resume_storage_result(
        &mut self,
        boundary: PendingHostBoundary,
        result: MemoryWriteResult,
    ) -> ControlSignal {
        let HostBoundaryDecode::Storage { result_type } = boundary.decode else {
            return ControlSignal::missing_checked_fact(
                "storage result requires its checked outcome type",
                boundary.span,
            );
        };
        let value = etas_host::memory::memory_write_result_value(result);
        match super::host_value::host_to_checked_interp_value(
            value,
            result_type,
            self.checked,
            &self.storage_limits.clone(),
        ) {
            Ok(value) => self.resume_host_signal(boundary, value),
            Err(error) => ControlSignal::missing_checked_fact(error, boundary.span),
        }
    }
}

fn invalid(message: impl Into<String>) -> HostError {
    HostError::new(HostErrorCode::InvalidRequest, message)
}
