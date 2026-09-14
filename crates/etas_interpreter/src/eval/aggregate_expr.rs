use super::*;
use crate::value::{RangeBounds, RangeValue};

enum AggregateKind {
    Tuple,
    Array,
    List,
    Set,
}

impl<'a> EvalContext<'a> {
    pub(super) fn eval_sequence_expr(
        &mut self,
        expr: HirExprId,
        frame: &mut Frame,
    ) -> ControlSignal {
        self.resume_expr_sequence(expr, 0, Vec::new(), frame)
    }

    pub(super) fn eval_list_cons_expr(
        &mut self,
        head: HirExprId,
        tail: HirExprId,
        span: Span,
        frame: &mut Frame,
    ) -> ControlSignal {
        match self.eval_expr(head, frame) {
            ControlSignal::Value(head) => self.resume_list_cons_head(head, tail, span, frame),
            signal if is_pending_host_boundary_signal(&signal) => compose_signal_continuation(
                signal,
                Continuation::ListConsHead {
                    tail,
                    span,
                    frame: frame.clone(),
                },
            ),
            other => other,
        }
    }

    pub(super) fn resume_list_cons_head(
        &mut self,
        head: InterpValue,
        tail: HirExprId,
        span: Span,
        frame: &mut Frame,
    ) -> ControlSignal {
        let tail = match self.eval_expr(tail, frame) {
            ControlSignal::Value(value) => value,
            signal if is_pending_host_boundary_signal(&signal) => {
                return compose_signal_continuation(
                    signal,
                    Continuation::ListConsTail { head, span },
                );
            }
            other => return other,
        };
        self.finish_list_cons(head, tail, span)
    }

    pub(super) fn finish_list_cons(
        &mut self,
        head: InterpValue,
        tail: InterpValue,
        span: Span,
    ) -> ControlSignal {
        let InterpValue::List(mut values) = tail else {
            return ControlSignal::missing_checked_fact(
                "list cons tail must evaluate to a List[T]",
                span,
            );
        };
        values.push_front(head);
        ControlSignal::Value(InterpValue::List(values))
    }

    pub(super) fn eval_range_expr(
        &mut self,
        start: HirExprId,
        end: HirExprId,
        bounds: etas_hir::HirRangeBounds,
        frame: &mut Frame,
    ) -> ControlSignal {
        match self.eval_expr(start, frame) {
            ControlSignal::Value(start) => self.resume_range_start(start, end, bounds, frame),
            signal if is_pending_host_boundary_signal(&signal) => compose_signal_continuation(
                signal,
                Continuation::RangeStart {
                    end,
                    bounds,
                    frame: frame.clone(),
                },
            ),
            other => other,
        }
    }

    pub(super) fn resume_range_start(
        &mut self,
        start: InterpValue,
        end: HirExprId,
        bounds: etas_hir::HirRangeBounds,
        frame: &mut Frame,
    ) -> ControlSignal {
        let end = match self.eval_expr(end, frame) {
            ControlSignal::Value(value) => value,
            signal if is_pending_host_boundary_signal(&signal) => {
                return compose_signal_continuation(
                    signal,
                    Continuation::RangeEnd { start, bounds },
                );
            }
            other => return other,
        };
        self.finish_range(start, end, bounds)
    }

    pub(super) fn finish_range(
        &mut self,
        start: InterpValue,
        end: InterpValue,
        bounds: etas_hir::HirRangeBounds,
    ) -> ControlSignal {
        let bounds = match bounds {
            etas_hir::HirRangeBounds::ClosedOpen => RangeBounds::ClosedOpen,
            etas_hir::HirRangeBounds::OpenClosed => RangeBounds::OpenClosed,
        };
        ControlSignal::Value(InterpValue::Range(RangeValue {
            start: Box::new(start),
            end: Box::new(end),
            bounds,
        }))
    }

    pub(super) fn eval_empty_sequence_expr(
        &mut self,
        expr: HirExprId,
        span: Span,
    ) -> ControlSignal {
        let Some(ty) = self.checked.types.expr_types.get(&expr) else {
            return ControlSignal::missing_checked_fact(
                "empty sequence expression is missing its checked type",
                span,
            );
        };
        match self.checked.type_store.get(*ty) {
            Some(etas_types::Type::Array(_)) => {
                ControlSignal::Value(InterpValue::Array(ArrayValue::new(Vec::new())))
            }
            Some(etas_types::Type::List(_)) => {
                ControlSignal::Value(InterpValue::List(Vec::new().into()))
            }
            _ => ControlSignal::missing_checked_fact(
                "empty sequence expression must be checked as Array[T] or List[T]",
                span,
            ),
        }
    }

    pub(super) fn eval_lambda_expr(&mut self, expr: HirExprId, frame: &mut Frame) -> ControlSignal {
        let span = self.checked.hir.exprs[expr].span(&self.checked.hir.blocks);
        let Some(layout) = self.plan.closures.get(expr) else {
            return ControlSignal::missing_checked_fact(
                "lambda is missing its checked capture layout",
                span,
            );
        };
        let captured = match frame.capture(layout.slots.clone(), &layout.captures) {
            Ok(captured) => captured,
            Err(message) => return ControlSignal::missing_checked_fact(message, span),
        };
        ControlSignal::Value(InterpValue::Callable(CallTarget::Lambda { expr, captured }))
    }

    pub(super) fn eval_record_expr(
        &mut self,
        expr: HirExprId,
        record: &etas_hir::HirRecordExpr,
        frame: &mut Frame,
    ) -> ControlSignal {
        self.resume_record_fields(expr, 0, Vec::with_capacity(record.fields.len()), frame)
    }

    pub(super) fn resume_record_fields(
        &mut self,
        expr: HirExprId,
        start_index: usize,
        mut values: Vec<(String, InterpValue)>,
        frame: &mut Frame,
    ) -> ControlSignal {
        let Some(HirExpr::Record(record)) = self.checked.hir.exprs.get(expr) else {
            return ControlSignal::missing_checked_fact(
                "record continuation is missing its checked construction expression",
                item_span(self.checked, self.entry_item),
            );
        };
        let nominal_type = if record.path.is_some() {
            let Some(ty) = self.checked.types.expr_types.get(&expr).copied() else {
                return ControlSignal::missing_checked_fact(
                    "nominal record constructor is missing its checked result type",
                    record.span,
                );
            };
            Some(ty)
        } else {
            None
        };
        let variant_symbol = self.named_variant_symbol(record.path.as_ref());
        values.reserve(record.fields.len().saturating_sub(values.len()));
        for (index, field) in record.fields.iter().enumerate().skip(start_index) {
            match field {
                etas_hir::HirFieldInit::Shorthand {
                    name,
                    resolution,
                    span,
                } => match self.eval_path_signal(resolution.clone(), *span, frame) {
                    ControlSignal::Value(value) => values.push((name.clone(), value)),
                    other => return other,
                },
                etas_hir::HirFieldInit::Named { name, value, .. } => {
                    match self.eval_expr(*value, frame) {
                        ControlSignal::Value(value) => values.push((name.clone(), value)),
                        signal if is_pending_host_boundary_signal(&signal) => {
                            return compose_signal_continuation(
                                signal,
                                Continuation::RecordField {
                                    expr,
                                    nominal_type,
                                    variant_symbol,
                                    next_index: index + 1,
                                    values,
                                    frame: frame.clone(),
                                },
                            );
                        }
                        other => return other,
                    }
                }
            }
        }
        if let Some(symbol) = variant_symbol {
            return match self.eval_named_variant(expr, symbol, values, record.span) {
                Ok(value) => ControlSignal::Value(value),
                Err(fault) => ControlSignal::Fault(Box::new(fault)),
            };
        }
        let value = match self.plan.records.construct(expr, values) {
            Ok(value) => value,
            Err(message) => return ControlSignal::missing_checked_fact(message, record.span),
        };
        ControlSignal::Value(match nominal_type {
            Some(ty) => InterpValue::Nominal {
                ty,
                value: crate::value::SharedValue::new(value),
            },
            None => value,
        })
    }

    pub(super) fn eval_map_expr(&mut self, expr: HirExprId, frame: &mut Frame) -> ControlSignal {
        self.resume_map_entries(expr, 0, Vec::new(), frame)
    }

    pub(super) fn resume_map_entries(
        &mut self,
        expr: HirExprId,
        start_index: usize,
        mut values: Vec<(InterpValue, InterpValue)>,
        frame: &mut Frame,
    ) -> ControlSignal {
        let Some(HirExpr::Map { entries, .. }) = self.checked.hir.exprs.get(expr) else {
            return ControlSignal::missing_checked_fact(
                "map continuation is missing its checked construction expression",
                item_span(self.checked, self.entry_item),
            );
        };
        values.reserve(entries.len().saturating_sub(values.len()));
        for (index, entry) in entries.iter().enumerate().skip(start_index) {
            let key = match self.eval_expr(entry.key, frame) {
                ControlSignal::Value(value) => value,
                signal if is_pending_host_boundary_signal(&signal) => {
                    return compose_signal_continuation(
                        signal,
                        Continuation::MapKey {
                            expr,
                            index,
                            values,
                            frame: frame.clone(),
                        },
                    );
                }
                other => return other,
            };
            let value = match self.eval_expr(entry.value, frame) {
                ControlSignal::Value(value) => value,
                signal if is_pending_host_boundary_signal(&signal) => {
                    return compose_signal_continuation(
                        signal,
                        Continuation::MapValue {
                            expr,
                            index,
                            key,
                            values,
                            frame: frame.clone(),
                        },
                    );
                }
                other => return other,
            };
            values.push((key, value));
        }
        ControlSignal::Value(InterpValue::Map(MapValue::new(values)))
    }

    pub(super) fn resume_map_key_value(
        &mut self,
        expr: HirExprId,
        index: usize,
        values: Vec<(InterpValue, InterpValue)>,
        key: InterpValue,
        frame: &mut Frame,
    ) -> ControlSignal {
        let Some(HirExpr::Map { entries, .. }) = self.checked.hir.exprs.get(expr) else {
            return ControlSignal::missing_checked_fact(
                "map continuation is missing its checked construction expression",
                item_span(self.checked, self.entry_item),
            );
        };
        let Some(entry) = entries.get(index) else {
            return ControlSignal::invalid_arguments(
                "map key continuation index is out of range",
                item_span(self.checked, self.entry_item),
            );
        };
        match self.eval_expr(entry.value, frame) {
            ControlSignal::Value(value) => {
                self.resume_map_value(expr, index, values, key, value, frame)
            }
            signal if is_pending_host_boundary_signal(&signal) => compose_signal_continuation(
                signal,
                Continuation::MapValue {
                    expr,
                    index,
                    key,
                    values,
                    frame: frame.clone(),
                },
            ),
            other => other,
        }
    }

    pub(super) fn resume_map_value(
        &mut self,
        expr: HirExprId,
        index: usize,
        mut values: Vec<(InterpValue, InterpValue)>,
        key: InterpValue,
        value: InterpValue,
        frame: &mut Frame,
    ) -> ControlSignal {
        values.push((key, value));
        self.resume_map_entries(expr, index + 1, values, frame)
    }

    pub(super) fn eval_empty_record_or_map_expr(
        &mut self,
        expr: HirExprId,
        span: Span,
    ) -> ControlSignal {
        if !self.checked.types.expr_types.contains_key(&expr) {
            return ControlSignal::missing_checked_fact(
                "empty brace literal is missing its checked type",
                span,
            );
        };
        match self.plan.dispatch.brace_literal_shape(expr) {
            Some(BraceLiteralShape::Record) => ControlSignal::Value(InterpValue::Record(
                Vec::<(String, InterpValue)>::new().into(),
            )),
            Some(BraceLiteralShape::Map) => {
                ControlSignal::Value(InterpValue::Map(MapValue::new(Vec::new())))
            }
            None => ControlSignal::missing_checked_fact(
                "empty brace literal must be checked as a record or Map[K, V]",
                span,
            ),
        }
    }

    pub(super) fn resume_expr_sequence(
        &mut self,
        expr: HirExprId,
        start_index: usize,
        mut values: Vec<InterpValue>,
        frame: &mut Frame,
    ) -> ControlSignal {
        let (kind, exprs) = match self.checked.hir.exprs.get(expr) {
            Some(HirExpr::Tuple { elems, .. }) => (AggregateKind::Tuple, elems),
            Some(HirExpr::Array { elems, .. }) => (AggregateKind::Array, elems),
            Some(HirExpr::List { elems, .. }) => (AggregateKind::List, elems),
            Some(HirExpr::Set { elems, .. }) => (AggregateKind::Set, elems),
            _ => {
                return ControlSignal::missing_checked_fact(
                    "aggregate continuation is missing its checked construction expression",
                    item_span(self.checked, self.entry_item),
                );
            }
        };
        values.reserve(exprs.len().saturating_sub(values.len()));
        for (index, element) in exprs.iter().enumerate().skip(start_index) {
            match self.eval_expr(*element, frame) {
                ControlSignal::Value(value) => values.push(value),
                signal if is_pending_host_boundary_signal(&signal) => {
                    return compose_signal_continuation(
                        signal,
                        Continuation::AggregateElement {
                            expr,
                            next_index: index + 1,
                            values,
                            frame: frame.clone(),
                        },
                    );
                }
                other => return other,
            }
        }
        ControlSignal::Value(match kind {
            AggregateKind::Tuple => InterpValue::Tuple(values.into()),
            AggregateKind::Array => InterpValue::Array(ArrayValue::new(values)),
            AggregateKind::List => InterpValue::List(values.into()),
            AggregateKind::Set => InterpValue::Set(values.into()),
        })
    }
}

#[cfg(test)]
#[path = "aggregate_expr/tests.rs"]
mod tests;
