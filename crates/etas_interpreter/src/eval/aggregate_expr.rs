use super::*;
use crate::value::{RangeBounds, RangeValue};

impl<'a> EvalContext<'a> {
    pub(super) fn eval_tuple_expr(
        &mut self,
        elems: &[HirExprId],
        frame: &mut Frame,
    ) -> ControlSignal {
        self.resume_expr_sequence(AggregateKind::Tuple, elems.to_vec(), 0, Vec::new(), frame)
    }

    pub(super) fn eval_array_expr(
        &mut self,
        elems: &[HirExprId],
        frame: &mut Frame,
    ) -> ControlSignal {
        self.resume_expr_sequence(AggregateKind::Array, elems.to_vec(), 0, Vec::new(), frame)
    }

    pub(super) fn eval_list_expr(
        &mut self,
        elems: &[HirExprId],
        frame: &mut Frame,
    ) -> ControlSignal {
        self.resume_expr_sequence(AggregateKind::List, elems.to_vec(), 0, Vec::new(), frame)
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
        let InterpValue::List(values) = tail else {
            return ControlSignal::missing_checked_fact(
                "list cons tail must evaluate to a List[T]",
                span,
            );
        };
        let mut result = values.snapshot();
        result.insert(0, head);
        ControlSignal::Value(InterpValue::List(result.into()))
    }

    pub(super) fn eval_set_expr(
        &mut self,
        elems: &[HirExprId],
        frame: &mut Frame,
    ) -> ControlSignal {
        self.resume_expr_sequence(AggregateKind::Set, elems.to_vec(), 0, Vec::new(), frame)
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
        ControlSignal::Value(InterpValue::Callable(CallTarget::Lambda {
            expr,
            captured: self.capture_frame(frame),
        }))
    }

    pub(super) fn eval_record_expr(
        &mut self,
        expr: HirExprId,
        record: &etas_hir::HirRecordExpr,
        frame: &mut Frame,
    ) -> ControlSignal {
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
        self.resume_record_fields(
            nominal_type,
            variant_symbol,
            record.fields.clone(),
            0,
            Vec::new(),
            frame,
        )
    }

    pub(super) fn resume_record_fields(
        &mut self,
        nominal_type: Option<etas_types::TypeId>,
        variant_symbol: Option<SymbolId>,
        fields: Vec<etas_hir::HirFieldInit>,
        start_index: usize,
        mut values: Vec<(String, InterpValue)>,
        frame: &mut Frame,
    ) -> ControlSignal {
        for (index, field) in fields.iter().enumerate().skip(start_index) {
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
                                    nominal_type,
                                    variant_symbol,
                                    fields,
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
            return match self.eval_named_variant(
                symbol,
                values,
                item_span(self.checked, self.entry_item),
            ) {
                Ok(value) => ControlSignal::Value(value),
                Err(fault) => ControlSignal::Fault(Box::new(fault)),
            };
        }
        let value = InterpValue::Record(values.into());
        ControlSignal::Value(match nominal_type {
            Some(ty) => InterpValue::Nominal {
                ty,
                value: Box::new(value),
            },
            None => value,
        })
    }

    pub(super) fn eval_map_expr(
        &mut self,
        entries: &[etas_hir::HirMapEntry],
        frame: &mut Frame,
    ) -> ControlSignal {
        self.resume_map_entries(entries.to_vec(), 0, Vec::new(), frame)
    }

    pub(super) fn resume_map_entries(
        &mut self,
        entries: Vec<etas_hir::HirMapEntry>,
        start_index: usize,
        mut values: Vec<(InterpValue, InterpValue)>,
        frame: &mut Frame,
    ) -> ControlSignal {
        for (index, entry) in entries.iter().enumerate().skip(start_index) {
            let key = match self.eval_expr(entry.key, frame) {
                ControlSignal::Value(value) => value,
                signal if is_pending_host_boundary_signal(&signal) => {
                    return compose_signal_continuation(
                        signal,
                        Continuation::MapKey {
                            entries,
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
                            entries,
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
        entries: Vec<etas_hir::HirMapEntry>,
        index: usize,
        values: Vec<(InterpValue, InterpValue)>,
        key: InterpValue,
        frame: &mut Frame,
    ) -> ControlSignal {
        let Some(entry) = entries.get(index) else {
            return ControlSignal::invalid_arguments(
                "map key continuation index is out of range",
                item_span(self.checked, self.entry_item),
            );
        };
        match self.eval_expr(entry.value, frame) {
            ControlSignal::Value(value) => {
                self.resume_map_value(entries, index, values, key, value, frame)
            }
            signal if is_pending_host_boundary_signal(&signal) => compose_signal_continuation(
                signal,
                Continuation::MapValue {
                    entries,
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
        entries: Vec<etas_hir::HirMapEntry>,
        index: usize,
        mut values: Vec<(InterpValue, InterpValue)>,
        key: InterpValue,
        value: InterpValue,
        frame: &mut Frame,
    ) -> ControlSignal {
        values.push((key, value));
        self.resume_map_entries(entries, index + 1, values, frame)
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
        kind: AggregateKind,
        exprs: Vec<HirExprId>,
        start_index: usize,
        mut values: Vec<InterpValue>,
        frame: &mut Frame,
    ) -> ControlSignal {
        for (index, expr) in exprs.iter().enumerate().skip(start_index) {
            match self.eval_expr(*expr, frame) {
                ControlSignal::Value(value) => values.push(value),
                signal if is_pending_host_boundary_signal(&signal) => {
                    return compose_signal_continuation(
                        signal,
                        Continuation::AggregateElement {
                            kind,
                            exprs,
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
            AggregateKind::Tuple => InterpValue::Tuple(values),
            AggregateKind::Array => InterpValue::Array(ArrayValue::new(values)),
            AggregateKind::List => InterpValue::List(values.into()),
            AggregateKind::Set => InterpValue::Set(values.into()),
        })
    }

    fn capture_frame(&self, frame: &Frame) -> Frame {
        let mut captured = Frame::new(self.plan.slots.clone());
        for (symbol, value) in frame.sorted_locals() {
            captured.insert(symbol, value);
        }
        captured
    }
}
