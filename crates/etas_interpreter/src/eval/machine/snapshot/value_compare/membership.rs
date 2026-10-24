use crate::{
    orchestration::ValueSnapshot as V,
    value::{
        comparison::{Comparison, CursorStep, EqualityCursor, SetSearch, compare},
        membership::MembershipIndex,
    },
};

pub(super) fn equal(a: &V, b: &V) -> bool {
    compare(node(a, b))
}

enum Cursor<'a> {
    Sequence {
        a: &'a [V],
        b: &'a [V],
        index: usize,
    },
    Map {
        a: &'a [(V, V)],
        b: &'a [(V, V)],
        index: usize,
    },
    Record {
        a: &'a [(String, V)],
        b: &'a [(String, V)],
        index: usize,
    },
    Pairs(std::vec::IntoIter<(&'a V, &'a V)>),
    Range(std::array::IntoIter<(&'a V, &'a V), 2>),
    Set {
        a: &'a [V],
        b: &'a [V],
        search: SetSearch,
        index: MembershipIndex,
        hash: Option<(usize, u64)>,
    },
}

fn node<'a>(mut a: &'a V, mut b: &'a V) -> Comparison<Cursor<'a>> {
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
            (V::Set(av), V::Set(bv)) | (V::OrderedSet(av), V::OrderedSet(bv)) => {
                if av.len() != bv.len() {
                    return Comparison::Ready(false);
                }
                if av.len() <= 1 {
                    Cursor::Sequence {
                        a: av,
                        b: bv,
                        index: 0,
                    }
                } else {
                    let mut index = MembershipIndex::with_capacity(bv.len());
                    for (position, value) in bv.iter().enumerate() {
                        index.insert(index.fingerprint(value), position);
                    }
                    Cursor::Set {
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
                Cursor::Sequence {
                    a: av,
                    b: bv,
                    index: 0,
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
                Cursor::Sequence {
                    a: av,
                    b: bv,
                    index: 0,
                }
            }
            (V::Map(av), V::Map(bv))
            | (V::OrderedMap(av), V::OrderedMap(bv))
            | (V::PriorityQueue(av), V::PriorityQueue(bv)) => {
                if av.len() != bv.len() {
                    return Comparison::Ready(false);
                }
                Cursor::Map {
                    a: av,
                    b: bv,
                    index: 0,
                }
            }
            (V::Record(av), V::Record(bv)) => {
                if av.len() != bv.len() {
                    return Comparison::Ready(false);
                }
                Cursor::Record {
                    a: av,
                    b: bv,
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
                Cursor::Range([(a.as_ref(), b.as_ref()), (c.as_ref(), d.as_ref())].into_iter())
            }
            _ => {
                let mut pairs = Vec::new();
                if !super::same_node(a, b, &mut pairs) {
                    return Comparison::Ready(false);
                }
                if pairs.is_empty() {
                    return Comparison::Ready(true);
                }
                Cursor::Pairs(pairs.into_iter())
            }
        };
        return Comparison::Pending(cursor);
    }
}

impl<'a> EqualityCursor for Cursor<'a> {
    fn advance(&mut self, previous: Option<bool>) -> CursorStep<Self> {
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
                Ok(Some((left, right))) => CursorStep::Child(node(&a[left], &b[right])),
                Ok(None) => CursorStep::Complete(true),
                Err(()) => CursorStep::Complete(false),
            };
        }
        if previous == Some(false) {
            return CursorStep::Complete(false);
        }
        let next = match self {
            Self::Sequence { a, b, index } => {
                let position = *index;
                *index += 1;
                a.get(position)
                    .zip(b.get(position))
                    .map(|(a, b)| node(a, b))
            }
            Self::Map { a, b, index } => {
                let position = *index;
                *index += 1;
                a.get(position / 2).zip(b.get(position / 2)).map(|(a, b)| {
                    if position % 2 == 0 {
                        node(&a.0, &b.0)
                    } else {
                        node(&a.1, &b.1)
                    }
                })
            }
            Self::Record { a, b, index } => {
                let position = *index;
                *index += 1;
                a.get(position)
                    .zip(b.get(position))
                    .map(|((an, av), (bn, bv))| {
                        if an == bn {
                            node(av, bv)
                        } else {
                            Comparison::Ready(false)
                        }
                    })
            }
            Self::Pairs(pairs) => pairs.next().map(|(a, b)| node(a, b)),
            Self::Range(pairs) => pairs.next().map(|(a, b)| node(a, b)),
            Self::Set { .. } => unreachable!("set searches handled above"),
        };
        next.map(CursorStep::Child)
            .unwrap_or(CursorStep::Complete(true))
    }
}
