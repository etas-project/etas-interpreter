use super::*;
use etas_host::{HostError, HostErrorCode};

impl EvalContext<'_> {
    pub(crate) fn filesystem_io_failure_signal(
        &mut self,
        boundary: PendingHostBoundary,
        error: HostError,
    ) -> ControlSignal {
        if let Some(signal) = self.cancellation_signal(boundary.span) {
            return signal;
        }
        if error.code != HostErrorCode::IoFailure {
            return ControlSignal::missing_checked_fact(
                "filesystem IOError projection requires a tagged I/O failure",
                boundary.span,
            );
        }
        let Some(error_type) = self.known_std_types.filesystem_io_error else {
            return ControlSignal::missing_checked_fact(
                "filesystem operation requires checked std.fs.IOError",
                boundary.span,
            );
        };
        let value = InterpValue::Nominal {
            ty: error_type,
            value: InterpValue::String(error.to_string().into()).into(),
        };
        self.raise_file_error(error_type, value, boundary.continuation, boundary.span)
    }

    pub(super) fn raise_file_error(
        &mut self,
        error_type: etas_types::TypeId,
        value: InterpValue,
        continuation: Continuation,
        span: Span,
    ) -> ControlSignal {
        self.propagate_perform_signal(PendingPerform {
            checked_action: None,
            default_boundary: None,
            forwarded_call: None,
            inference: None,
            expr: None,
            action: etas_hir::ResolvedActionRef {
                effect: etas_hir::HirEffectRef {
                    path: etas_hir::unresolved_path_from_segments(&["Error"], span),
                    args: vec![],
                    span,
                },
                action: "raise".into(),
                action_symbol: etas_hir::ResolveResult::Unresolved,
                span,
            },
            error_type: Some(error_type),
            args: vec![value],
            span,
            continuation,
        })
    }
}
