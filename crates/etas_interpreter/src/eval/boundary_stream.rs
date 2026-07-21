use super::*;
use etas_host::{HostError, HostErrorCode};

impl<'a> EvalContext<'a> {
    pub(crate) fn stream_host_error_signal(
        &mut self,
        host: PendingHostBoundary,
        error: HostError,
    ) -> Option<ControlSignal> {
        let HostBoundaryRequest::Stream(_) = &host.request else {
            return None;
        };
        let Some(error_type) = self.known_std_types.stream_error else {
            return Some(ControlSignal::missing_checked_fact(
                "stream host failure requires checked std.stream StreamError type",
                host.span,
            ));
        };
        let perform = PendingPerform {
            expr: None,
            action: stream_error_raise_action(host.span),
            error_type: Some(error_type),
            args: vec![stream_error_value(error)],
            span: host.span,
            continuation: host.continuation,
        };
        Some(self.propagate_perform_signal(perform))
    }
}

fn stream_error_raise_action(span: Span) -> ResolvedActionRef {
    ResolvedActionRef {
        effect: etas_hir::HirEffectRef {
            path: etas_hir::unresolved_path_from_segments(&["Error"], span),
            args: Vec::new(),
            span,
        },
        action: "raise".to_owned(),
        action_symbol: ResolveResult::Unresolved,
        span,
    }
}

fn stream_error_value(error: HostError) -> InterpValue {
    let message = error.message;
    match error.code {
        HostErrorCode::BudgetExceeded => InterpValue::Variant {
            name: "LimitExceeded".to_owned(),
            fields: Vec::new(),
        },
        HostErrorCode::ProviderUnavailable if host_error_timed_out(&error.details) => {
            InterpValue::Variant {
                name: "TimedOut".to_owned(),
                fields: Vec::new(),
            }
        }
        HostErrorCode::ProviderUnavailable if host_error_interrupted(&message) => {
            InterpValue::Variant {
                name: "Interrupted".to_owned(),
                fields: Vec::new(),
            }
        }
        HostErrorCode::ProviderUnavailable if host_error_closed(&message) => InterpValue::Variant {
            name: "Closed".to_owned(),
            fields: Vec::new(),
        },
        _ => InterpValue::Variant {
            name: "Host".to_owned(),
            fields: vec![InterpValue::String(format!(
                "{}: {message}; details={:?}",
                host_error_code_name(error.code),
                error.details
            ))],
        },
    }
}

fn host_error_timed_out(details: &[etas_host::HostErrorDetail]) -> bool {
    details.iter().any(|detail| detail.key == "timeout_ms")
}

fn host_error_closed(message: &str) -> bool {
    message.to_ascii_lowercase().contains("closed")
}

fn host_error_interrupted(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    message.contains("interrupt") || message.contains("cancel")
}

fn host_error_code_name(code: HostErrorCode) -> &'static str {
    match code {
        HostErrorCode::ProviderRejected => "ProviderRejected",
        HostErrorCode::ProviderUnavailable => "ProviderUnavailable",
        HostErrorCode::ToolRejected => "ToolRejected",
        HostErrorCode::ToolUnavailable => "ToolUnavailable",
        HostErrorCode::InvalidRequest => "InvalidRequest",
        HostErrorCode::InvalidResponse => "InvalidResponse",
        HostErrorCode::SchemaMismatch => "SchemaMismatch",
        HostErrorCode::BudgetExceeded => "BudgetExceeded",
        HostErrorCode::AuthorityDenied => "AuthorityDenied",
    }
}
