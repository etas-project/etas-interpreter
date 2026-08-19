use super::*;
use etas_host::HostError;

impl<'a> EvalContext<'a> {
    pub(crate) fn network_host_error_signal(
        &mut self,
        host: PendingHostBoundary,
        error: HostError,
    ) -> Option<ControlSignal> {
        let (error_type, fact_name) = match &host.request {
            HostBoundaryRequest::Tcp(_) => (
                self.known_std_types.network_error,
                "std.net.tcp NetworkError",
            ),
            HostBoundaryRequest::Tls(_) => (self.known_std_types.tls_error, "std.tls TlsError"),
            _ => return None,
        };
        let Some(error_type) = error_type else {
            return Some(ControlSignal::missing_checked_fact(
                format!("network host failure requires checked {fact_name} type"),
                host.span,
            ));
        };
        let perform = PendingPerform {
            expr: None,
            action: network_error_raise_action(host.span),
            error_type: Some(error_type),
            args: vec![typed_host_error_value(error_type, error)],
            span: host.span,
            continuation: host.continuation,
        };
        Some(self.propagate_perform_signal(perform))
    }
}

fn network_error_raise_action(span: Span) -> ResolvedActionRef {
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

fn typed_host_error_value(error_type: etas_types::TypeId, error: HostError) -> InterpValue {
    let code = error.code.as_str();
    let details = error
        .details
        .into_iter()
        .map(|detail| {
            InterpValue::Record(
                vec![
                    ("key".to_owned(), InterpValue::String(detail.key)),
                    ("value".to_owned(), InterpValue::String(detail.value)),
                ]
                .into(),
            )
        })
        .collect::<Vec<_>>();
    InterpValue::Nominal {
        ty: error_type,
        value: Box::new(InterpValue::Record(
            vec![
                ("code".to_owned(), InterpValue::String(code.to_owned())),
                ("message".to_owned(), InterpValue::String(error.message)),
                ("details".to_owned(), InterpValue::Array(details.into())),
            ]
            .into(),
        )),
    }
}
