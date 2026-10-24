use crate::orchestration::{MessageSnapshot, ValueSnapshot};
mod membership;

// Runtime Set equality is order-independent; snapshot equality below remains
// order-sensitive so conflicting definitions of a frame cannot change iteration.
pub(super) fn membership_equal(a: &ValueSnapshot, b: &ValueSnapshot) -> bool {
    membership::equal(a, b)
}

impl PartialEq for ValueSnapshot {
    fn eq(&self, other: &Self) -> bool {
        let mut pending = vec![(self, other)];
        while let Some((left, right)) = pending.pop() {
            if !same_node(left, right, &mut pending) {
                return false;
            }
        }
        true
    }
}

fn same_node<'a>(
    a: &'a ValueSnapshot,
    b: &'a ValueSnapshot,
    pending: &mut Vec<(&'a ValueSnapshot, &'a ValueSnapshot)>,
) -> bool {
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
        | (V::Stack(a), V::Stack(b))
        | (V::OrderedSet(a), V::OrderedSet(b)) => {
            pending.extend(a.iter().zip(b));
            a.len() == b.len()
        }
        (V::Variant { name: a, fields: x }, V::Variant { name: b, fields: y }) => {
            pending.extend(x.iter().zip(y));
            a == b && x.len() == y.len()
        }
        (V::Map(a), V::Map(b))
        | (V::OrderedMap(a), V::OrderedMap(b))
        | (V::PriorityQueue(a), V::PriorityQueue(b)) => {
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
        (V::Callable(a), V::Callable(b)) => a == b,
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
                predicate: k,
                limit: m,
            },
            V::MemorySelection {
                region_stable_id: b,
                path: d,
                key_type: f,
                value_type: h,
                kind: j,
                predicate: l,
                limit: n,
            },
        ) => {
            if a != b || c != d || e != f || g != h || i != j || m != n {
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

fn message<'a>(
    a: &'a MessageSnapshot,
    b: &'a MessageSnapshot,
    pending: &mut Vec<(&'a ValueSnapshot, &'a ValueSnapshot)>,
) -> bool {
    pending.push((&a.payload, &b.payload));
    a.id == b.id
        && a.from == b.from
        && a.to == b.to
        && a.role == b.role
        && a.session == b.session
        && a.created_at == b.created_at
        && a.provenance == b.provenance
}
