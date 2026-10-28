use super::{Children, Context, Cursor, ordered::OrderedComparison, set_comparison, target};
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

pub(super) fn compare_node<'c>(
    mut a: &V,
    mut b: &V,
    context: &'c Context,
) -> Comparison<Cursor<'c>> {
    context.visit();
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
            (V::PriorityQueue(av), V::PriorityQueue(bv)) => {
                if av.len() != bv.len() {
                    return Comparison::Ready(false);
                }
                break Children::PriorityQueue {
                    a: av.clone(),
                    b: bv.clone(),
                    a_slot: 0,
                    b_slot: 0,
                };
            }
            (V::Map(av), V::Map(bv)) => {
                if av.borrow().len() != bv.borrow().len() {
                    return Comparison::Ready(false);
                }
                break Children::Map(av.clone(), bv.clone());
            }
            (V::OrderedMap(av), V::OrderedMap(bv)) => {
                if av.len() != bv.len() {
                    return Comparison::Ready(false);
                }
                break Children::Ordered(Box::new(OrderedComparison::map(av, bv)));
            }
            (V::Record(av), V::Record(bv)) => {
                if av.borrow().len() != bv.borrow().len() {
                    return Comparison::Ready(false);
                }
                break Children::Record(av.clone(), bv.clone());
            }
            (V::Set(av), V::Set(bv)) => {
                return set_comparison(av, bv, context);
            }
            (V::OrderedSet(av), V::OrderedSet(bv)) => {
                if av.len() != bv.len() {
                    return Comparison::Ready(false);
                }
                break Children::Ordered(Box::new(OrderedComparison::set(av, bv)));
            }
            (V::Callable(a), V::Callable(b)) => return target::target(a, b, context),
            (
                V::MemorySelection {
                    region_stable_id: ar,
                    path: ap,
                    key_type: ak,
                    value_type: av,
                    kind: ai,
                    predicate: a_pred,
                    limit: al,
                },
                V::MemorySelection {
                    region_stable_id: br,
                    path: bp,
                    key_type: bk,
                    value_type: bv,
                    kind: bi,
                    predicate: b_pred,
                    limit: bl,
                },
            ) => {
                if ar != br || ap != bp || ak != bk || av != bv || ai != bi || al != bl {
                    return Comparison::Ready(false);
                }
                match (a_pred, b_pred) {
                    (Some(av), Some(bv)) => {
                        a = av;
                        b = bv;
                    }
                    (None, None) => return Comparison::Ready(true),
                    _ => return Comparison::Ready(false),
                }
            }
            _ => return Comparison::Ready(leaf_equal(a, b)),
        }
    };
    context.pending(children)
}

fn leaf_equal(a: &V, b: &V) -> bool {
    match (a, b) {
        (V::Unit, V::Unit) | (V::OptionNone, V::OptionNone) => true,
        (V::Bool(a), V::Bool(b)) => a == b,
        (V::Number(a), V::Number(b)) => a == b,
        (V::String(a), V::String(b)) => a == b,
        (V::Bytes(a), V::Bytes(b)) => a == b,
        (V::Json(a), V::Json(b)) => a == b,
        (V::SyntaxBlock(a), V::SyntaxBlock(b)) => a == b,
        (V::CodeMetadata(a), V::CodeMetadata(b)) => a == b,
        (V::AgentDefinition(a), V::AgentDefinition(b)) => a == b,
        (V::CallableDefinition(a), V::CallableDefinition(b)) => a == b,
        (V::Prompt(a), V::Prompt(b)) => a == b,
        (V::Schema(a), V::Schema(b)) => a == b,
        (V::Provenance(a), V::Provenance(b)) => a == b,
        (V::ModelResponse(a), V::ModelResponse(b)) => a == b,
        (V::Range(a), V::Range(b)) => a == b,
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
        _ => false,
    }
}
