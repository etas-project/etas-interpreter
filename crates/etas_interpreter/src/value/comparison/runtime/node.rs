use super::{Children, Cursor, set_comparison};
use crate::value::{InterpValue as V, comparison::Comparison};

pub(super) fn message_headers_equal(
    a: &crate::value::MessageValue,
    b: &crate::value::MessageValue,
) -> bool {
    a.id == b.id
        && a.from == b.from
        && a.to == b.to
        && a.role == b.role
        && a.session == b.session
        && a.created_at == b.created_at
        && a.provenance == b.provenance
}

pub(super) fn compare_node(mut a: &V, mut b: &V) -> Comparison<Cursor> {
    let children = loop {
        match (a, b) {
            (V::Nominal { ty: at, value: av }, V::Nominal { ty: bt, value: bv }) => {
                if at != bt {
                    return Comparison::Ready(false);
                }
                a = av;
                b = bv;
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
            }
            (V::OptionSome(av), V::OptionSome(bv)) => {
                a = av;
                b = bv;
            }
            (V::Message(av), V::Message(bv)) => {
                if !message_headers_equal(av, bv) {
                    return Comparison::Ready(false);
                }
                a = &av.payload;
                b = &bv.payload;
            }
            (V::Conversation(av), V::Conversation(bv)) => {
                if av.session != bv.session
                    || av.history_fence != bv.history_fence
                    || av.cursor != bv.cursor
                    || av.selected_context != bv.selected_context
                    || av.messages.len() != bv.messages.len()
                {
                    return Comparison::Ready(false);
                }
                if std::ptr::eq(av.messages.as_ptr(), bv.messages.as_ptr()) {
                    return Comparison::Ready(true);
                }
                break Children::Messages(av.messages.clone(), bv.messages.clone());
            }
            (V::Tuple(av), V::Tuple(bv)) => {
                if av.len() != bv.len() {
                    return Comparison::Ready(false);
                }
                break Children::Fields(av.clone(), bv.clone());
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
                break Children::Fields(av.clone(), bv.clone());
            }
            (V::Array(av), V::Array(bv)) | (V::Stack(av), V::Stack(bv)) => {
                if av.borrow().len() != bv.borrow().len() {
                    return Comparison::Ready(false);
                }
                break Children::Array(av.clone(), bv.clone());
            }
            (V::Slice(av), V::Slice(bv)) => {
                if av.borrow().len() != bv.borrow().len() {
                    return Comparison::Ready(false);
                }
                break Children::Slice(av.clone(), bv.clone());
            }
            (V::Deque(av), V::Deque(bv)) | (V::Queue(av), V::Queue(bv)) => {
                if av.borrow().len() != bv.borrow().len() {
                    return Comparison::Ready(false);
                }
                break Children::Deque(av.clone(), bv.clone());
            }
            (V::List(av), V::List(bv)) => {
                if av.len() != bv.len() {
                    return Comparison::Ready(false);
                }
                break Children::List(av.clone(), bv.clone());
            }
            (V::Map(av), V::Map(bv))
            | (V::OrderedMap(av), V::OrderedMap(bv))
            | (V::PriorityQueue(av), V::PriorityQueue(bv)) => {
                if av.borrow().len() != bv.borrow().len() {
                    return Comparison::Ready(false);
                }
                break Children::Map(av.clone(), bv.clone());
            }
            (V::Record(av), V::Record(bv)) => {
                if av.borrow().len() != bv.borrow().len() {
                    return Comparison::Ready(false);
                }
                break Children::Record(av.clone(), bv.clone());
            }
            (V::Set(av), V::Set(bv)) | (V::OrderedSet(av), V::OrderedSet(bv)) => {
                return set_comparison(av, bv);
            }
            _ => return Comparison::Ready(leaf_equal(a, b)),
        }
    };
    Comparison::Pending(Cursor { children, index: 0 })
}

fn leaf_equal(a: &V, b: &V) -> bool {
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
        (V::Range(a), V::Range(b)) => a == b,
        (V::Callable(a), V::Callable(b)) => a == b,
        (V::HostHandle(a), V::HostHandle(b)) => a == b,
        (V::MemoryWriteIntent(a), V::MemoryWriteIntent(b)) => a == b,
        (V::WorkspacePath(a), V::WorkspacePath(b)) => a == b,
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
        ) => a == b && c == d && e == f && g == h && i == j && k == l && m == n,
        _ => false,
    }
}
