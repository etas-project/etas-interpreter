use super::*;
use std::collections::HashSet;

pub(crate) struct SourceForLoopResume {
    pub iter: HirExprId,
    pub pat: etas_hir::HirPatId,
    pub source: Option<InterpValue>,
    pub awaiting_step: bool,
    pub next_index: usize,
    pub body: HirBlockId,
    pub iterations: usize,
    pub loop_scope: HashSet<SymbolId>,
    pub span: Span,
}

impl SourceForLoopResume {
    pub(crate) fn continuation(self, frame: Frame) -> Continuation {
        Continuation::SourceForLoop {
            iter: self.iter,
            pat: self.pat,
            source: self.source.map(Box::new),
            awaiting_step: self.awaiting_step,
            next_index: self.next_index,
            body: self.body,
            iterations: self.iterations,
            loop_scope: self.loop_scope,
            span: self.span,
            frame,
        }
    }
}

impl EvalContext<'_> {
    pub(crate) fn resume_source_for_loop(
        &mut self,
        mut state: SourceForLoopResume,
        value: InterpValue,
        frame: &mut Frame,
    ) -> ControlSignal {
        if state.awaiting_step {
            let Some(protocol) = self.plan.records.source_iterator(state.iter) else {
                return ControlSignal::missing_checked_fact(
                    "source loop has no checked iterator protocol",
                    state.span,
                );
            };
            let pair = if protocol.native_option {
                let step = match self.materialized_type(
                    protocol.step_type,
                    &frame.bindings_at(self.checked, etas_hir::HirOwner::Expr(state.iter)),
                    state.span,
                ) {
                    Ok(step) => step,
                    Err(fault) => return ControlSignal::Fault(Box::new(fault)),
                };
                match crate::value::enum_value::payload(&value, step, self.checked) {
                    Ok(("None", [])) => {
                        frame.cleanup_to(&state.loop_scope);
                        return ControlSignal::Value(InterpValue::Unit);
                    }
                    Ok(("Some", [pair])) => pair.clone(),
                    Ok(_) => {
                        return ControlSignal::missing_checked_fact(
                            "source iterator step differs from its checked Option declaration",
                            state.span,
                        );
                    }
                    Err(message) => {
                        return ControlSignal::missing_checked_fact(message, state.span);
                    }
                }
            } else {
                match value {
                    InterpValue::OptionNone => {
                        frame.cleanup_to(&state.loop_scope);
                        return ControlSignal::Value(InterpValue::Unit);
                    }
                    InterpValue::OptionSome(value) => value.into_value(),
                    _ => {
                        return ControlSignal::missing_checked_fact(
                            "source iterator did not return Option",
                            state.span,
                        );
                    }
                }
            };
            let InterpValue::Tuple(pair) = pair else {
                return ControlSignal::missing_checked_fact(
                    "source iterator did not return an element/cursor pair",
                    state.span,
                );
            };
            let Ok([element, remaining]) =
                <Vec<InterpValue> as TryInto<[InterpValue; 2]>>::try_into(pair.into_values())
            else {
                return ControlSignal::missing_checked_fact(
                    "source iterator step returned the wrong tuple arity",
                    state.span,
                );
            };
            state.source = Some(remaining);
            state.awaiting_step = false;
            state.next_index += 1;
            if let Err(fault) = self.bind_pattern(state.pat, element, frame, state.span) {
                frame.cleanup_to(&state.loop_scope);
                return ControlSignal::Fault(Box::new(fault));
            }
            match self.execute_block(state.body, frame) {
                ControlSignal::Value(_) | ControlSignal::Continue => {}
                ControlSignal::Break => {
                    frame.cleanup_to(&state.loop_scope);
                    return ControlSignal::Value(InterpValue::Unit);
                }
                signal => {
                    return compose_signal_continuation(signal, state.continuation(frame.clone()));
                }
            }
        } else if state.source.is_none() {
            state.source = Some(value);
        }
        if state.next_index >= state.iterations {
            frame.cleanup_to(&state.loop_scope);
            return ControlSignal::Value(InterpValue::Unit);
        }
        let Some(protocol) = self.plan.records.source_iterator(state.iter) else {
            return ControlSignal::missing_checked_fact(
                "source loop has no checked iterator protocol",
                state.span,
            );
        };
        let target = match protocol.next(
            self.checked,
            state.source.as_ref().expect("source cursor initialized"),
        ) {
            Ok(target) => target,
            Err(message) => return ControlSignal::missing_checked_fact(message, state.span),
        };
        state.awaiting_step = true;
        let signal = self.execute_call_target(target, Vec::new(), state.span);
        compose_signal_continuation(signal, state.continuation(frame.clone()))
    }
}
