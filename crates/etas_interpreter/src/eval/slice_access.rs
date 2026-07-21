use super::*;
use crate::control::ExecutionFault;
use crate::value::{RangeBounds, RangeValue};

impl<'a> EvalContext<'a> {
    pub(super) fn eval_slice_expr(
        &mut self,
        eval: SliceExprEval,
        frame: &mut Frame,
    ) -> ControlSignal {
        let base = match self.eval_expr(eval.base, frame) {
            ControlSignal::Value(value) => value,
            signal if is_pending_host_boundary_signal(&signal) => {
                return compose_signal_continuation(
                    signal,
                    Continuation::SliceBase {
                        eval,
                        frame: frame.clone(),
                    },
                );
            }
            other => return other,
        };
        self.resume_slice_base(eval, base, frame)
    }

    pub(super) fn resume_slice_base(
        &mut self,
        eval: SliceExprEval,
        base: InterpValue,
        frame: &mut Frame,
    ) -> ControlSignal {
        let start = match self.eval_expr(eval.start, frame) {
            ControlSignal::Value(value) => value,
            signal if is_pending_host_boundary_signal(&signal) => {
                return compose_signal_continuation(
                    signal,
                    Continuation::SliceStart {
                        eval,
                        base,
                        frame: frame.clone(),
                    },
                );
            }
            other => return other,
        };
        self.resume_slice_start(eval, base, start, frame)
    }

    pub(super) fn resume_slice_start(
        &mut self,
        eval: SliceExprEval,
        base: InterpValue,
        start: InterpValue,
        frame: &mut Frame,
    ) -> ControlSignal {
        let end = match self.eval_expr(eval.end, frame) {
            ControlSignal::Value(value) => value,
            signal if is_pending_host_boundary_signal(&signal) => {
                return compose_signal_continuation(
                    signal,
                    Continuation::SliceEnd { eval, base, start },
                );
            }
            other => return other,
        };
        match self.eval_slice_value(eval.expr, base, start, end, eval.bounds, eval.span) {
            Ok(value) => ControlSignal::Value(value),
            Err(fault) => ControlSignal::Fault(Box::new(fault)),
        }
    }

    pub(super) fn eval_slice_value(
        &self,
        expr: HirExprId,
        base: InterpValue,
        start: InterpValue,
        end: InterpValue,
        bounds: etas_hir::HirRangeBounds,
        span: Span,
    ) -> Result<InterpValue, ExecutionFault> {
        if !self.checked.types.slice_facts.contains_key(&expr) {
            return Err(ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                span,
                "slice expression is missing its checked slice fact",
            ));
        }
        match base {
            InterpValue::Array(values) => {
                let (start, end) = self.slice_bounds(start, end, bounds, span)?;
                let values = values.borrow();
                self.slice_sequence(values.as_slice(), start, end, "array", span)
                    .map(SliceValue::new)
                    .map(InterpValue::Slice)
            }
            InterpValue::Slice(values) => {
                let (start, end) = self.slice_bounds(start, end, bounds, span)?;
                let values = values.borrow();
                self.slice_sequence(values.as_slice(), start, end, "slice", span)
                    .map(SliceValue::new)
                    .map(InterpValue::Slice)
            }
            InterpValue::Bytes(values) => {
                let (start, end) = self.slice_bounds(start, end, bounds, span)?;
                if start > end || end > values.len() {
                    return Err(ExecutionFault::new(
                        AnalysisDiagnosticCode::InvalidArguments,
                        span,
                        "bytes slice bounds are out of range at runtime",
                    ));
                }
                Ok(InterpValue::Bytes(values[start..end].to_vec()))
            }
            InterpValue::Range(range) => {
                let range_start = match *range.start {
                    InterpValue::Number(value) => value,
                    _ => {
                        return Err(ExecutionFault::new(
                            AnalysisDiagnosticCode::MissingCheckedFact,
                            span,
                            "range slicing requires integer range bounds",
                        ));
                    }
                };
                let range_end = match *range.end {
                    InterpValue::Number(value) => value,
                    _ => {
                        return Err(ExecutionFault::new(
                            AnalysisDiagnosticCode::MissingCheckedFact,
                            span,
                            "range slicing requires integer range bounds",
                        ));
                    }
                };
                let values = self.range_values(range_start, range_end, range.bounds, span)?;
                let (start, end) = self.slice_bounds(start, end, bounds, span)?;
                let sliced = self.slice_sequence(values.as_slice(), start, end, "range", span)?;
                let (Some(InterpValue::Number(start)), Some(InterpValue::Number(end))) =
                    (sliced.first().cloned(), sliced.last().cloned())
                else {
                    return Ok(InterpValue::Range(RangeValue {
                        start: Box::new(InterpValue::Number(range_start)),
                        end: Box::new(InterpValue::Number(range_start)),
                        bounds: RangeBounds::ClosedOpen,
                    }));
                };
                let exclusive_end = increment_range_number(end, span)?;
                Ok(InterpValue::Range(RangeValue {
                    start: Box::new(InterpValue::Number(start)),
                    end: Box::new(InterpValue::Number(exclusive_end)),
                    bounds: RangeBounds::ClosedOpen,
                }))
            }
            other => Err(ExecutionFault::new(
                AnalysisDiagnosticCode::InvalidArguments,
                span,
                format!(
                    "slice evaluation expects an array, slice, bytes, or range value, got {:?}",
                    other
                ),
            )),
        }
    }

    fn slice_sequence(
        &self,
        values: &[InterpValue],
        start: usize,
        end: usize,
        label: &str,
        span: Span,
    ) -> Result<Vec<InterpValue>, ExecutionFault> {
        if start > end || end > values.len() {
            return Err(ExecutionFault::new(
                AnalysisDiagnosticCode::InvalidArguments,
                span,
                format!("{label} slice bounds are out of range at runtime"),
            ));
        }
        Ok(values[start..end].to_vec())
    }

    fn slice_bounds(
        &self,
        start: InterpValue,
        end: InterpValue,
        bounds: etas_hir::HirRangeBounds,
        span: Span,
    ) -> Result<(usize, usize), ExecutionFault> {
        let start = self.index_usize(start, span).ok_or_else(|| {
            ExecutionFault::new(
                AnalysisDiagnosticCode::InvalidArguments,
                span,
                "slice start bound is not a non-negative integer",
            )
        })?;
        let end = self.index_usize(end, span).ok_or_else(|| {
            ExecutionFault::new(
                AnalysisDiagnosticCode::InvalidArguments,
                span,
                "slice end bound is not a non-negative integer",
            )
        })?;
        match bounds {
            etas_hir::HirRangeBounds::ClosedOpen => Ok((start, end)),
            etas_hir::HirRangeBounds::OpenClosed => {
                let Some(start) = start.checked_add(1) else {
                    return Err(ExecutionFault::new(
                        AnalysisDiagnosticCode::InvalidArguments,
                        span,
                        "slice start bound overflowed at runtime",
                    ));
                };
                let Some(end) = end.checked_add(1) else {
                    return Err(ExecutionFault::new(
                        AnalysisDiagnosticCode::InvalidArguments,
                        span,
                        "slice end bound overflowed at runtime",
                    ));
                };
                Ok((start, end))
            }
        }
    }

    fn range_values(
        &self,
        start: crate::value::NumericValue,
        end: crate::value::NumericValue,
        bounds: RangeBounds,
        span: Span,
    ) -> Result<Vec<InterpValue>, ExecutionFault> {
        let mut values = Vec::new();
        let mut current = match bounds {
            RangeBounds::ClosedClosed | RangeBounds::ClosedOpen => start,
            RangeBounds::OpenOpen | RangeBounds::OpenClosed => {
                if !compare_range_numbers(start, end, |ordering| ordering.is_lt(), span)? {
                    return Ok(values);
                }
                increment_range_number(start, span)?
            }
        };
        loop {
            let in_bounds = match bounds {
                RangeBounds::ClosedClosed | RangeBounds::OpenClosed => {
                    compare_range_numbers(current, end, |ordering| ordering.is_le(), span)?
                }
                RangeBounds::ClosedOpen | RangeBounds::OpenOpen => {
                    compare_range_numbers(current, end, |ordering| ordering.is_lt(), span)?
                }
            };
            if !in_bounds {
                break;
            }
            values.push(InterpValue::Number(current));
            current = increment_range_number(current, span)?;
        }
        Ok(values)
    }
}

fn increment_range_number(
    value: crate::value::NumericValue,
    span: Span,
) -> Result<crate::value::NumericValue, ExecutionFault> {
    value
        .one_same()
        .and_then(|one| value.checked_add(one))
        .map_err(|_| {
            ExecutionFault::new(
                AnalysisDiagnosticCode::InvalidArguments,
                span,
                "range slicing overflowed its integer bounds",
            )
        })
}

fn compare_range_numbers(
    lhs: crate::value::NumericValue,
    rhs: crate::value::NumericValue,
    cmp: impl FnOnce(std::cmp::Ordering) -> bool,
    span: Span,
) -> Result<bool, ExecutionFault> {
    lhs.partial_cmp_same(rhs)
        .map(|ordering| ordering.is_some_and(cmp))
        .map_err(|_| {
            ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                span,
                "range bounds must have the same checked integer type",
            )
        })
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct SliceExprEval {
    pub expr: HirExprId,
    pub base: HirExprId,
    pub start: HirExprId,
    pub end: HirExprId,
    pub bounds: etas_hir::HirRangeBounds,
    pub span: Span,
}
