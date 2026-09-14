mod node;

use super::{Comparison, CursorStep, EqualityCursor, SetSearch, compare};
use crate::value::{
    ArrayValue, DequeValue, InterpValue, ListValue, MapValue, RecordValue, SetValue, SharedFields,
    SliceValue,
};

pub(in crate::value) fn value_equal(left: &InterpValue, right: &InterpValue) -> bool {
    compare(node::compare_node(left, right))
}

pub(in crate::value) fn sets_equal(left: &SetValue, right: &SetValue) -> bool {
    compare(set_comparison(left, right))
}

fn set_comparison(left: &SetValue, right: &SetValue) -> Comparison<Cursor> {
    let len = left.borrow().len();
    if len != right.borrow().len() {
        return Comparison::Ready(false);
    }
    if len == 0 {
        return Comparison::Ready(true);
    }
    let children = if len == 1 {
        Children::SetSequence(left.clone(), right.clone())
    } else {
        Children::SetSearch {
            left: left.clone(),
            right: right.clone(),
            search: SetSearch::unique(len),
            hash: None,
        }
    };
    Comparison::Pending(Cursor { children, index: 0 })
}

pub(super) struct Cursor {
    children: Children,
    index: usize,
}

enum Children {
    Fields(SharedFields, SharedFields),
    Array(ArrayValue, ArrayValue),
    Slice(SliceValue, SliceValue),
    Deque(DequeValue, DequeValue),
    List(ListValue, ListValue),
    Map(MapValue, MapValue),
    Record(RecordValue, RecordValue),
    SetSequence(SetValue, SetValue),
    SetSearch {
        left: SetValue,
        right: SetValue,
        search: SetSearch,
        hash: Option<(usize, u64)>,
    },
}

impl EqualityCursor for Cursor {
    fn advance(&mut self, previous: Option<bool>) -> CursorStep<Self> {
        use node::compare_node;
        if let Children::SetSearch {
            left,
            right,
            search,
            hash,
        } = &mut self.children
        {
            let index = right.index();
            return match search.next(previous, |left_position, candidate| {
                if hash.is_none_or(|(position, _)| position != left_position) {
                    *hash = Some((
                        left_position,
                        index.fingerprint(&left.borrow()[left_position]),
                    ));
                }
                index.candidate(hash.as_ref()?.1, candidate)
            }) {
                Ok(Some((a, b))) => {
                    CursorStep::Child(compare_node(&left.borrow()[a], &right.borrow()[b]))
                }
                Ok(None) => CursorStep::Complete(true),
                Err(()) => CursorStep::Complete(false),
            };
        }
        if previous == Some(false) {
            return CursorStep::Complete(false);
        }
        let position = self.index;
        self.index += 1;
        let child = match &mut self.children {
            Children::Fields(a, b) => a
                .get(position)
                .zip(b.get(position))
                .map(|(a, b)| compare_node(a, b)),
            Children::Array(a, b) => a
                .borrow()
                .get(position)
                .zip(b.borrow().get(position))
                .map(|(a, b)| compare_node(a, b)),
            Children::Slice(a, b) => a
                .borrow()
                .get(position)
                .zip(b.borrow().get(position))
                .map(|(a, b)| compare_node(a, b)),
            Children::Deque(a, b) => a
                .borrow()
                .get(position)
                .zip(b.borrow().get(position))
                .map(|(a, b)| compare_node(a, b)),
            Children::SetSequence(a, b) => a
                .borrow()
                .get(position)
                .zip(b.borrow().get(position))
                .map(|(a, b)| compare_node(a, b)),
            Children::List(a, b) => {
                let next = a.get(0).zip(b.get(0)).map(|(a, b)| compare_node(a, b));
                a.advance();
                b.advance();
                next
            }
            Children::Map(a, b) => {
                let a = a.borrow();
                let b = b.borrow();
                a.get(position / 2).zip(b.get(position / 2)).map(|(a, b)| {
                    if position % 2 == 0 {
                        compare_node(&a.0, &b.0)
                    } else {
                        compare_node(&a.1, &b.1)
                    }
                })
            }
            Children::Record(a, b) => {
                let a = a.borrow();
                let b = b.borrow();
                a.get(position)
                    .zip(b.get(position))
                    .map(|((an, av), (bn, bv))| {
                        if an != bn {
                            Comparison::Ready(false)
                        } else {
                            compare_node(av, bv)
                        }
                    })
            }
            Children::SetSearch { .. } => unreachable!("set searches handled above"),
        };
        child
            .map(CursorStep::Child)
            .unwrap_or(CursorStep::Complete(true))
    }
}
