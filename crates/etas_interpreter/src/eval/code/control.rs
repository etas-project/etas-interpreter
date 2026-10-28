use etas_code::syntax::SyntaxOperationError;
use etas_core::Span;
use etas_host::{ExecutionBudget, HostErrorCode, execution::CancelSignal};
use etas_utils::{PipelineExecutionControl, PipelineInterruption, PipelineInterruptionKind};

use crate::{control::ControlSignal, eval::EvalContext};

pub(crate) struct CodeSyntaxControl {
    signal: CancelSignal,
    budget: ExecutionBudget,
}

impl PipelineExecutionControl for CodeSyntaxControl {
    fn checkpoint(&self) -> Result<(), PipelineInterruption> {
        self.signal
            .check()
            .and_then(|()| self.budget.check_time())
            .map_err(|error| PipelineInterruption {
                kind: match error.code {
                    HostErrorCode::Cancelled => PipelineInterruptionKind::Cancelled,
                    HostErrorCode::BudgetExceeded | HostErrorCode::TimedOut => {
                        PipelineInterruptionKind::ResourceLimit
                    }
                    _ => PipelineInterruptionKind::Unavailable,
                },
                reason: error.to_string(),
            })
    }
}

impl EvalContext<'_> {
    pub(in crate::eval) fn eval_syntax_text_projection(
        &self,
        site: crate::plan::FieldAccessSite<'_>,
        syntax: crate::value::SyntaxBlockValue,
        field: &str,
        span: Span,
    ) -> ControlSignal {
        if self.checked.types.known_std_types.code.syntax_block != Some(syntax.ty())
            || self
                .plan
                .records
                .field_type(self.checked, site)
                .is_none_or(|ty| {
                    !matches!(
                        self.checked.type_store.get(ty),
                        Some(etas_types::Type::Primitive(
                            etas_types::PrimitiveType::String
                        ))
                    )
                })
            || field != "text"
        {
            return ControlSignal::missing_checked_fact(
                "syntax projection differs from its checked registry field",
                span,
            );
        }
        let control = match self.code_syntax_control(span) {
            Ok(control) => control,
            Err(fault) => return ControlSignal::Fault(Box::new(fault)),
        };
        match syntax.format_controlled(self.syntax_limits.max_formatted_bytes, &control) {
            Ok(text) => ControlSignal::Value(crate::value::InterpValue::String(text.into())),
            Err(error) => self.controlled_syntax_failure(error, span),
        }
    }

    pub(crate) fn code_syntax_control(
        &self,
        span: Span,
    ) -> Result<CodeSyntaxControl, crate::control::ExecutionFault> {
        Ok(CodeSyntaxControl {
            signal: self.execution.signal().map_err(|error| {
                crate::control::ExecutionFault::new(
                    etas_core::AnalysisDiagnosticCode::MissingCheckedFact,
                    span,
                    error.to_string(),
                )
            })?,
            budget: self.host_context.budget.clone(),
        })
    }

    pub(super) fn controlled_syntax_failure(
        &self,
        error: SyntaxOperationError,
        span: Span,
    ) -> ControlSignal {
        if matches!(&error, SyntaxOperationError::Interrupted(interruption) if interruption.kind == PipelineInterruptionKind::Cancelled)
        {
            return self.cancellation_signal(span).unwrap_or_else(|| {
                ControlSignal::missing_checked_fact(
                    "syntax backend reported cancellation without a run-owned cancellation cause",
                    span,
                )
            });
        }
        super::syntax_operation_failure(error, span)
    }
}
