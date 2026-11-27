use super::*;
use crate::control::ExecutionFault;
use crate::value::range::IntegerRange;

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
                        base: Box::new(base),
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
                    Continuation::SliceEnd {
                        eval,
                        base: Box::new(base),
                        start: Box::new(start),
                        frame: frame.clone(),
                    },
                );
            }
            other => return other,
        };
        self.eval_checked_slice_value(eval, base, start, end, frame)
    }

    pub(super) fn eval_checked_slice_value(
        &mut self,
        eval: SliceExprEval,
        base: InterpValue,
        start: InterpValue,
        end: InterpValue,
        frame: &Frame,
    ) -> ControlSignal {
        let diagnostic_base = base.clone();
        match self.eval_slice_value(
            eval.expr,
            base,
            start.clone(),
            end.clone(),
            eval.bounds,
            eval.span,
            frame,
        ) {
            Ok(value) => ControlSignal::Value(value),
            Err(fault) if fault.code == AnalysisDiagnosticCode::InvalidArguments => {
                let length = match &diagnostic_base {
                    InterpValue::Array(values) => Some(values.borrow().len()),
                    InterpValue::Slice(values) => Some(values.borrow().len()),
                    InterpValue::Bytes(values) => Some(values.len()),
                    _ => match crate::value::source_slice::SourceSliceView::checked(
                        &diagnostic_base,
                        &self.checked.types.known_std_types,
                        &self.checked.type_store,
                    ) {
                        Ok(view) => view.map(|view| view.length),
                        Err(message) => {
                            return ControlSignal::missing_checked_fact(message, eval.span);
                        }
                    },
                };
                match length {
                    Some(length) => {
                        let index = self
                            .slice_bounds(start, end, eval.bounds, eval.span)
                            .ok()
                            .map(|(start, end)| if start > end { start } else { end });
                        self.raise_index_error(eval.expr, index, length, "slice", eval.span)
                    }
                    None => ControlSignal::Fault(Box::new(fault)),
                }
            }
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
        frame: &Frame,
    ) -> Result<InterpValue, ExecutionFault> {
        let Some(fact) = self.checked.types.slice_facts.get(&expr) else {
            return Err(ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                span,
                "slice expression is missing its checked slice fact",
            ));
        };
        let source_slice = crate::value::source_slice::SourceSliceView::checked(
            &base,
            &self.checked.types.known_std_types,
            &self.checked.type_store,
        )
        .map_err(|message| {
            ExecutionFault::new(AnalysisDiagnosticCode::MissingCheckedFact, span, message)
        })?;
        let source_output = if self.checked.types.known_std_types.source_slice.is_some()
            && (matches!(&base, InterpValue::Array(_)) || source_slice.is_some())
        {
            let output = match fact {
                etas_types::CheckedSliceKind::Sequence { output, .. }
                | etas_types::CheckedSliceKind::Range { output, .. } => *output,
            };
            Some(self.materialized_type(
                output,
                &frame.bindings_at(self.checked, etas_hir::HirOwner::Expr(expr)),
                span,
            )?)
        } else {
            None
        };
        if let Some(view) = source_slice {
            let (start, end) = self.slice_bounds(start, end, bounds, span)?;
            if start > end || end > view.length {
                return Err(self.slice_bounds_fault("slice", span));
            }
            return Ok(crate::value::source_slice::SourceSliceView {
                source: view.source,
                start: view.start + start,
                length: end - start,
            }
            .into_value(source_output.expect("checked source Slice result")));
        }
        match base {
            InterpValue::Array(values) => {
                let (start, end) = self.slice_bounds(start, end, bounds, span)?;
                if let Some(ty) = source_output {
                    if start > end || end > values.borrow().len() {
                        return Err(self.slice_bounds_fault("array", span));
                    }
                    return Ok(crate::value::source_slice::SourceSliceView {
                        source: values,
                        start,
                        length: end - start,
                    }
                    .into_value(ty));
                }
                SliceValue::from_array(values, start..end)
                    .ok_or_else(|| self.slice_bounds_fault("array", span))
                    .map(InterpValue::Slice)
            }
            InterpValue::Slice(values) => {
                let (start, end) = self.slice_bounds(start, end, bounds, span)?;
                values
                    .slice(start..end)
                    .ok_or_else(|| self.slice_bounds_fault("slice", span))
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
                Ok(InterpValue::Bytes(values[start..end].to_vec().into()))
            }
            InterpValue::Range(range) => {
                let interval = IntegerRange::from_value(&range).map_err(|error| {
                    ExecutionFault::new(
                        AnalysisDiagnosticCode::MissingCheckedFact,
                        span,
                        format!("range slicing requires checked integer bounds: {error:?}"),
                    )
                })?;
                let (start, end) = self.slice_bounds(start, end, bounds, span)?;
                interval
                    .slice(start, end)
                    .map(InterpValue::Range)
                    .map_err(|error| {
                        ExecutionFault::new(
                            AnalysisDiagnosticCode::InvalidArguments,
                            span,
                            format!("range slice bounds are out of range at runtime: {error:?}"),
                        )
                    })
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

    fn slice_bounds_fault(&self, label: &str, span: Span) -> ExecutionFault {
        ExecutionFault::new(
            AnalysisDiagnosticCode::InvalidArguments,
            span,
            format!("{label} slice bounds are out of range at runtime"),
        )
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
