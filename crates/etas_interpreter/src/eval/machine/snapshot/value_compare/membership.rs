use crate::{
    orchestration::ValueSnapshot as V,
    value::{
        comparison::{Comparison, CursorStep, EqualityCursor, SetSearch, compare},
        membership::MembershipIndex,
        record::RecordLayout,
    },
};
use std::{cell::RefCell, collections::HashSet};

pub(super) fn equal(a: &V, b: &V) -> bool {
    let context = Context::default();
    compare(node(a, b, &context))
}

#[derive(Default)]
struct Context {
    // Only successful comparisons survive a failed Set collision candidate.
    completed: RefCell<HashSet<(usize, usize)>>,
    #[cfg(test)]
    visits: std::cell::Cell<usize>,
}

struct Cursor<'a> {
    kind: Kind<'a>,
    context: &'a Context,
    pair: (usize, usize),
}

enum Kind<'a> {
    Sequence {
        a: &'a [V],
        b: &'a [V],
        index: usize,
        comparator: Option<(&'a V, &'a V)>,
    },
    Map {
        a: &'a [(V, V)],
        b: &'a [(V, V)],
        index: usize,
        comparator: Option<(&'a V, &'a V)>,
    },
    Record {
        a: &'a [(String, V)],
        b: &'a [(String, V)],
        a_layout: RecordLayout,
        b_layout: RecordLayout,
        index: usize,
    },
    Pairs(super::ReferencePairs<'a>),
    Range(std::array::IntoIter<(&'a V, &'a V), 2>),
    Set {
        a: &'a [V],
        b: &'a [V],
        search: SetSearch,
        index: MembershipIndex,
        hash: Option<(usize, u64)>,
    },
}

fn node<'a>(mut a: &'a V, mut b: &'a V, context: &'a Context) -> Comparison<Cursor<'a>> {
    #[cfg(test)]
    context.visits.set(context.visits.get() + 1);
    let pair = (a as *const V as usize, b as *const V as usize);
    if context.completed.borrow().contains(&pair) {
        return Comparison::Ready(true);
    }
    loop {
        let cursor = match (a, b) {
            (V::Nominal { ty: at, value: av }, V::Nominal { ty: bt, value: bv }) => {
                if at != bt {
                    return Comparison::Ready(false);
                }
                a = av;
                b = bv;
                continue;
            }
            (
                V::Trust {
                    wrapper: at,
                    value: av,
                },
                V::Trust {
                    wrapper: bt,
                    value: bv,
                },
            ) => {
                if at != bt {
                    return Comparison::Ready(false);
                }
                a = av;
                b = bv;
                continue;
            }
            (V::OptionSome(av), V::OptionSome(bv)) => {
                a = av;
                b = bv;
                continue;
            }
            (V::Set(av), V::Set(bv)) => {
                if av.len() != bv.len() {
                    return Comparison::Ready(false);
                }
                if av.len() <= 1 {
                    Kind::Sequence {
                        a: av,
                        b: bv,
                        index: 0,
                        comparator: None,
                    }
                } else {
                    let mut index = MembershipIndex::with_capacity(bv.len());
                    for (position, value) in bv.iter().enumerate() {
                        index.insert(index.fingerprint(value), position);
                    }
                    Kind::Set {
                        a: av,
                        b: bv,
                        search: SetSearch::new(av.len()),
                        index,
                        hash: None,
                    }
                }
            }
            (V::Tuple(av), V::Tuple(bv))
            | (V::Array(av), V::Array(bv))
            | (V::List(av), V::List(bv))
            | (V::Slice(av), V::Slice(bv))
            | (V::Deque(av), V::Deque(bv))
            | (V::Queue(av), V::Queue(bv))
            | (V::Stack(av), V::Stack(bv)) => {
                if av.len() != bv.len() {
                    return Comparison::Ready(false);
                }
                Kind::Sequence {
                    a: av,
                    b: bv,
                    index: 0,
                    comparator: None,
                }
            }
            (
                V::Variant {
                    name: an,
                    fields: av,
                },
                V::Variant {
                    name: bn,
                    fields: bv,
                },
            ) => {
                if an != bn || av.len() != bv.len() {
                    return Comparison::Ready(false);
                }
                Kind::Sequence {
                    a: av,
                    b: bv,
                    index: 0,
                    comparator: None,
                }
            }
            (
                V::PriorityQueue {
                    entries: av,
                    comparator: ac,
                },
                V::PriorityQueue {
                    entries: bv,
                    comparator: bc,
                },
            ) => {
                if av.len() != bv.len() || ac.is_some() != bc.is_some() {
                    return Comparison::Ready(false);
                }
                Kind::Map {
                    a: av,
                    b: bv,
                    index: 0,
                    comparator: ac.as_deref().zip(bc.as_deref()),
                }
            }
            (
                V::OrderedSet {
                    values: av,
                    comparator: ac,
                },
                V::OrderedSet {
                    values: bv,
                    comparator: bc,
                },
            ) => {
                if av.len() != bv.len() || ac.is_some() != bc.is_some() {
                    return Comparison::Ready(false);
                }
                Kind::Sequence {
                    a: av,
                    b: bv,
                    index: 0,
                    comparator: ac.as_deref().zip(bc.as_deref()),
                }
            }
            (
                V::OrderedMap {
                    entries: av,
                    comparator: ac,
                },
                V::OrderedMap {
                    entries: bv,
                    comparator: bc,
                },
            ) => {
                if av.len() != bv.len() || ac.is_some() != bc.is_some() {
                    return Comparison::Ready(false);
                }
                Kind::Map {
                    a: av,
                    b: bv,
                    index: 0,
                    comparator: ac.as_deref().zip(bc.as_deref()),
                }
            }
            (V::Map(av), V::Map(bv)) => {
                if av.len() != bv.len() {
                    return Comparison::Ready(false);
                }
                Kind::Map {
                    a: av,
                    b: bv,
                    index: 0,
                    comparator: None,
                }
            }
            (V::Record(av), V::Record(bv)) => {
                if av.len() != bv.len() {
                    return Comparison::Ready(false);
                }
                Kind::Record {
                    a: av,
                    b: bv,
                    a_layout: RecordLayout::from_fields(av),
                    b_layout: RecordLayout::from_fields(bv),
                    index: 0,
                }
            }
            (
                V::Range {
                    start: a,
                    end: c,
                    bounds: x,
                },
                V::Range {
                    start: b,
                    end: d,
                    bounds: y,
                },
            ) => {
                if x != y {
                    return Comparison::Ready(false);
                }
                if let (V::Number(a), V::Number(b), V::Number(c), V::Number(d)) =
                    (a.as_ref(), b.as_ref(), c.as_ref(), d.as_ref())
                {
                    return Comparison::Ready(a == b && c == d);
                }
                Kind::Range([(a.as_ref(), b.as_ref()), (c.as_ref(), d.as_ref())].into_iter())
            }
            _ => {
                let mut pairs = super::Pending(Vec::new());
                if !super::same_node(a, b, &mut pairs) {
                    return Comparison::Ready(false);
                }
                if pairs.0.is_empty() {
                    return Comparison::Ready(true);
                }
                Kind::Pairs(super::ReferencePairs::from_pending(pairs))
            }
        };
        return Comparison::Pending(Cursor {
            kind: cursor,
            context,
            pair,
        });
    }
}

impl<'a> EqualityCursor for Cursor<'a> {
    fn advance(&mut self, previous: Option<bool>) -> CursorStep<Self> {
        let step = self.kind.advance(self.context, previous);
        if matches!(step, CursorStep::Complete(true)) {
            self.context.completed.borrow_mut().insert(self.pair);
        }
        step
    }
}

impl<'a> Kind<'a> {
    fn advance(&mut self, context: &'a Context, previous: Option<bool>) -> CursorStep<Cursor<'a>> {
        if let Self::Set {
            a,
            b,
            search,
            index,
            hash,
        } = self
        {
            return match search.next(previous, |left, candidate| {
                if hash.is_none_or(|(position, _)| position != left) {
                    *hash = Some((left, index.fingerprint(&a[left])));
                }
                index.candidate(hash.as_ref()?.1, candidate)
            }) {
                Ok(Some((left, right))) => CursorStep::Child(node(&a[left], &b[right], context)),
                Ok(None) => CursorStep::Complete(true),
                Err(()) => CursorStep::Complete(false),
            };
        }
        if previous == Some(false) {
            return CursorStep::Complete(false);
        }
        let next = match self {
            Self::Sequence {
                a,
                b,
                index,
                comparator,
            } => {
                if let Some((a, b)) = comparator.take() {
                    return CursorStep::Child(node(a, b, context));
                }
                let position = *index;
                *index += 1;
                a.get(position)
                    .zip(b.get(position))
                    .map(|(a, b)| node(a, b, context))
            }
            Self::Map {
                a,
                b,
                index,
                comparator,
            } => {
                if let Some((a, b)) = comparator.take() {
                    return CursorStep::Child(node(a, b, context));
                }
                let position = *index;
                *index += 1;
                a.get(position / 2).zip(b.get(position / 2)).map(|(a, b)| {
                    if position % 2 == 0 {
                        node(&a.0, &b.0, context)
                    } else {
                        node(&a.1, &b.1, context)
                    }
                })
            }
            Self::Record {
                a,
                b,
                a_layout,
                b_layout,
                index,
            } => {
                let position = *index;
                *index += 1;
                a_layout
                    .field_at(a, position)
                    .zip(b_layout.field_at(b, position))
                    .map(|((an, av), (bn, bv))| {
                        if an == bn {
                            node(av, bv, context)
                        } else {
                            Comparison::Ready(false)
                        }
                    })
            }
            Self::Pairs(pairs) => match pairs.next_values() {
                Ok(Some((a, b))) => Some(node(a, b, context)),
                Ok(None) => None,
                Err(()) => Some(Comparison::Ready(false)),
            },
            Self::Range(pairs) => pairs.next().map(|(a, b)| node(a, b, context)),
            Self::Set { .. } => unreachable!("set searches handled above"),
        };
        next.map(CursorStep::Child)
            .unwrap_or(CursorStep::Complete(true))
    }
}

#[cfg(test)]
mod tests;
