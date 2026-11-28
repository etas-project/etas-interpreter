use super::*;
use crate::value::{ArrayValue, InterpValue as V, NumericValue};
use std::{collections::HashMap, rc::Weak};

#[derive(Default)]
pub(super) struct MapIndexes {
    program: std::sync::Weak<etas_frontend::CheckedProgramData>,
    entries: HashMap<(usize, u64), (Weak<Vec<V>>, ArrayValue)>,
    sweep_at: usize,
}

impl MapIndexes {
    // The weak backing identifies one immutable slots snapshot without keeping
    // its keys/values alive. The program identity scopes its checked Hash impl.
    fn for_program(&mut self, program: &etas_frontend::CheckedProject) {
        if self.program.as_ptr() != std::sync::Arc::as_ptr(program) {
            self.entries.clear();
            self.sweep_at = 0;
            self.program = std::sync::Arc::downgrade(program);
        }
    }

    fn remember(&mut self, slots: Weak<Vec<V>>, seed: u64, index: ArrayValue) {
        if self.entries.len() >= self.sweep_at {
            self.entries
                .retain(|_, (backing, _)| backing.strong_count() != 0);
            self.sweep_at = (self.entries.len() * 2).max(16);
        }
        self.entries
            .insert((slots.as_ptr() as usize, seed), (slots, index));
    }
}

impl EvalContext<'_> {
    pub(super) fn execute_array_storage(
        &mut self,
        role: etas_std::StdExecutionRole,
        args: Vec<V>,
        span: Span,
    ) -> ControlSignal {
        use etas_std::StdExecutionRole as R;
        let mut args = args.into_iter();
        let Some(V::Array(slots)) = args.next() else {
            return ControlSignal::missing_checked_fact(
                "array storage requires a checked array",
                span,
            );
        };
        let result = match (role, args.next(), args.next()) {
            (R::ArrayTruncate, Some(V::Number(length)), None) => {
                let Some(length) = length.as_usize() else {
                    return ControlSignal::missing_checked_fact(
                        "array prefix requires a usize length",
                        span,
                    );
                };
                V::Array(slots.truncate(length))
            }
            (R::CachedMapIndex, Some(V::Number(NumericValue::U64(seed))), None) => {
                let cache = &mut self.runtime.map_indexes;
                cache.for_program(self.checked);
                let key = (slots.weak_backing().as_ptr() as usize, seed);
                V::Array(
                    cache
                        .entries
                        .get(&key)
                        .map_or_else(|| Vec::new().into(), |(_, index)| index.clone()),
                )
            }
            (
                R::RememberMapIndex,
                Some(V::Number(NumericValue::U64(seed))),
                Some(V::Array(index)),
            ) => {
                let cache = &mut self.runtime.map_indexes;
                cache.for_program(self.checked);
                cache.remember(slots.weak_backing(), seed, index);
                V::Unit
            }
            _ => {
                return ControlSignal::missing_checked_fact(
                    "invalid checked array storage arguments",
                    span,
                );
            }
        };
        ControlSignal::Value(result)
    }
}
