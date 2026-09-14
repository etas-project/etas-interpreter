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
                match scalar_at(value.chars(), index) {
                    Ok(ch) => ControlSignal::Value(InterpValue::String(ch.to_string().into())),
                    Err(len) => self.raise_index_error(
                        expr,
                        format!("string index {index} is out of bounds, length is {len}"),
                        span,
                    ),
                }
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
                name: "IndexError".to_owned().into(),
                fields: vec![InterpValue::String(message.into().into())].into(),
            }],
            span,
            continuation: Continuation::BlockValue,
        })
    }
}

fn scalar_at(chars: impl Iterator<Item = char>, index: usize) -> Result<char, usize> {
    let mut len = 0;
    for ch in chars {
        if len == index {
            return Ok(ch);
        }
        len += 1;
    }
    Err(len)
}

#[cfg(test)]
mod tests {
    use super::scalar_at;
    use std::cell::Cell;

    #[test]
    fn scalar_index_stops_at_the_requested_unicode_scalar() {
        for count in [1000, 2000, 4000] {
            let text = "aé中🙂".repeat(count);
            for (index, expected) in [(0, 'a'), (1, 'é'), (2, '中'), (3, '🙂')] {
                let visited = Cell::new(0);
                let chars = text.chars().inspect(|_| visited.set(visited.get() + 1));
                assert_eq!(scalar_at(chars, index), Ok(expected));
                assert_eq!(visited.get(), index + 1);
            }
            assert_eq!(scalar_at(text.chars(), usize::MAX), Err(4 * count));
        }
        assert_eq!(scalar_at("".chars(), 0), Err(0));
        assert_eq!(scalar_at("e\u{301}".chars(), 1), Ok('\u{301}'));
        assert_eq!(scalar_at("e\u{301}".chars(), 2), Err(2));
    }
}
