use super::{Comparison, Context, Cursor, node, target};
use crate::value::{OrderedMapValue, OrderedSetValue, ordered};

enum Source {
    Map(OrderedMapValue, OrderedMapValue),
    Set(OrderedSetValue, OrderedSetValue),
}

pub(super) struct OrderedComparison {
    source: Source,
    left: ordered::Cursor,
    right: ordered::Cursor,
}

impl OrderedComparison {
    pub(super) fn map(left: &OrderedMapValue, right: &OrderedMapValue) -> Self {
        Self {
            source: Source::Map(left.clone(), right.clone()),
            left: left.cursor(),
            right: right.cursor(),
        }
    }

    pub(super) fn set(left: &OrderedSetValue, right: &OrderedSetValue) -> Self {
        Self {
            source: Source::Set(left.clone(), right.clone()),
            left: left.cursor(),
            right: right.cursor(),
        }
    }

    pub(super) fn next<'c>(
        &mut self,
        position: usize,
        context: &'c Context,
    ) -> Option<Comparison<Cursor<'c>>> {
        if position == 0 {
            let (a, b) = match &self.source {
                Source::Map(a, b) => (a.comparator(), b.comparator()),
                Source::Set(a, b) => (a.comparator(), b.comparator()),
            };
            return Some(target::optional(a, b, context));
        }
        let map = matches!(self.source, Source::Map(..));
        let value = map && (position - 1) % 2 == 1;
        let (a, b) = self.left.current().zip(self.right.current())?;
        let child = if value {
            node::compare_node(&a.1, &b.1, context)
        } else {
            node::compare_node(&a.0, &b.0, context)
        };
        if !map || value {
            self.left.advance();
            self.right.advance();
        }
        Some(child)
    }
}
