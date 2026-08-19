use super::*;
use etas_host::StreamFailure;

impl<'a> EvalContext<'a> {
    pub(crate) fn stream_failure_signal(
        &mut self,
        host: PendingHostBoundary,
        failure: StreamFailure,
    ) -> ControlSignal {
        let Some(error_type) = self.known_std_types.stream_error else {
            return ControlSignal::missing_checked_fact(
                "stream host failure requires checked std.stream StreamError type",
                host.span,
            );
        };
        let perform = PendingPerform {
            expr: None,
            action: stream_error_raise_action(host.span),
            error_type: Some(error_type),
            args: vec![stream_error_value(failure)],
            span: host.span,
            continuation: host.continuation,
        };
        self.propagate_perform_signal(perform)
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

fn stream_error_value(failure: StreamFailure) -> InterpValue {
    match failure {
        StreamFailure::LimitExceeded { .. } => InterpValue::Variant {
            name: "LimitExceeded".to_owned(),
            fields: Vec::new(),
        },
        StreamFailure::TimedOut => InterpValue::Variant {
            name: "TimedOut".to_owned(),
            fields: Vec::new(),
        },
        StreamFailure::Cancelled => InterpValue::Variant {
            name: "Cancelled".to_owned(),
            fields: Vec::new(),
        },
        StreamFailure::Closed => InterpValue::Variant {
            name: "Closed".to_owned(),
            fields: Vec::new(),
        },
        StreamFailure::Interrupted => InterpValue::Variant {
            name: "Interrupted".to_owned(),
            fields: Vec::new(),
        },
        StreamFailure::Host(error) => InterpValue::Variant {
            name: "Host".to_owned(),
            fields: vec![InterpValue::String(format!(
                "{}: {}; details={:?}",
                error.code.as_str(),
                error.message,
                error.details
            ))],
        },
    }
}

#[cfg(test)]
mod tests {
    use etas_host::{HostError, HostErrorCode, StreamFailure};

    use super::stream_error_value;
    use crate::value::InterpValue;

    #[test]
    fn stream_error_mapping_uses_typed_code_not_message_text() {
        assert_eq!(
            stream_error_value(StreamFailure::Host(HostError::new(
                HostErrorCode::ProviderUnavailable,
                "closed, cancelled, interrupted, and timed out",
            ))),
            InterpValue::Variant {
                name: "Host".to_owned(),
                fields: vec![InterpValue::String(
                    "ProviderUnavailable: closed, cancelled, interrupted, and timed out; details=[]"
                        .to_owned(),
                )],
            }
        );
        assert_eq!(
            stream_error_value(StreamFailure::TimedOut),
            InterpValue::Variant {
                name: "TimedOut".to_owned(),
                fields: Vec::new(),
            }
        );
    }

    #[test]
    fn stream_error_mapping_preserves_all_typed_variants() {
        let cases = [
            (
                StreamFailure::LimitExceeded { limit_bytes: 1 },
                "LimitExceeded",
            ),
            (StreamFailure::TimedOut, "TimedOut"),
            (StreamFailure::Cancelled, "Cancelled"),
            (StreamFailure::Closed, "Closed"),
            (StreamFailure::Interrupted, "Interrupted"),
        ];
        for (failure, expected) in cases {
            assert_eq!(
                stream_error_value(failure),
                InterpValue::Variant {
                    name: expected.to_owned(),
                    fields: Vec::new(),
                }
            );
        }
    }
}
