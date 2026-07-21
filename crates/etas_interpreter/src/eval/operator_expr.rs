use super::*;
use crate::control::ExecutionFault;
use crate::value::{NumericError, NumericValue};
use etas_hir::{HirBinaryOp, HirUnaryOp};

impl<'a> EvalContext<'a> {
    pub(super) fn eval_unary_expr(
        &mut self,
        op: HirUnaryOp,
        expr: HirExprId,
        span: Span,
        frame: &mut Frame,
    ) -> ControlSignal {
        if op == HirUnaryOp::Neg {
            let literal = self.checked.hir.exprs[expr].clone();
            let parsed = match literal {
                HirExpr::Literal(HirLiteral::Int {
                    text,
                    span: literal_span,
                }) => Some(
                    self.numeric_literal_type(expr, false, literal_span)
                        .and_then(|primitive| {
                            NumericValue::parse_negated_integer(&text, primitive)
                                .map(InterpValue::Number)
                                .map_err(|error| numeric_fault(error, span, "negation"))
                        }),
                ),
                HirExpr::Literal(HirLiteral::Float {
                    text,
                    span: literal_span,
                }) => Some(
                    self.numeric_literal_type(expr, true, literal_span)
                        .and_then(|primitive| {
                            NumericValue::parse_negated_float(&text, primitive)
                                .map(InterpValue::Number)
                                .map_err(|error| numeric_fault(error, span, "negation"))
                        }),
                ),
                _ => None,
            };
            if let Some(parsed) = parsed {
                if let Err(fault) = self.consume_execution_step(
                    self.checked.hir.exprs[expr].span(&self.checked.hir.blocks),
                ) {
                    return ControlSignal::Fault(Box::new(fault));
                }
                return match parsed {
                    Ok(value) => ControlSignal::Value(value),
                    Err(fault) => ControlSignal::Fault(Box::new(fault)),
                };
            }
        }
        match self.eval_expr(expr, frame) {
            ControlSignal::Value(value) => match self.eval_unary_value(op, value, span) {
                Ok(value) => ControlSignal::Value(value),
                Err(fault) => ControlSignal::Fault(Box::new(fault)),
            },
            signal if is_pending_host_boundary_signal(&signal) => {
                compose_signal_continuation(signal, Continuation::Unary { op, span })
            }
            other => other,
        }
    }

    pub(super) fn eval_unary_value(
        &self,
        op: HirUnaryOp,
        value: InterpValue,
        span: Span,
    ) -> Result<InterpValue, ExecutionFault> {
        match (op, value) {
            (HirUnaryOp::Not, InterpValue::Bool(value)) => Ok(InterpValue::Bool(!value)),
            (HirUnaryOp::Neg, InterpValue::Number(value)) => value
                .checked_neg()
                .map(InterpValue::Number)
                .map_err(|error| numeric_fault(error, span, "negation")),
            (HirUnaryOp::Not, other) => Err(ExecutionFault::new(
                AnalysisDiagnosticCode::InvalidArguments,
                span,
                format!("logical not expects bool, got {:?}", other),
            )),
            (HirUnaryOp::Neg, other) => Err(ExecutionFault::new(
                AnalysisDiagnosticCode::InvalidArguments,
                span,
                format!("numeric negation expects int, got {:?}", other),
            )),
        }
    }

    pub(super) fn eval_binary_expr(
        &mut self,
        op: HirBinaryOp,
        lhs: HirExprId,
        rhs: HirExprId,
        span: Span,
        frame: &mut Frame,
    ) -> ControlSignal {
        let left = match self.eval_expr(lhs, frame) {
            ControlSignal::Value(value) => value,
            signal if is_pending_host_boundary_signal(&signal) => {
                return compose_signal_continuation(
                    signal,
                    Continuation::BinaryLeft {
                        op,
                        rhs,
                        span,
                        frame: frame.clone(),
                    },
                );
            }
            other => return other,
        };
        self.resume_binary_left(op, left, rhs, span, frame)
    }

    pub(super) fn resume_binary_left(
        &mut self,
        op: HirBinaryOp,
        left: InterpValue,
        rhs: HirExprId,
        span: Span,
        frame: &mut Frame,
    ) -> ControlSignal {
        match op {
            HirBinaryOp::AndAnd => {
                let left_bool = match self.expect_bool(left, span) {
                    Ok(value) => value,
                    Err(fault) => return ControlSignal::Fault(Box::new(fault)),
                };
                if !left_bool {
                    return ControlSignal::Value(InterpValue::Bool(false));
                }
                let right = match self.eval_expr(rhs, frame) {
                    ControlSignal::Value(value) => value,
                    signal if is_pending_host_boundary_signal(&signal) => {
                        return compose_signal_continuation(
                            signal,
                            Continuation::BinaryRight {
                                op,
                                left: InterpValue::Bool(true),
                                span,
                            },
                        );
                    }
                    other => return other,
                };
                let right_bool = match self.expect_bool(right, span) {
                    Ok(value) => value,
                    Err(fault) => return ControlSignal::Fault(Box::new(fault)),
                };
                return ControlSignal::Value(InterpValue::Bool(right_bool));
            }
            HirBinaryOp::OrOr => {
                let left_bool = match self.expect_bool(left, span) {
                    Ok(value) => value,
                    Err(fault) => return ControlSignal::Fault(Box::new(fault)),
                };
                if left_bool {
                    return ControlSignal::Value(InterpValue::Bool(true));
                }
                let right = match self.eval_expr(rhs, frame) {
                    ControlSignal::Value(value) => value,
                    signal if is_pending_host_boundary_signal(&signal) => {
                        return compose_signal_continuation(
                            signal,
                            Continuation::BinaryRight {
                                op,
                                left: InterpValue::Bool(false),
                                span,
                            },
                        );
                    }
                    other => return other,
                };
                let right_bool = match self.expect_bool(right, span) {
                    Ok(value) => value,
                    Err(fault) => return ControlSignal::Fault(Box::new(fault)),
                };
                return ControlSignal::Value(InterpValue::Bool(right_bool));
            }
            _ => {}
        }
        let right = match self.eval_expr(rhs, frame) {
            ControlSignal::Value(value) => value,
            signal if is_pending_host_boundary_signal(&signal) => {
                return compose_signal_continuation(
                    signal,
                    Continuation::BinaryRight { op, left, span },
                );
            }
            other => return other,
        };
        match self.eval_binary_values(op, left, right, span) {
            Ok(value) => ControlSignal::Value(value),
            Err(fault) => ControlSignal::Fault(Box::new(fault)),
        }
    }

    pub(super) fn eval_binary_values(
        &self,
        op: HirBinaryOp,
        left: InterpValue,
        right: InterpValue,
        span: Span,
    ) -> Result<InterpValue, ExecutionFault> {
        match op {
            HirBinaryOp::EqEq => Ok(InterpValue::Bool(left == right)),
            HirBinaryOp::BangEq => Ok(InterpValue::Bool(left != right)),
            HirBinaryOp::Lt => {
                self.eval_numeric_comparison_value(left, right, span, |ordering| ordering.is_lt())
            }
            HirBinaryOp::LtEq => {
                self.eval_numeric_comparison_value(left, right, span, |ordering| ordering.is_le())
            }
            HirBinaryOp::Gt => {
                self.eval_numeric_comparison_value(left, right, span, |ordering| ordering.is_gt())
            }
            HirBinaryOp::GtEq => {
                self.eval_numeric_comparison_value(left, right, span, |ordering| ordering.is_ge())
            }
            HirBinaryOp::Add => self.eval_add_value(left, right, span),
            HirBinaryOp::Sub => self.eval_numeric_arithmetic_value(
                left,
                right,
                span,
                "subtraction",
                NumericValue::checked_sub,
            ),
            HirBinaryOp::Mul => self.eval_numeric_arithmetic_value(
                left,
                right,
                span,
                "multiplication",
                NumericValue::checked_mul,
            ),
            HirBinaryOp::Div => self.eval_numeric_arithmetic_value(
                left,
                right,
                span,
                "division",
                NumericValue::checked_div,
            ),
            HirBinaryOp::Rem => self.eval_numeric_arithmetic_value(
                left,
                right,
                span,
                "remainder",
                NumericValue::checked_rem,
            ),
            HirBinaryOp::AndAnd | HirBinaryOp::OrOr => Err(ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                span,
                "short-circuit binary operator reached non-short-circuit evaluator",
            )),
        }
    }

    pub(super) fn finish_binary_right(
        &self,
        op: HirBinaryOp,
        left: InterpValue,
        right: InterpValue,
        span: Span,
    ) -> Result<InterpValue, ExecutionFault> {
        match op {
            HirBinaryOp::AndAnd | HirBinaryOp::OrOr => {
                self.expect_bool(right, span).map(InterpValue::Bool)
            }
            _ => self.eval_binary_values(op, left, right, span),
        }
    }

    fn eval_numeric_comparison_value(
        &self,
        lhs: InterpValue,
        rhs: InterpValue,
        span: Span,
        cmp: impl FnOnce(std::cmp::Ordering) -> bool,
    ) -> Result<InterpValue, ExecutionFault> {
        match (lhs, rhs) {
            (InterpValue::Number(lhs), InterpValue::Number(rhs)) => lhs
                .partial_cmp_same(rhs)
                .map(|ordering| InterpValue::Bool(ordering.is_some_and(cmp)))
                .map_err(|error| numeric_fault(error, span, "comparison")),
            (lhs, rhs) => Err(ExecutionFault::new(
                AnalysisDiagnosticCode::InvalidArguments,
                span,
                format!(
                    "numeric comparison expects matching numeric values, got {:?} and {:?}",
                    lhs, rhs
                ),
            )),
        }
    }

    fn eval_numeric_arithmetic_value(
        &self,
        lhs: InterpValue,
        rhs: InterpValue,
        span: Span,
        operation: &'static str,
        op: impl FnOnce(NumericValue, NumericValue) -> Result<NumericValue, NumericError>,
    ) -> Result<InterpValue, ExecutionFault> {
        match (lhs, rhs) {
            (InterpValue::Number(lhs), InterpValue::Number(rhs)) => op(lhs, rhs)
                .map(InterpValue::Number)
                .map_err(|error| numeric_fault(error, span, operation)),
            (lhs, rhs) => Err(ExecutionFault::new(
                AnalysisDiagnosticCode::InvalidArguments,
                span,
                format!(
                    "numeric arithmetic expects matching numeric values, got {:?} and {:?}",
                    lhs, rhs
                ),
            )),
        }
    }

    fn eval_add_value(
        &self,
        lhs: InterpValue,
        rhs: InterpValue,
        span: Span,
    ) -> Result<InterpValue, ExecutionFault> {
        match (lhs, rhs) {
            (InterpValue::Number(lhs), InterpValue::Number(rhs)) => lhs
                .checked_add(rhs)
                .map(InterpValue::Number)
                .map_err(|error| numeric_fault(error, span, "addition")),
            (InterpValue::String(mut lhs), InterpValue::String(rhs)) => {
                lhs.push_str(&rhs);
                Ok(InterpValue::String(lhs))
            }
            (InterpValue::Array(lhs), InterpValue::Array(rhs)) => {
                let mut values = lhs.snapshot();
                values.extend(rhs.snapshot());
                Ok(InterpValue::Array(ArrayValue::new(values)))
            }
            (InterpValue::List(lhs), InterpValue::List(rhs)) => {
                let mut values = lhs.snapshot();
                values.extend(rhs.snapshot());
                Ok(InterpValue::List(ListValue::new(values)))
            }
            (lhs, rhs) => Err(ExecutionFault::new(
                AnalysisDiagnosticCode::InvalidArguments,
                span,
                format!(
                    "addition expects ints, strings, arrays, or lists, got {:?} and {:?}",
                    lhs, rhs
                ),
            )),
        }
    }
}

fn numeric_fault(error: NumericError, span: Span, operation: &str) -> ExecutionFault {
    let detail = match error {
        NumericError::TypeMismatch => "operands have different numeric types",
        NumericError::Overflow => "result overflows the checked numeric type",
        NumericError::DivisionByZero => "integer divisor is zero",
        NumericError::InvalidLiteral => "numeric literal is invalid",
    };
    ExecutionFault::new(
        AnalysisDiagnosticCode::InvalidArguments,
        span,
        format!("numeric {operation} failed: {detail}"),
    )
}
