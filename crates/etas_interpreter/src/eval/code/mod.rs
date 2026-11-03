mod annotation;
mod behavior;
mod compile_result;
mod control;
mod definition_fields;
mod diagnostic;
mod diff;
mod execution;
mod project;
mod report;
mod signature;
mod spec;
pub(super) use definition_fields::{agent_field, callable_field};

use etas_code::definition::DefinitionOperationError;
use etas_code::syntax::{ParseBodyFailure, SyntaxOperationError};
use etas_core::{AnalysisDiagnosticCode, Span};
use etas_types::Type;

use super::{ControlSignal, EvalContext, InterpValue};
use crate::intrinsic::dispatch::{CheckedStdIntrinsicCall, CodeSyntaxCallable};

impl EvalContext<'_> {
    pub(super) fn execute_code_syntax(
        &mut self,
        kind: CodeSyntaxCallable,
        call: &CheckedStdIntrinsicCall,
        args: Vec<InterpValue>,
        span: Span,
    ) -> ControlSignal {
        if let Some(signal) = self.code_operation_boundary(span) {
            return signal;
        }
        if kind == CodeSyntaxCallable::WithBody {
            let result = self.execute_code_with_body(call, args, span);
            return self.code_operation_boundary(span).unwrap_or(result);
        }
        if kind == CodeSyntaxCallable::Diff {
            let result = self.execute_code_diff(call, args, span);
            return self.code_operation_boundary(span).unwrap_or(result);
        }
        let [arg]: [InterpValue; 1] = match args.try_into() {
            Ok(args) => args,
            Err(_) => {
                return ControlSignal::invalid_arguments(
                    "syntax operation expects exactly one argument",
                    span,
                );
            }
        };
        let result = match (kind, arg) {
            (CodeSyntaxCallable::ParseBody, InterpValue::String(source)) => {
                let Some(Type::Result { ok, err }) = self.checked.type_store.get(call.result_type)
                else {
                    return ControlSignal::missing_checked_fact(
                        "parse_body result has no checked Result ABI",
                        span,
                    );
                };
                if self.checked.types.known_std_types.code.syntax_block != Some(*ok) {
                    return ControlSignal::missing_checked_fact(
                        "parse_body result is not the checked SyntaxBlock type",
                        span,
                    );
                }
                let control = match self.code_syntax_control(span) {
                    Ok(control) => control,
                    Err(fault) => return ControlSignal::Fault(Box::new(fault)),
                };
                match self.code_syntax.parse_body_bounded_controlled(
                    &source,
                    self.syntax_limits,
                    &control,
                ) {
                    Ok(syntax) => ControlSignal::Value(variant(
                        "Ok",
                        InterpValue::SyntaxBlock(crate::value::SyntaxBlockValue::new(
                            *ok,
                            syntax,
                            self.code_syntax.clone(),
                        )),
                    )),
                    Err(ParseBodyFailure::InvalidSyntax(error)) => {
                        match diagnostic::parse_error(
                            &self.checked.type_store,
                            &self.checked.types.known_std_types.code,
                            *err,
                            error,
                        ) {
                            Ok(value) => ControlSignal::Value(variant("Err", value)),
                            Err(reason) => ControlSignal::missing_checked_fact(reason, span),
                        }
                    }
                    Err(ParseBodyFailure::Operation(error)) => {
                        self.controlled_syntax_failure(error, span)
                    }
                }
            }
            (CodeSyntaxCallable::Format, InterpValue::SyntaxBlock(syntax)) => {
                if call.parameter_types.as_slice() != [syntax.ty()] {
                    return ControlSignal::missing_checked_fact(
                        "format argument differs from its checked SyntaxBlock ABI",
                        span,
                    );
                }
                let control = match self.code_syntax_control(span) {
                    Ok(control) => control,
                    Err(fault) => return ControlSignal::Fault(Box::new(fault)),
                };
                match syntax.format_controlled(self.syntax_limits.max_formatted_bytes, &control) {
                    Ok(text) => ControlSignal::Value(InterpValue::String(text.into())),
                    Err(error) => self.controlled_syntax_failure(error, span),
                }
            }
            _ => ControlSignal::invalid_arguments(
                "syntax operation argument differs from its checked ABI",
                span,
            ),
        };
        self.code_operation_boundary(span).unwrap_or(result)
    }

    pub(super) fn code_operation_boundary(&self, span: Span) -> Option<ControlSignal> {
        if self
            .code_definitions
            .as_ref()
            .is_some_and(|definitions| !definitions.is_for(self.checked))
        {
            return Some(ControlSignal::missing_checked_fact(
                "code operations require a definition catalog from the current checked program",
                span,
            ));
        }
        if let Some(signal) = self.cancellation_signal(span) {
            return Some(signal);
        }
        self.host_context.budget.check_time().err().map(|error| {
            ControlSignal::runtime_fault(
                format!(
                    "syntax operation exhausted the run-owned wall-time budget: {}",
                    error.message
                ),
                span,
            )
        })
    }

    fn execute_code_with_body(
        &mut self,
        call: &CheckedStdIntrinsicCall,
        args: Vec<InterpValue>,
        span: Span,
    ) -> ControlSignal {
        let [definition, InterpValue::SyntaxBlock(syntax)]: [InterpValue; 2] =
            (match args.try_into() {
                Ok(args) => args,
                Err(_) => {
                    return ControlSignal::invalid_arguments(
                        "with_body expects a definition and structured syntax",
                        span,
                    );
                }
            })
        else {
            return ControlSignal::invalid_arguments(
                "with_body argument differs from its checked definition/syntax ABI",
                span,
            );
        };
        let control = match self.code_syntax_control(span) {
            Ok(control) => control,
            Err(fault) => return ControlSignal::Fault(Box::new(fault)),
        };
        if let InterpValue::CallableDefinition(definition) = &definition {
            if call.parameter_types != [definition.ty(), syntax.ty()]
                || call.result_type != definition.ty()
                || !std::sync::Arc::ptr_eq(definition.program(), self.checked)
            {
                return ControlSignal::missing_checked_fact(
                    "with_body does not preserve its checked CallableDef<F> ABI and frozen environment",
                    span,
                );
            }
            let snapshot = match definition.definition() {
                Ok(snapshot) => snapshot,
                Err(reason) => return ControlSignal::missing_checked_fact(reason, span),
            };
            return match definition.backend().with_body_controlled(
                snapshot,
                syntax.syntax().clone(),
                &control,
            ) {
                Ok(edited) => match definition.with_definition(edited) {
                    Ok(edited) => ControlSignal::Value(InterpValue::CallableDefinition(edited)),
                    Err(reason) => ControlSignal::missing_checked_fact(reason, span),
                },
                Err(DefinitionOperationError::Syntax(error)) => {
                    self.controlled_syntax_failure(error, span)
                }
                Err(error) => definition_operation_failure(error.into(), span),
            };
        }
        let InterpValue::AgentDefinition(definition) = definition else {
            return ControlSignal::invalid_arguments(
                "with_body requires a checked definition descriptor",
                span,
            );
        };
        if call.parameter_types != [definition.ty(), syntax.ty()]
            || call.result_type != definition.ty()
            || !std::sync::Arc::ptr_eq(definition.program(), self.checked)
        {
            return ControlSignal::missing_checked_fact(
                "with_body does not preserve the checked nominal AgentDef<A> ABI and definition environment",
                span,
            );
        }
        match definition.backend().with_body_controlled(
            definition.definition(),
            syntax.syntax().clone(),
            &control,
        ) {
            Ok(edited) => ControlSignal::Value(InterpValue::AgentDefinition(
                crate::value::AgentDefinitionValue::with_abi(
                    call.result_type,
                    edited,
                    definition.backend().clone(),
                    self.checked.clone(),
                ),
            )),
            Err(DefinitionOperationError::Syntax(error)) => {
                self.controlled_syntax_failure(error, span)
            }
            Err(error) => definition_operation_failure(error.into(), span),
        }
    }
}

pub(super) fn definition_operation_failure(
    error: etas_frontend::code::definition::NativeDefinitionError,
    span: Span,
) -> ControlSignal {
    use etas_code::definition::DefinitionOperationError;
    use etas_frontend::code::definition::NativeDefinitionError;
    match error {
        NativeDefinitionError::Operation(DefinitionOperationError::ResourceLimit { .. }) => {
            ControlSignal::runtime_fault(error.to_string(), span)
        }
        NativeDefinitionError::Operation(DefinitionOperationError::Syntax(error)) => {
            syntax_operation_failure(error, span)
        }
        NativeDefinitionError::Projection(
            etas_frontend::code::descriptor::ProjectionError::Syntax(error),
        ) => syntax_operation_failure(error, span),
        _ => ControlSignal::missing_checked_fact(error.to_string(), span),
    }
}

pub(super) fn syntax_operation_failure(error: SyntaxOperationError, span: Span) -> ControlSignal {
    match error {
        SyntaxOperationError::Interrupted(error) => match error.kind {
            etas_utils::PipelineInterruptionKind::ResourceLimit => {
                ControlSignal::runtime_fault(error.reason, span)
            }
            etas_utils::PipelineInterruptionKind::Cancelled
            | etas_utils::PipelineInterruptionKind::Unavailable => {
                ControlSignal::missing_checked_fact(error.reason, span)
            }
        },
        SyntaxOperationError::ResourceLimit { .. } => {
            ControlSignal::runtime_fault(error.to_string(), span)
        }
        _ => ControlSignal::fault(
            AnalysisDiagnosticCode::MissingCheckedFact,
            span,
            error.to_string(),
        ),
    }
}

fn variant(name: &str, value: InterpValue) -> InterpValue {
    InterpValue::Variant {
        name: name.into(),
        fields: vec![value].into(),
    }
}
