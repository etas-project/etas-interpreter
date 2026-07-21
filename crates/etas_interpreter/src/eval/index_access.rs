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
                    Continuation::IndexValue { expr, base, span },
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
        match base {
            InterpValue::Array(values) => {
                let Some(index) = self.index_usize(index, span) else {
                    return self.raise_index_error(
                        expr,
                        "array index is not a non-negative integer",
                        span,
                    );
                };
                let values = values.borrow();
                match values.get(index).cloned() {
                    Some(value) => ControlSignal::Value(value),
                    None => self.raise_index_error(
                        expr,
                        format!(
                            "array index {index} is out of bounds, length is {}",
                            values.len()
                        ),
                        span,
                    ),
                }
            }
            InterpValue::List(values) => {
                let Some(index) = self.index_usize(index, span) else {
                    return self.raise_index_error(
                        expr,
                        "list index is not a non-negative integer",
                        span,
                    );
                };
                let values = values.borrow();
                match values.get(index).cloned() {
                    Some(value) => ControlSignal::Value(value),
                    None => self.raise_index_error(
                        expr,
                        format!(
                            "list index {index} is out of bounds, length is {}",
                            values.len()
                        ),
                        span,
                    ),
                }
            }
            InterpValue::Slice(values) => {
                let Some(index) = self.index_usize(index, span) else {
                    return self.raise_index_error(
                        expr,
                        "slice index is not a non-negative integer",
                        span,
                    );
                };
                let values = values.borrow();
                match values.get(index).cloned() {
                    Some(value) => ControlSignal::Value(value),
                    None => self.raise_index_error(
                        expr,
                        format!(
                            "slice index {index} is out of bounds, length is {}",
                            values.len()
                        ),
                        span,
                    ),
                }
            }
            InterpValue::Map(entries) => entries
                .snapshot()
                .into_iter()
                .find_map(|(key, value)| (key == index).then_some(value))
                .map(ControlSignal::Value)
                .unwrap_or_else(|| {
                    ControlSignal::invalid_arguments(
                        "map key is missing at runtime; use Map.get when absence is expected",
                        span,
                    )
                }),
            InterpValue::Bytes(values) => {
                let Some(index) = self.index_usize(index, span) else {
                    return self.raise_index_error(
                        expr,
                        "bytes index is not a non-negative integer",
                        span,
                    );
                };
                values
                    .get(index)
                    .map(|value| InterpValue::u8(*value))
                    .map(ControlSignal::Value)
                    .unwrap_or_else(|| {
                        self.raise_index_error(
                            expr,
                            format!(
                                "bytes index {index} is out of bounds, length is {}",
                                values.len()
                            ),
                            span,
                        )
                    })
            }
            InterpValue::String(value) => {
                let Some(index) = self.index_usize(index, span) else {
                    return self.raise_index_error(
                        expr,
                        "string index is not a non-negative integer",
                        span,
                    );
                };
                let len = value.chars().count();
                value
                    .chars()
                    .nth(index)
                    .map(|ch| InterpValue::String(ch.to_string()))
                    .map(ControlSignal::Value)
                    .unwrap_or_else(|| {
                        self.raise_index_error(
                            expr,
                            format!("string index {index} is out of bounds, length is {len}"),
                            span,
                        )
                    })
            }
            other => ControlSignal::fault(
                AnalysisDiagnosticCode::InvalidArguments,
                span,
                format!(
                    "index evaluation expects an array, list, slice, map, bytes, or string value, got {:?}",
                    other
                ),
            ),
        }
    }

    fn raise_index_error(
        &mut self,
        expr: HirExprId,
        message: impl Into<String>,
        span: Span,
    ) -> ControlSignal {
        let Some(error_type) = self.checked.types.checked_index_errors.get(&expr).copied() else {
            return ControlSignal::missing_checked_fact(
                "checked index evaluation requires a materialized IndexError fact",
                span,
            );
        };
        ControlSignal::pending_perform(PendingPerform {
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
            args: vec![InterpValue::Variant {
                name: "IndexError".to_owned(),
                fields: vec![InterpValue::String(message.into())],
            }],
            span,
            continuation: Continuation::BlockValue,
        })
    }
}
