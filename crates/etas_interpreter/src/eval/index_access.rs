use super::*;

impl<'a> EvalContext<'a> {
    pub(super) fn eval_index_expr(
        &mut self,
        expr: HirExprId,
        base: HirExprId,
        index: HirExprId,
        span: Span,
        frame: &mut Frame,
    ) -> ControlSignal {
        let base = match self.eval_expr(base, frame) {
            ControlSignal::Value(value) => value,
            signal if is_pending_host_boundary_signal(&signal) => {
                return compose_signal_continuation(
                    signal,
                    Continuation::IndexBase {
                        expr,
                        index,
                        span,
                        frame: frame.clone(),
                    },
                );
            }
            other => return other,
        };
        self.resume_index_base(expr, base, index, span, frame)
    }

    pub(super) fn resume_index_base(
        &mut self,
        expr: HirExprId,
        base: InterpValue,
        index: HirExprId,
        span: Span,
        frame: &mut Frame,
    ) -> ControlSignal {
        let index = match self.eval_expr(index, frame) {
            ControlSignal::Value(value) => value,
            signal if is_pending_host_boundary_signal(&signal) => {
                return compose_signal_continuation(
                    signal,
                    Continuation::IndexValue {
                        expr,
                        base: Box::new(base),
                        span,
                        frame: frame.clone(),
                    },
                );
            }
            other => return other,
        };
        self.eval_index_value(expr, base, index, span)
    }

    pub(super) fn eval_index_value(
        &mut self,
        expr: HirExprId,
        base: InterpValue,
        index: InterpValue,
        span: Span,
    ) -> ControlSignal {
        if !self.checked.types.index_facts.contains_key(&expr) {
            return ControlSignal::missing_checked_fact(
                "index expression is missing its checked index fact",
                span,
            );
        }
        match crate::value::source_slice::SourceSliceView::checked(
            &base,
            &self.checked.types.known_std_types,
            &self.checked.type_store,
        ) {
            Ok(Some(view)) => {
                let Some(index) = self.index_usize(index, span) else {
                    return self.raise_index_error(expr, None, view.length, "slice", span);
                };
                if index >= view.length {
                    return self.raise_index_error(expr, Some(index), view.length, "slice", span);
                }
                return ControlSignal::Value(view.source.borrow()[view.start + index].clone());
            }
            Ok(None) => {}
            Err(message) => return ControlSignal::missing_checked_fact(message, span),
        }
        match base {
            InterpValue::Array(values) => {
                let Some(index) = self.index_usize(index, span) else {
                    return self.raise_index_error(
                        expr,
                        None,
                        values.borrow().len(),
                        "array",
                        span,
                    );
                };
                let values = values.borrow();
                match values.get(index).cloned() {
                    Some(value) => ControlSignal::Value(value),
                    None => self.raise_index_error(expr, Some(index), values.len(), "array", span),
                }
            }
            InterpValue::List(values) => {
                let Some(index) = self.index_usize(index, span) else {
                    return self.raise_index_error(expr, None, values.len(), "list", span);
                };
                match values.get(index).cloned() {
                    Some(value) => ControlSignal::Value(value),
                    None => self.raise_index_error(expr, Some(index), values.len(), "list", span),
                }
            }
            InterpValue::Slice(values) => {
                let Some(index) = self.index_usize(index, span) else {
                    return self.raise_index_error(
                        expr,
                        None,
                        values.borrow().len(),
                        "slice",
                        span,
                    );
                };
                let values = values.borrow();
                match values.get(index).cloned() {
                    Some(value) => ControlSignal::Value(value),
                    None => self.raise_index_error(expr, Some(index), values.len(), "slice", span),
                }
            }
            InterpValue::Map(entries) => entries
                .get(&index)
                .map(ControlSignal::Value)
                .unwrap_or_else(|| {
                    ControlSignal::invalid_arguments(
                        "map key is missing at runtime; use Map.get when absence is expected",
                        span,
                    )
                }),
            InterpValue::Bytes(values) => {
                let Some(index) = self.index_usize(index, span) else {
                    return self.raise_index_error(expr, None, values.len(), "bytes", span);
                };
                values
                    .get(index)
                    .map(|value| InterpValue::u8(*value))
                    .map(ControlSignal::Value)
                    .unwrap_or_else(|| {
                        self.raise_index_error(expr, Some(index), values.len(), "bytes", span)
                    })
            }
            other => ControlSignal::fault(
                AnalysisDiagnosticCode::InvalidArguments,
                span,
                format!(
                    "index evaluation expects an array, list, slice, map, or bytes value, got {:?}",
                    other
                ),
            ),
        }
    }

    pub(super) fn raise_index_error(
        &mut self,
        expr: HirExprId,
        index: Option<usize>,
        length: usize,
        kind: &str,
        span: Span,
    ) -> ControlSignal {
        let Some(error_type) = self.checked.types.checked_index_errors.get(&expr).copied() else {
            return ControlSignal::missing_checked_fact(
                "checked index evaluation requires a materialized IndexError fact",
                span,
            );
        };
        if self.checked.hir.source_standard_types {
            let error = InterpValue::Nominal {
                ty: error_type,
                value: InterpValue::Record(crate::value::RecordValue::new(
                    [
                        (
                            "index".into(),
                            InterpValue::usize(index.unwrap_or(usize::MAX)),
                        ),
                        ("length".into(), InterpValue::usize(length)),
                    ]
                    .into_iter()
                    .collect(),
                ))
                .into(),
            };
            return self.raise_index_error_value(error_type, error, span);
        }
        let message = match index {
            Some(index) => format!("{kind} index {index} is out of bounds, length is {length}"),
            None => format!("{kind} index is not a non-negative integer"),
        };
        self.raise_typed_index_error(error_type, message, span)
    }

    pub(super) fn raise_typed_index_error(
        &mut self,
        error_type: etas_types::TypeId,
        message: impl Into<String>,
        span: Span,
    ) -> ControlSignal {
        let error = InterpValue::Variant {
            name: "IndexError".to_owned().into(),
            fields: vec![InterpValue::String(message.into().into())].into(),
        };
        self.raise_index_error_value(error_type, error, span)
    }

    fn raise_index_error_value(
        &self,
        error_type: etas_types::TypeId,
        error: InterpValue,
        span: Span,
    ) -> ControlSignal {
        ControlSignal::pending_perform(PendingPerform {
            checked_action: None,
            default_boundary: None,
            forwarded_call: None,
            inference: None,
            expr: None,
            error_type: Some(error_type),
            action: ResolvedActionRef {
                effect: HirEffectRef {
                    path: etas_hir::unresolved_path_from_segments(&["Error"], span),
                    args: Vec::new(),
                    span,
                },
                action: "raise".to_owned(),
                action_symbol: ResolveResult::Unresolved,
                span,
            },
            args: vec![error],
            span,
            continuation: Continuation::BlockValue,
            residual: None,
        })
    }
}
