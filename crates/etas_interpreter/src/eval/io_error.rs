use super::*;
use etas_host::{HostError, HostErrorCode, HostValue};

impl EvalContext<'_> {
    pub(crate) fn source_io_error_value(
        &self,
        error: &HostError,
        span: Span,
    ) -> Result<InterpValue, crate::control::ExecutionFault> {
        let ty = self.known_std_types.io_error.ok_or_else(|| {
            crate::control::ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                span,
                "I/O failure requires the checked shared std.io.IOError",
            )
        })?;
        let kind = error
            .details
            .iter()
            .find(|detail| detail.key == "io_kind")
            .map(|detail| detail.value.as_str());
        let name = match kind {
            Some("UnexpectedEof") => "Eof",
            Some("Interrupted") => "Interrupted",
            Some("PermissionDenied") => "PermissionDenied",
            Some("InvalidData" | "InvalidUtf8") => "InvalidUtf8",
            Some("NotFound") => "NotFound",
            Some("AlreadyExists") => "AlreadyExists",
            Some("InvalidInput") => "InvalidInput",
            Some("NotADirectory") => "NotDirectory",
            Some("IsADirectory") => "IsDirectory",
            Some("DirectoryNotEmpty") => "DirectoryNotEmpty",
            Some("Unsupported") => "Unsupported",
            Some("TimedOut") => "TimedOut",
            _ => match error.code {
                HostErrorCode::AuthorityDenied => "PermissionDenied",
                HostErrorCode::Interrupted => "Interrupted",
                HostErrorCode::Closed => "Closed",
                HostErrorCode::TimedOut => "TimedOut",
                HostErrorCode::InvalidRequest => "InvalidInput",
                _ => "Host",
            },
        };
        let fields = if name == "Host" {
            vec![HostValue::String(error.message.clone())]
        } else {
            vec![]
        };
        decode_checked_host_value(
            HostValue::Variant {
                name: name.into(),
                fields,
            },
            ty,
            self.checked,
        )
        .map_err(|reason| {
            crate::control::ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                span,
                reason,
            )
        })
    }
}
