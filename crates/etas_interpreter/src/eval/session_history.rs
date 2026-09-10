use super::*;
use crate::intrinsic::dispatch::CheckedStdIntrinsicCall;
use etas_host::{
    HostError, HostErrorCode, SessionCursor, SessionOperation, SessionRef, SessionRequest,
};

impl EvalContext<'_> {
    pub(super) fn execute_session_history_page(
        &mut self,
        checked: &CheckedStdIntrinsicCall,
        args: &[InterpValue],
        span: Span,
    ) -> ControlSignal {
        match self.session_history_request(checked, args) {
            Ok(request) => ControlSignal::pending_host(PendingHostBoundary {
                request: HostBoundaryRequest::SessionHistory(request),
                decode: HostBoundaryDecode::SessionHistory {
                    result_type: checked.result_type,
                },
                span,
                continuation: Continuation::BlockValue,
            }),
            Err(error) => self.storage_error_signal(error, span),
        }
    }

    fn session_history_request(
        &mut self,
        checked: &CheckedStdIntrinsicCall,
        args: &[InterpValue],
    ) -> Result<SessionRequest, HostError> {
        let [config, cursor, InterpValue::Number(limit)] = args else {
            return Err(invalid(
                "history_page requires SessionConfig, Option<SessionCursor>, u32",
            ));
        };
        let config = self.checked_session_config(config, checked.parameter_types.first())?;
        let limits = self.storage_limits.clone();
        let limit = limit
            .as_u32()
            .filter(|n| *n > 0 && (*n as usize) <= limits.max_page_entries)
            .ok_or_else(|| invalid("session page limit is outside checked bounds"))?;
        let cursor_type =
            super::resolve_std_type(self.checked, &["std", "agent", "session", "SessionCursor"])
                .ok_or_else(|| invalid("missing checked SessionCursor type"))?;
        let cursor = match cursor {
            InterpValue::OptionNone => None,
            InterpValue::OptionSome(value) => {
                let InterpValue::Nominal { ty, value } = value.as_ref() else {
                    return Err(invalid("session cursor lacks checked identity"));
                };
                if *ty != cursor_type {
                    return Err(invalid("session cursor has a foreign type"));
                }
                let InterpValue::Record(fields) = value.as_ref() else {
                    return Err(invalid("invalid cursor representation"));
                };
                let fields = fields.borrow();
                let [(name, InterpValue::String(opaque))] = fields.as_slice() else {
                    return Err(invalid("invalid cursor fields"));
                };
                if name != "opaque" || opaque.len() > limits.max_value_bytes {
                    return Err(invalid("invalid cursor token"));
                }
                Some(SessionCursor {
                    opaque: opaque.clone(),
                })
            }
            _ => return Err(invalid("history_page requires Option<SessionCursor>")),
        };
        Ok(SessionRequest {
            id: self.next_host_request_id(),
            operation: SessionOperation::Load {
                session: SessionRef { id: config.id },
                context: config.context,
                cursor,
                limit: Some(limit),
            },
            authority: self.host_authority(),
            trace: self.host_trace(),
            budget: self.host_budget(),
        })
    }

    pub(crate) fn resume_session_history_page(
        &mut self,
        boundary: PendingHostBoundary,
        value: HostValue,
    ) -> ControlSignal {
        let HostBoundaryDecode::SessionHistory { result_type } = boundary.decode else {
            return ControlSignal::missing_checked_fact(
                "session page is missing its checked result ABI",
                boundary.span,
            );
        };
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
