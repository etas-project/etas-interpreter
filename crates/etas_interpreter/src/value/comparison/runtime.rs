mod context;
mod node;
mod ordered;
mod target;
#[cfg(test)]
mod tests;

use super::{Comparison, CursorStep, EqualityCursor, SetSearch, compare};
use crate::control::{CallTarget, CallTargetChildren, CallTargetLink, Frame};
use crate::value::{
    ArrayValue, DequeValue, InterpValue, ListValue, MapValue, MessageList, RecordValue, SetValue,
    SharedFields, SliceValue,
};
use context::Context;

pub(in crate::value) fn value_equal(left: &InterpValue, right: &InterpValue) -> bool {
    let context = Context::default();
    compare(node::compare_node(left, right, &context))
}

pub(crate) fn targets_equal(left: &CallTarget, right: &CallTarget) -> bool {
    compare(target::target(left, right, &Context::default()))
}

pub(crate) fn target_lists_equal(left: &CallTargetChildren, right: &CallTargetChildren) -> bool {
    compare(target::lists(left, right, &Context::default()))
}

pub(crate) fn frames_equal(left: &Frame, right: &Frame) -> bool {
    compare(target::frames(left, right, &Context::default()))
}

pub(crate) fn scoped_equal(
    left: &crate::value::ScopedCallableValue,
    right: &crate::value::ScopedCallableValue,
) -> bool {
    compare(target::scoped(left, right, &Context::default()))
}

pub(in crate::value) fn messages_equal(left: &MessageList, right: &MessageList) -> bool {
    if left.len() != right.len() {
        return false;
    }
    if std::ptr::eq(left.as_ptr(), right.as_ptr()) {
        return true;
    }
    compare(Context::default().pending(Children::Messages(left.clone(), right.clone())))
}

pub(in crate::value) fn sets_equal(left: &SetValue, right: &SetValue) -> bool {
    compare(set_comparison(left, right, &Context::default()))
}

fn set_comparison<'c>(
    left: &SetValue,
    right: &SetValue,
    context: &'c Context,
) -> Comparison<Cursor<'c>> {
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
    context.pending(children)
}

pub(super) struct Cursor<'c> {
    context: &'c Context,
    pair: Option<std::num::NonZeroUsize>,
    children: Children,
    index: usize,
}

enum Children {
    Frames(Box<crate::control::FrameComparison>),
    TargetLinks(CallTargetLink, CallTargetLink),
    Targets(CallTargetChildren, CallTargetChildren),
    Scoped(
        crate::value::ScopedCallableValue,
        crate::value::ScopedCallableValue,
    ),
    Messages(MessageList, MessageList),
    Fields(SharedFields, SharedFields),
    Array(ArrayValue, ArrayValue),
    Slice(SliceValue, SliceValue),
    Deque(DequeValue, DequeValue),
    List(ListValue, ListValue),
    Map(MapValue, MapValue),
    Ordered(Box<ordered::OrderedComparison>),
    PriorityQueue {
        a: crate::value::PriorityQueueValue,
        b: crate::value::PriorityQueueValue,
        a_slot: usize,
        b_slot: usize,
    },
    Record(RecordValue, RecordValue),
    SetSequence(SetValue, SetValue),
    SetSearch {
        left: SetValue,
        right: SetValue,
        search: SetSearch,
        hash: Option<(usize, u64)>,
    },
}

impl Cursor<'_> {
    fn complete(&self, equal: bool) -> CursorStep<Self> {
        self.context.finish(self.pair, equal);
        CursorStep::Complete(equal)
    }
}

impl EqualityCursor for Cursor<'_> {
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
            let next = search.next(previous, |left_position, candidate| {
                if hash.is_none_or(|(position, _)| position != left_position) {
                    *hash = Some((
                        left_position,
                        index.fingerprint(&left.borrow()[left_position]),
                    ));
                }
                index.candidate(hash.as_ref()?.1, candidate)
            });
            return match next {
                Ok(Some((a, b))) => CursorStep::Child(compare_node(
                    &left.borrow()[a],
                    &right.borrow()[b],
                    self.context,
                )),
                Ok(None) => self.complete(true),
                Err(()) => self.complete(false),
            };
        }
        if previous == Some(false) {
            return self.complete(false);
        }
        let position = self.index;
        self.index += 1;
        let context = self.context;
        let compare_node = |a: &InterpValue, b: &InterpValue| compare_node(a, b, context);
        let child = match &mut self.children {
            Children::Frames(frames) => {
                return match frames.next(compare_node) {
                    crate::control::FrameComparisonStep::Child(child) => CursorStep::Child(child),
                    crate::control::FrameComparisonStep::Complete(equal) => self.complete(equal),
                };
            }
            Children::TargetLinks(a, b) => (position == 0).then(|| target::target(a, b, context)),
            Children::Targets(a, b) => a
                .get(position)
                .zip(b.get(position))
                .map(|(a, b)| target::target(a, b, context)),
            Children::Scoped(a, b) => {
                (position == 0).then(|| target::target(a.target_ref(), b.target_ref(), context))
            }
            Children::Messages(a, b) => a.get(position).zip(b.get(position)).map(|(a, b)| {
                if node::message_headers_equal(a, b) {
                    compare_node(&a.payload, &b.payload)
                } else {
                    Comparison::Ready(false)
                }
            }),
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
            Children::Ordered(ordered) => ordered.next(position, context),
            Children::List(a, b) => {
                let next = a.get(0).zip(b.get(0)).map(|(a, b)| compare_node(a, b));
                a.advance();
                b.advance();
                next
            }
            Children::PriorityQueue {
                a,
                b,
                a_slot,
                b_slot,
            } => {
                if position == 0 {
                    return CursorStep::Child(target::optional(
                        a.comparator(),
                        b.comparator(),
                        context,
                    ));
                }
                let position = position - 1;
                while *a_slot < a.slot_count() && a.slot(*a_slot).is_none() {
                    *a_slot += 1;
                }
                while *b_slot < b.slot_count() && b.slot(*b_slot).is_none() {
                    *b_slot += 1;
                }
                let pair = a.slot(*a_slot).zip(b.slot(*b_slot));
                if position % 2 == 0 {
                    pair.map(|(a, b)| compare_node(&a.0, &b.0))
                } else {
                    *a_slot += 1;
                    *b_slot += 1;
                    pair.map(|(a, b)| compare_node(&a.1, &b.1))
                }
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
        match child {
            Some(child) => CursorStep::Child(child),
            None => self.complete(true),
        }
    }
}
