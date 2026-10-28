use crate::orchestration::{MessageSnapshot, ValueSnapshot};
use std::collections::HashSet;
mod callables;
mod membership;

enum Comparison<'a> {
    Value(&'a ValueSnapshot, &'a ValueSnapshot),
    Target(
        &'a crate::orchestration::CallTargetSnapshot,
        &'a crate::orchestration::CallTargetSnapshot,
    ),
    Frame(
        &'a crate::orchestration::LocalsSnapshot,
        &'a crate::orchestration::LocalsSnapshot,
    ),
    Targets(TargetPairs<'a>),
}

type TargetPairs<'a> = std::iter::Zip<
    std::slice::Iter<'a, crate::orchestration::CallTargetSnapshot>,
    std::slice::Iter<'a, crate::orchestration::CallTargetSnapshot>,
>;

#[derive(Default)]
struct Seen(HashSet<(u8, usize, usize)>);
impl Seen {
    fn edge<T>(&mut self, kind: u8, a: Option<*const T>, b: Option<*const T>) -> bool {
        match (a, b) {
            (Some(a), Some(b)) => !self.0.insert((kind, a as usize, b as usize)),
            _ => false,
        }
    }
}

struct Pending<'a>(Vec<Comparison<'a>>);
impl<'a> Pending<'a> {
    fn push(&mut self, (a, b): (&'a ValueSnapshot, &'a ValueSnapshot)) {
        self.0.push(Comparison::Value(a, b));
    }
    fn extend(&mut self, pairs: impl IntoIterator<Item = (&'a ValueSnapshot, &'a ValueSnapshot)>) {
        self.0
            .extend(pairs.into_iter().map(|(a, b)| Comparison::Value(a, b)));
    }
}

fn equal(root: Comparison<'_>) -> bool {
    let mut pairs = ReferencePairs {
        pending: Pending(Vec::new()),
        next: Some(root),
        seen: Seen::default(),
        compare_frame_ids: true,
    };
    loop {
        match pairs.next_values() {
            Ok(Some((a, b))) if same_node(a, b, &mut pairs.pending) => {}
            Ok(None) => return true,
            _ => return false,
        }
    }
}

struct ReferencePairs<'a> {
    pending: Pending<'a>,
    next: Option<Comparison<'a>>,
    seen: Seen,
    compare_frame_ids: bool,
}

impl<'a> ReferencePairs<'a> {
    fn from_pending(pending: Pending<'a>) -> Self {
        Self {
            pending,
            next: None,
            seen: Seen::default(),
            // Frame IDs name artifact definitions, not captured language values.
            compare_frame_ids: false,
        }
    }

    fn next_values(&mut self) -> Result<Option<(&'a ValueSnapshot, &'a ValueSnapshot)>, ()> {
        while let Some(pair) = self.next.take().or_else(|| self.pending.0.pop()) {
            let same = match pair {
                Comparison::Value(a, b) => return Ok(Some((a, b))),
                Comparison::Target(a, b) => {
                    callables::target(a, b, &mut self.pending, &mut self.next, &mut self.seen)
                }
                Comparison::Frame(a, b) => callables::frame(
                    a,
                    b,
                    &mut self.pending,
                    &mut self.seen,
                    self.compare_frame_ids,
                ),
                Comparison::Targets(mut pairs) => {
                    if let Some((a, b)) = pairs.next() {
                        if pairs.len() > 0 {
                            self.pending.0.push(Comparison::Targets(pairs));
                        }
                        self.next = Some(Comparison::Target(a, b));
                    }
                    true
                }
            };
            if !same {
                return Err(());
            }
        }
        Ok(None)
    }
}

pub(crate) fn equal_targets(
    a: &[crate::orchestration::CallTargetSnapshot],
    b: &[crate::orchestration::CallTargetSnapshot],
) -> bool {
    a.len() == b.len() && equal(Comparison::Targets(a.iter().zip(b)))
}

// Runtime Set equality is order-independent; snapshot equality below remains
// order-sensitive so conflicting definitions of a frame cannot change iteration.
pub(super) fn membership_equal(a: &ValueSnapshot, b: &ValueSnapshot) -> bool {
    membership::equal(a, b)
}

impl PartialEq for ValueSnapshot {
    fn eq(&self, other: &Self) -> bool {
        equal(Comparison::Value(self, other))
    }
}

fn same_node<'a>(a: &'a ValueSnapshot, b: &'a ValueSnapshot, pending: &mut Pending<'a>) -> bool {
    use ValueSnapshot as V;
    match (a, b) {
        (V::Unit, V::Unit) | (V::OptionNone, V::OptionNone) => true,
        (V::Bool(a), V::Bool(b)) => a == b,
        (V::Number(a), V::Number(b)) => a == b,
        (V::String(a), V::String(b)) => a == b,
        (V::Bytes(a), V::Bytes(b)) => a == b,
        (V::Json(a), V::Json(b)) => a == b,
        (V::Prompt(a), V::Prompt(b)) => a == b,
        (V::Provenance(a), V::Provenance(b)) => a == b,
        (V::ModelResponse(a), V::ModelResponse(b)) => a == b,
        (V::MemoryWriteIntent(a), V::MemoryWriteIntent(b)) => a == b,
        (V::CodeIdentity { ty: a, identity: x }, V::CodeIdentity { ty: b, identity: y }) => {
            a == b && x == y
        }
        (V::Nominal { ty: a, value: x }, V::Nominal { ty: b, value: y }) => {
            pending.push((x, y));
            a == b
        }
        (
            V::Trust {
                wrapper: a,
                value: x,
            },
            V::Trust {
                wrapper: b,
                value: y,
            },
        ) => {
            pending.push((x, y));
            a == b
        }
        (V::OptionSome(a), V::OptionSome(b)) => {
            pending.push((a, b));
            true
        }
        (V::Tuple(a), V::Tuple(b))
        | (V::Array(a), V::Array(b))
        | (V::List(a), V::List(b))
        | (V::Slice(a), V::Slice(b))
        | (V::Set(a), V::Set(b))
        | (V::Deque(a), V::Deque(b))
        | (V::Queue(a), V::Queue(b))
        | (V::Stack(a), V::Stack(b)) => {
            pending.extend(a.iter().zip(b));
            a.len() == b.len()
        }
        (V::Variant { name: a, fields: x }, V::Variant { name: b, fields: y }) => {
            pending.extend(x.iter().zip(y));
            a == b && x.len() == y.len()
        }
        (
            V::OrderedSet {
                values: a,
                comparator: ac,
            },
            V::OrderedSet {
                values: b,
                comparator: bc,
            },
        ) => {
            pending.extend(a.iter().zip(b));
            if let Some((a, b)) = ac.as_deref().zip(bc.as_deref()) {
                pending.push((a, b));
            }
            a.len() == b.len() && ac.is_some() == bc.is_some()
        }
        (
            V::PriorityQueue {
                entries: a,
                comparator: ac,
            },
            V::PriorityQueue {
                entries: b,
                comparator: bc,
            },
        ) => {
            if ac.is_some() != bc.is_some() {
                return false;
            }
            if let (Some(a), Some(b)) = (ac, bc) {
                pending.push((a, b));
            }
            for ((ak, av), (bk, bv)) in a.iter().zip(b) {
                pending.push((ak, bk));
                pending.push((av, bv));
            }
            a.len() == b.len()
        }
        (
            V::OrderedMap {
                entries: a,
                comparator: ac,
            },
            V::OrderedMap {
                entries: b,
                comparator: bc,
            },
        ) => {
            if let Some((a, b)) = ac.as_deref().zip(bc.as_deref()) {
                pending.push((a, b));
            }
            for ((ak, av), (bk, bv)) in a.iter().zip(b) {
                pending.push((ak, bk));
                pending.push((av, bv));
            }
            a.len() == b.len() && ac.is_some() == bc.is_some()
        }
        (V::Map(a), V::Map(b)) => {
            for ((ak, av), (bk, bv)) in a.iter().zip(b) {
                pending.push((ak, bk));
                pending.push((av, bv));
            }
            a.len() == b.len()
        }
        (V::Record(a), V::Record(b)) => {
            if a.len() != b.len() {
                return false;
            }
            for ((an, av), (bn, bv)) in a.iter().zip(b) {
                if an != bn {
                    return false;
                }
                pending.push((av, bv));
            }
            true
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
            pending.push((a, b));
            pending.push((c, d));
            x == y
        }
        (V::Message(a), V::Message(b)) => message(a, b, pending),
        (V::Conversation(a), V::Conversation(b)) => {
            a.selected_context == b.selected_context
                && a.session == b.session
                && a.history_fence == b.history_fence
                && a.cursor == b.cursor
                && a.messages.len() == b.messages.len()
                && a.messages
                    .iter()
                    .zip(&b.messages)
                    .all(|(a, b)| message(a, b, pending))
        }
        (
            V::Command {
                argv: a,
                env: c,
                cwd: e,
                stdin: g,
            },
            V::Command {
                argv: b,
                env: d,
                cwd: f,
                stdin: h,
            },
        ) => a == b && c == d && e == f && g == h,
        (
            V::CommandResult {
                exit_code: a,
                stdout: c,
                stderr: e,
            },
            V::CommandResult {
                exit_code: b,
                stdout: d,
                stderr: f,
            },
        ) => a == b && c == d && e == f,
        (V::Callable(a), V::Callable(b)) => {
            pending.0.push(Comparison::Target(a, b));
            true
        }
        (
            V::Handler {
                fact_expr: a,
                handlers: c,
            },
            V::Handler {
                fact_expr: b,
                handlers: d,
            },
        ) => a == b && c == d,
        (
            V::ResourceHandle {
                name: a,
                stable_id: c,
                ty: e,
            },
            V::ResourceHandle {
                name: b,
                stable_id: d,
                ty: f,
            },
        ) => a == b && c == d && e == f,
        (
            V::WorkspacePath {
                region: a,
                relative: c,
            },
            V::WorkspacePath {
                region: b,
                relative: d,
            },
        ) => a == b && c == d,
        (
            V::MemoryStore {
                region_stable_id: a,
                path: c,
                key_type: e,
                value_type: g,
            },
            V::MemoryStore {
                region_stable_id: b,
                path: d,
                key_type: f,
                value_type: h,
            },
        ) => a == b && c == d && e == f && g == h,
        (
            V::MemorySelection {
                region_stable_id: a,
                path: c,
                key_type: e,
                value_type: g,
                kind: i,
                predicate_type: o,
                predicate: k,
                limit: m,
            },
            V::MemorySelection {
                region_stable_id: b,
                path: d,
                key_type: f,
                value_type: h,
                kind: j,
                predicate_type: p,
                predicate: l,
                limit: n,
            },
        ) => {
            if a != b || c != d || e != f || g != h || i != j || m != n || o != p {
                return false;
            }
            match (k, l) {
                (Some(a), Some(b)) => {
                    pending.push((a, b));
                    true
                }
                (None, None) => true,
                _ => false,
            }
        }
        _ => false,
    }
}

fn message<'a>(a: &'a MessageSnapshot, b: &'a MessageSnapshot, pending: &mut Pending<'a>) -> bool {
    pending.push((&a.payload, &b.payload));
    a.id == b.id
        && a.from == b.from
        && a.to == b.to
        && a.role == b.role
        && a.session == b.session
        && a.created_at == b.created_at
        && a.provenance == b.provenance
}
