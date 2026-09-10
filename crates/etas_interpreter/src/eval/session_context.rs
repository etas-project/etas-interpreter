use super::*;
use crate::intrinsic::dispatch::{CheckedStdIntrinsicCall, SessionContextCallable};
use etas_host::session::{
    SessionContextPublication, SessionWriteOperation, SessionWriteRequest, SessionWriteResult,
};
use etas_host::{HostError, HostErrorCode, SessionRef};

impl EvalContext<'_> {
    pub(super) fn execute_session_context(
        &mut self,
        kind: SessionContextCallable,
        checked: &CheckedStdIntrinsicCall,
        args: &[InterpValue],
        span: Span,
    ) -> ControlSignal {
        match self.session_context_operation(kind, checked, args) {
            Ok(operation) if kind == SessionContextCallable::Prepare => {
                let SessionWriteOperation::PublishContext(publication) = operation else {
                    return ControlSignal::missing_checked_fact(
                        "invalid context preparation",
                        span,
                    );
                };
                let op = publication.operation;
                let value = HostValue::Record(vec![
                    ("key".into(), HostValue::String(op.key.as_str().into())),
                    (
                        "fingerprint".into(),
                        HostValue::String(op.request_fingerprint),
                    ),
                ]);
                match super::host_value::host_to_checked_interp_value(
                    value,
                    checked.result_type,
                    self.checked,
                    &self.storage_limits.clone(),
                ) {
                    Ok(value) => ControlSignal::Value(value),
                    Err(error) => ControlSignal::missing_checked_fact(error, span),
                }
            }
            Ok(operation) => ControlSignal::pending_host(PendingHostBoundary {
                request: HostBoundaryRequest::SessionContext(SessionWriteRequest {
                    id: self.next_host_request_id(),
                    operation,
                    authority: self.host_authority(),
                    trace: self.host_trace(),
                    budget: self.host_budget(),
                }),
                decode: HostBoundaryDecode::SessionContext {
                    result_type: checked.result_type,
                },
                span,
                continuation: Continuation::BlockValue,
            }),
            Err(error) => self.storage_error_signal(error, span),
        }
    }
    fn session_context_operation(
        &self,
        kind: SessionContextCallable,
        checked: &CheckedStdIntrinsicCall,
        args: &[InterpValue],
    ) -> Result<SessionWriteOperation, HostError> {
        let (config, rest) = args
            .split_first()
            .ok_or_else(|| invalid("context operation requires session"))?;
        let config = self.checked_session_config(config, checked.parameter_types.first())?;
        let session = SessionRef { id: config.id };
        let limits = self.storage_limits.clone();
        match (kind, rest) {
            (SessionContextCallable::Reconcile, [operation]) => {
                Ok(SessionWriteOperation::ReconcileContext {
                    session,
                    operation: self
                        .checked_operation_ref(operation, checked.parameter_types.get(1))?,
                })
            }
            (SessionContextCallable::Prepare, [fence, content]) => {
                Ok(SessionWriteOperation::PublishContext(Box::new(
                    SessionContextPublication::prepare(
                        session,
                        self.session_history_fence(fence)?,
                        self.session_context_content(content)?,
                        &limits,
                    )?,
                )))
            }
            (SessionContextCallable::Publish, [fence, content, operation]) => {
                let fence = self.session_history_fence(fence)?;
                let content = self.session_context_content(content)?;
                let operation =
                    self.checked_operation_ref(operation, checked.parameter_types.get(3))?;
                let expected = etas_host::session::context_operation_ref(
                    &session,
                    &fence,
                    &content,
                    operation.key.clone(),
                    &limits,
                )?;
                if operation != expected {
                    return Err(invalid(
                        "context operation identity is bound to a different request",
                    ));
                }
                Ok(SessionWriteOperation::PublishContext(Box::new(
                    SessionContextPublication {
                        session,
                        fence,
                        content,
                        operation,
                    },
                )))
            }
            _ => Err(invalid("invalid context operation arguments")),
        }
    }
    pub(crate) fn resume_session_context_result(
        &mut self,
        boundary: PendingHostBoundary,
        result: SessionWriteResult,
    ) -> ControlSignal {
        let HostBoundaryDecode::SessionContext { result_type } = boundary.decode else {
            return ControlSignal::missing_checked_fact(
                "missing context result ABI",
                boundary.span,
            );
        };
        let value = etas_host::session::session_context_result_value(result)
            .map_err(|error| error.message)
            .and_then(|value| {
                super::host_value::host_to_checked_interp_value(
                    value,
                    result_type,
                    self.checked,
                    &self.storage_limits.clone(),
                )
            });
        match value {
            Ok(value) => self.resume_host_signal(boundary, value),
            Err(error) => ControlSignal::missing_checked_fact(error, boundary.span),
        }
    }
}
fn invalid(message: &str) -> HostError {
    HostError::new(HostErrorCode::InvalidRequest, message)
}
