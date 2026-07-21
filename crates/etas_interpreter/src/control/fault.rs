use etas_core::{AnalysisDiagnosticCode, Diagnostic, Span};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutionFault {
    pub code: AnalysisDiagnosticCode,
    pub span: Span,
    pub message: String,
    pub notes: Vec<String>,
}

impl ExecutionFault {
    pub fn new(code: AnalysisDiagnosticCode, span: Span, message: impl Into<String>) -> Self {
        Self {
            code,
            span,
            message: message.into(),
            notes: Vec::new(),
        }
    }

    pub fn into_diagnostic(self) -> Diagnostic {
        let mut diagnostic = Diagnostic::analysis(self.code, self.span, self.message);
        diagnostic.notes = self.notes;
        diagnostic
    }
}
