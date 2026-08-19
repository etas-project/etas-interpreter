use super::*;
use crate::control::ExecutionFault;
use etas_host::console::ConsoleResult;
use etas_host::{HostError, HostErrorCode};

impl<'a> EvalContext<'a> {
    pub(crate) fn replayed_console_result(&self, console: &PendingConsole) -> Option<InterpValue> {
        let key = self.console_boundary_key(console);
        self.completed_host_boundary_result("console", &key)
    }

    pub(crate) fn console_result_value(
        &self,
        console: &PendingConsole,
        result: ConsoleResult,
    ) -> Result<InterpValue, ExecutionFault> {
        match (console.decode, result) {
            (ConsoleDecode::String, ConsoleResult::Input(text)) => Ok(InterpValue::String(text)),
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

    pub(crate) fn console_boundary_key(&self, console: &PendingConsole) -> String {
        let operation = match &console.request.operation {
            ConsoleOperation::ReadAllStdin => "read_all".to_owned(),
            ConsoleOperation::ReadLineStdin => "read_line".to_owned(),
            ConsoleOperation::WriteStdout { text, newline } => {
                format!("stdout:{newline}:{text}")
            }
            ConsoleOperation::WriteStderr { text, newline } => {
                format!("stderr:{newline}:{text}")
            }
        };
        format!("console:{operation}")
    }

    pub(crate) fn console_host_error_signal(
        &mut self,
        console: PendingConsole,
        error: HostError,
    ) -> ControlSignal {
        let error_type = match self.standard_io_error_type(console.span) {
            Ok(error_type) => error_type,
            Err(fault) => return ControlSignal::Fault(Box::new(fault)),
        };
        let perform = PendingPerform {
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
            name: "PermissionDenied".to_owned(),
            fields: Vec::new(),
        },
        HostErrorCode::ProviderRejected
        | HostErrorCode::ProviderUnavailable
        | HostErrorCode::ToolRejected
        | HostErrorCode::ToolUnavailable
        | HostErrorCode::InvalidRequest
        | HostErrorCode::InvalidResponse
        | HostErrorCode::SchemaMismatch
        | HostErrorCode::BudgetExceeded
        | HostErrorCode::TimedOut
        | HostErrorCode::Cancelled
        | HostErrorCode::Closed
        | HostErrorCode::Interrupted => InterpValue::Variant {
            name: "Host".to_owned(),
            fields: vec![InterpValue::String(format!(
                "{}: {}",
                error.code.as_str(),
                error.message
            ))],
        },
    }
}
