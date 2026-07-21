use super::*;
use crate::control::ExecutionFault;
use crate::value::RangeBounds;
use etas_std::StdLimitKind;
use std::collections::HashSet;

pub(super) struct ForLoopResume {
    pub pat: etas_hir::HirPatId,
    pub values: Option<Vec<InterpValue>>,
    pub next_index: usize,
    pub body: HirBlockId,
    pub iterations: usize,
    pub loop_scope: HashSet<SymbolId>,
    pub span: Span,
}

pub(super) struct WhileLoopResume {
    pub value: InterpValue,
    pub cond: HirExprId,
    pub body: HirBlockId,
    pub iteration: u32,
    pub max_iterations: u32,
    pub resume_after_body: bool,
    pub span: Span,
}

impl<'a> EvalContext<'a> {
    pub(super) fn execute_for_loop(
        &mut self,
        pat: etas_hir::HirPatId,
        iter: HirExprId,
        limits: &[HirExprId],
        body: HirBlockId,
        span: Span,
        frame: &mut Frame,
    ) -> ControlSignal {
        let iterations = match self.loop_iterations(limits, span) {
            Ok(iterations) => iterations as usize,
            Err(fault) => return ControlSignal::Fault(Box::new(fault)),
        };
        let loop_scope = frame.snapshot_symbols();
        let iterable = match self.eval_expr(iter, frame) {
            ControlSignal::Value(value) => value,
            signal @ (ControlSignal::Apply(_)
            | ControlSignal::Checkpoint(_)
            | ControlSignal::Block(_)
            | ControlSignal::Expr(_)
            | ControlSignal::Call(_)
            | ControlSignal::Perform(_)
            | ControlSignal::Memory(_)
            | ControlSignal::Session(_)
            | ControlSignal::Console(_)
            | ControlSignal::Command(_)
            | ControlSignal::Model(_)
            | ControlSignal::Host(_)) => {
                return compose_signal_continuation(
                    signal,
                    Continuation::ForLoop {
                        pat,
                        values: None,
                        next_index: 0,
                        body,
                        iterations,
                        loop_scope,
                        span,
                        frame: frame.clone(),
                    },
                );
            }
            ControlSignal::Return(value) => return ControlSignal::Return(value),
            ControlSignal::Resume(value) => return ControlSignal::Resume(value),
            ControlSignal::Finish(value) => return ControlSignal::Finish(value),
            ControlSignal::Break => return ControlSignal::Break,
            ControlSignal::Fault(fault) => return ControlSignal::Fault(fault),
            ControlSignal::Continue => return ControlSignal::Continue,
        };
        self.resume_for_loop(
            ForLoopResume {
                pat,
                values: None,
                next_index: 0,
                body,
                iterations,
                loop_scope,
                span,
            },
            iterable,
            frame,
        )
    }

    pub(super) fn resume_for_loop(
        &mut self,
        mut state: ForLoopResume,
        iterable_or_body_value: InterpValue,
        frame: &mut Frame,
    ) -> ControlSignal {
        let values = match state.values.take() {
            Some(values) => values,
            None => {
                match self.iterable_values(iterable_or_body_value, state.iterations, state.span) {
                    Ok(values) => values,
                    Err(fault) => return ControlSignal::Fault(Box::new(fault)),
                }
            }
        };
        self.continue_for_loop(state, values, frame)
    }

    fn continue_for_loop(
        &mut self,
        state: ForLoopResume,
        values: Vec<InterpValue>,
        frame: &mut Frame,
    ) -> ControlSignal {
        let ForLoopResume {
            pat,
            values: _,
            next_index,
            body,
            iterations,
            loop_scope,
            span,
        } = state;
        let end = values.len().min(iterations);
        let mut index = next_index;
        while index < end {
            let value = values[index].clone();
            if let Err(fault) = self.bind_pattern(pat, value, frame, span) {
                frame.cleanup_to(&loop_scope);
                return ControlSignal::Fault(Box::new(fault));
            }
            match self.execute_block(body, frame) {
                ControlSignal::Value(_) => {}
                ControlSignal::Break => {
                    frame.cleanup_to(&loop_scope);
                    return ControlSignal::Value(InterpValue::Unit);
                }
                ControlSignal::Continue => continue,
                ControlSignal::Return(value) => {
                    frame.cleanup_to(&loop_scope);
                    return ControlSignal::Return(value);
                }
                ControlSignal::Resume(value) => {
                    frame.cleanup_to(&loop_scope);
                    return ControlSignal::Resume(value);
                }
                ControlSignal::Finish(value) => {
                    frame.cleanup_to(&loop_scope);
                    return ControlSignal::Finish(value);
                }
                ControlSignal::Fault(fault) => {
                    frame.cleanup_to(&loop_scope);
                    return ControlSignal::Fault(fault);
                }
                signal @ (ControlSignal::Apply(_)
                | ControlSignal::Memory(_)
                | ControlSignal::Session(_)
                | ControlSignal::Checkpoint(_)
                | ControlSignal::Block(_)
                | ControlSignal::Expr(_)
                | ControlSignal::Call(_)
                | ControlSignal::Perform(_)
                | ControlSignal::Console(_)
                | ControlSignal::Command(_)
                | ControlSignal::Model(_)
                | ControlSignal::Host(_)) => {
                    return self.wrap_loop_body_signal(
                        signal,
                        Continuation::ForLoop {
                            pat,
                            values: Some(values),
                            next_index: index + 1,
                            body,
                            iterations,
                            loop_scope,
                            span,
                            frame: frame.clone(),
                        },
                    );
                }
            }
            index += 1;
        }
        frame.cleanup_to(&loop_scope);
        ControlSignal::Value(InterpValue::Unit)
    }

    fn wrap_loop_body_signal(
        &mut self,
        signal: ControlSignal,
        continuation: Continuation,
    ) -> ControlSignal {
        compose_signal_continuation(signal, continuation)
    }

    fn iterable_values(
        &mut self,
        iterable: InterpValue,
        iterations: usize,
        span: Span,
    ) -> Result<Vec<InterpValue>, ExecutionFault> {
        match iterable {
            InterpValue::Array(values) => Ok(values.snapshot()),
            InterpValue::List(values) => Ok(values.snapshot()),
            InterpValue::Slice(values) => Ok(values.snapshot()),
            InterpValue::Set(values) => Ok(values.snapshot()),
            InterpValue::Deque(values)
            | InterpValue::Queue(values)
            | InterpValue::Stack(values) => Ok(values.snapshot()),
            InterpValue::PriorityQueue(entries) | InterpValue::OrderedMap(entries) => Ok(entries
                .snapshot()
                .into_iter()
                .map(|(key, value)| InterpValue::Tuple(vec![key, value]))
                .collect()),
            InterpValue::OrderedSet(values) => Ok(values.snapshot()),
            InterpValue::Range(range) => {
                let (InterpValue::Number(start), InterpValue::Number(end)) =
                    (*range.start, *range.end)
                else {
                    return Err(ExecutionFault::new(
                        AnalysisDiagnosticCode::MissingCheckedFact,
                        span,
                        "range iteration requires integer range bounds",
                    ));
                };
                self.range_iteration_values(start, end, range.bounds, iterations, span)
            }
            other => Err(ExecutionFault::new(
                AnalysisDiagnosticCode::InvalidArguments,
                span,
                format!("for iteration requires a local collection or range value, got {other:?}"),
            )),
        }
    }

    fn range_iteration_values(
        &self,
        start: crate::value::NumericValue,
        end: crate::value::NumericValue,
        bounds: RangeBounds,
        iterations: usize,
        span: Span,
    ) -> Result<Vec<InterpValue>, ExecutionFault> {
        let mut values = Vec::new();
        let mut current = match bounds {
            RangeBounds::ClosedClosed | RangeBounds::ClosedOpen => start,
            RangeBounds::OpenOpen | RangeBounds::OpenClosed => {
                if !numeric_range_cmp(start, end, |ordering| ordering.is_lt(), span)? {
                    return Ok(values);
                }
                numeric_range_increment(start, span)?
            }
        };
        while values.len() < iterations {
            let in_bounds = match bounds {
                RangeBounds::ClosedClosed | RangeBounds::OpenClosed => {
                    numeric_range_cmp(current, end, |ordering| ordering.is_le(), span)?
                }
                RangeBounds::ClosedOpen | RangeBounds::OpenOpen => {
                    numeric_range_cmp(current, end, |ordering| ordering.is_lt(), span)?
                }
            };
            if !in_bounds {
                break;
            }
            values.push(InterpValue::Number(current));
            current = match numeric_range_increment(current, span) {
                Ok(value) => value,
                Err(_) if values.len() == iterations => current,
                Err(fault) => return Err(fault),
            };
        }
        Ok(values)
    }

    pub(super) fn execute_while_loop(
        &mut self,
        cond: HirExprId,
        limits: &[HirExprId],
        body: HirBlockId,
        span: Span,
        frame: &mut Frame,
    ) -> ControlSignal {
        let iterations = match self.loop_iterations(limits, span) {
            Ok(iterations) => iterations,
            Err(fault) => return ControlSignal::Fault(Box::new(fault)),
        };
        self.continue_while_loop(cond, body, 0, iterations, span, frame)
    }

    fn continue_while_loop(
        &mut self,
        cond: HirExprId,
        body: HirBlockId,
        start_iteration: u32,
        max_iterations: u32,
        span: Span,
        frame: &mut Frame,
    ) -> ControlSignal {
        for iteration in start_iteration..max_iterations {
            let condition = match self.eval_expr(cond, frame) {
                ControlSignal::Value(value) => value,
                signal @ (ControlSignal::Apply(_)
                | ControlSignal::Checkpoint(_)
                | ControlSignal::Block(_)
                | ControlSignal::Expr(_)
                | ControlSignal::Call(_)
                | ControlSignal::Perform(_)
                | ControlSignal::Memory(_)
                | ControlSignal::Session(_)
                | ControlSignal::Console(_)
                | ControlSignal::Command(_)
                | ControlSignal::Model(_)
                | ControlSignal::Host(_)) => {
                    return compose_signal_continuation(
                        signal,
                        Continuation::WhileLoop {
                            cond,
                            body,
                            iteration,
                            max_iterations,
                            resume_after_body: false,
                            span,
                            frame: frame.clone(),
                        },
                    );
                }
                ControlSignal::Return(value) => return ControlSignal::Return(value),
                ControlSignal::Resume(value) => return ControlSignal::Resume(value),
                ControlSignal::Finish(value) => return ControlSignal::Finish(value),
                ControlSignal::Fault(fault) => return ControlSignal::Fault(fault),
                ControlSignal::Break => return ControlSignal::Break,
                ControlSignal::Continue => return ControlSignal::Continue,
            };
            let condition = match self.expect_bool(condition, span) {
                Ok(condition) => condition,
                Err(fault) => return ControlSignal::Fault(Box::new(fault)),
            };
            if !condition {
                return ControlSignal::Value(InterpValue::Unit);
            }
            match self.execute_block(body, frame) {
                ControlSignal::Value(_) => {}
                ControlSignal::Break => return ControlSignal::Value(InterpValue::Unit),
                ControlSignal::Continue => continue,
                ControlSignal::Return(value) => return ControlSignal::Return(value),
                ControlSignal::Resume(value) => return ControlSignal::Resume(value),
                ControlSignal::Finish(value) => return ControlSignal::Finish(value),
                ControlSignal::Fault(fault) => return ControlSignal::Fault(fault),
                signal @ (ControlSignal::Apply(_)
                | ControlSignal::Memory(_)
                | ControlSignal::Session(_)
                | ControlSignal::Checkpoint(_)
                | ControlSignal::Block(_)
                | ControlSignal::Expr(_)
                | ControlSignal::Call(_)
                | ControlSignal::Perform(_)
                | ControlSignal::Console(_)
                | ControlSignal::Command(_)
                | ControlSignal::Model(_)
                | ControlSignal::Host(_)) => {
                    return self.wrap_loop_body_signal(
                        signal,
                        Continuation::WhileLoop {
                            cond,
                            body,
                            iteration,
                            max_iterations,
                            resume_after_body: true,
                            span,
                            frame: frame.clone(),
                        },
                    );
                }
            }
        }
        ControlSignal::Value(InterpValue::Unit)
    }

    pub(super) fn resume_while_loop(
        &mut self,
        state: WhileLoopResume,
        frame: &mut Frame,
    ) -> ControlSignal {
        if state.resume_after_body {
            return self.continue_while_loop(
                state.cond,
                state.body,
                state.iteration.saturating_add(1),
                state.max_iterations,
                state.span,
                frame,
            );
        }
        let condition = match self.expect_bool(state.value, state.span) {
            Ok(condition) => condition,
            Err(fault) => return ControlSignal::Fault(Box::new(fault)),
        };
        if !condition {
            return ControlSignal::Value(InterpValue::Unit);
        }
        match self.execute_block(state.body, frame) {
            ControlSignal::Value(_) | ControlSignal::Continue => self.continue_while_loop(
                state.cond,
                state.body,
                state.iteration.saturating_add(1),
                state.max_iterations,
                state.span,
                frame,
            ),
            ControlSignal::Break => ControlSignal::Value(InterpValue::Unit),
            ControlSignal::Return(value) => ControlSignal::Return(value),
            ControlSignal::Resume(value) => ControlSignal::Resume(value),
            ControlSignal::Finish(value) => ControlSignal::Finish(value),
            ControlSignal::Fault(fault) => ControlSignal::Fault(fault),
            signal @ (ControlSignal::Apply(_)
            | ControlSignal::Memory(_)
            | ControlSignal::Session(_)
            | ControlSignal::Checkpoint(_)
            | ControlSignal::Block(_)
            | ControlSignal::Expr(_)
            | ControlSignal::Call(_)
            | ControlSignal::Perform(_)
            | ControlSignal::Console(_)
            | ControlSignal::Command(_)
            | ControlSignal::Model(_)
            | ControlSignal::Host(_)) => self.wrap_loop_body_signal(
                signal,
                Continuation::WhileLoop {
                    cond: state.cond,
                    body: state.body,
                    iteration: state.iteration,
                    max_iterations: state.max_iterations,
                    resume_after_body: true,
                    span: state.span,
                    frame: frame.clone(),
                },
            ),
        }
    }

    pub(super) fn retry_attempts(
        &self,
        limits: &[HirExprId],
        span: Span,
    ) -> Result<u32, ExecutionFault> {
        for limit in limits {
            let limit = self.resolve_limit_expr(*limit)?;
            if let (
                StdLimitKind::Attempts,
                crate::eval::limit::RuntimeLimitValue::Count(attempts),
            ) = (limit.kind, limit.value)
            {
                return u32::try_from(attempts.max(1)).map_err(|_| {
                    ExecutionFault::new(
                        AnalysisDiagnosticCode::InvalidArguments,
                        limit.span,
                        "Attempts(...) limit exceeds the interpreter retry counter range",
                    )
                });
            }
        }
        Err(ExecutionFault::new(
            AnalysisDiagnosticCode::InvalidArguments,
            span,
            "retry execution requires an Attempts(...) limit",
        ))
    }

    fn loop_iterations(&self, limits: &[HirExprId], span: Span) -> Result<u32, ExecutionFault> {
        for limit in limits {
            let limit = self.resolve_limit_expr(*limit)?;
            if let (
                StdLimitKind::Iterations,
                crate::eval::limit::RuntimeLimitValue::Count(iterations),
            ) = (limit.kind, limit.value)
            {
                return u32::try_from(iterations.max(1)).map_err(|_| {
                    ExecutionFault::new(
                        AnalysisDiagnosticCode::InvalidArguments,
                        limit.span,
                        "Iterations(...) limit exceeds the interpreter loop counter range",
                    )
                });
            }
        }
        Err(ExecutionFault::new(
            AnalysisDiagnosticCode::InvalidArguments,
            span,
            "loop execution requires an Iterations(...) limit",
        ))
    }
}

fn numeric_range_increment(
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
                "range iteration overflowed its integer bounds",
            )
        })
}

fn numeric_range_cmp(
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
