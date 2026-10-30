use super::*;
use crate::control::ExecutionFault;
use etas_host::console::ConsoleResult;
use etas_host::{HostError, HostErrorCode};

impl<'a> EvalContext<'a> {
    pub(crate) fn console_result_value(
        &self,
        console: &PendingConsole,
        result: ConsoleResult,
    ) -> Result<InterpValue, ExecutionFault> {
        match (console.decode, result) {
            (ConsoleDecode::String, ConsoleResult::Input(text)) => {
                Ok(InterpValue::String(text.into()))
            }
            (ConsoleDecode::Unit, ConsoleResult::Written) => Ok(InterpValue::Unit),
            (_, other) => Err(ExecutionFault::new(
                AnalysisDiagnosticCode::UnhandledRuntimeError,
                console.span,
                format!(
                    "console operation returned a result shape that does not match the checked console operation: {:?}",
                    other
                ),
            )),
        }
    }

    pub(crate) fn console_boundary_key(
        &self,
        console: &PendingConsole,
    ) -> Result<etas_host::replay::ReplayFingerprint, crate::control::ExecutionFault> {
        self.replay_fingerprint(&console.request, console.request.id, console.span)
    }

    pub(crate) fn console_host_error_signal(
        &mut self,
        console: PendingConsole,
        error: HostError,
    ) -> ControlSignal {
        if let Some(signal) = self.cancellation_signal(console.span) {
            return signal;
        }
        let error_type = match self.standard_io_error_type(console.span) {
            Ok(error_type) => error_type,
            Err(fault) => return ControlSignal::Fault(Box::new(fault)),
        };
        let perform = PendingPerform {
            checked_action: None,
            default_boundary: None,
            forwarded_call: None,
            inference: None,
            expr: None,
            action: error_raise_action(console.span),
            error_type: Some(error_type),
            args: vec![console_error_value(error)],
            span: console.span,
            continuation: console.continuation,
        };
        self.propagate_perform_signal(perform)
    }

    fn standard_io_error_type(&self, span: Span) -> Result<etas_types::TypeId, ExecutionFault> {
        self.known_std_types.io_error.ok_or_else(|| {
            ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                span,
                "console host failure requires checked std.io IOError type",
            )
        })
    }
}

fn error_raise_action(span: Span) -> ResolvedActionRef {
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

fn console_error_value(error: HostError) -> InterpValue {
    match error.code {
        HostErrorCode::AuthorityDenied => InterpValue::Variant {
            name: "PermissionDenied".to_owned().into(),
            fields: Vec::new().into(),
        },
        HostErrorCode::ProviderRejected
        | HostErrorCode::ProviderUnavailable
        | HostErrorCode::ToolRejected
        | HostErrorCode::ToolUnavailable
        | HostErrorCode::InvalidRequest
        | HostErrorCode::InvalidResponse
        | HostErrorCode::IoFailure
        | HostErrorCode::SchemaMismatch
        | HostErrorCode::BudgetExceeded
        | HostErrorCode::TimedOut
        | HostErrorCode::Cancelled
        | HostErrorCode::Closed
        | HostErrorCode::Interrupted => InterpValue::Variant {
            name: "Host".to_owned().into(),
            fields: vec![InterpValue::String(
                format!("{}: {}", error.code.as_str(), error.message).into(),
            )]
            .into(),
        },
    }
}
