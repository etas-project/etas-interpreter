use std::{
    collections::{HashSet, vec_deque},
    slice,
};

use super::capture_identity::CaptureIdentity;
use crate::{
    control::{CallTarget, Frame},
    value::{InterpValue as Value, ListIter, MessageValue},
};

pub(super) fn snapshot_frames(
    value: &crate::orchestration::ValueSnapshot,
    output: &mut Vec<(crate::orchestration::LocalsSnapshot, bool)>,
) {
    use crate::orchestration::{CallTargetSnapshot as Target, ValueSnapshot};
    let mut nodes = HashSet::new();
    let mut tables = HashSet::new();
    let mut frames = HashSet::new();
    for value in value.walk_unique() {
        let ValueSnapshot::Callable(root) = value else {
            continue;
        };
        let mut pending = vec![root];
        while let Some(target) = pending.pop() {
            match target {
                Target::Lambda { captured, .. } => {
                    if frames.insert((captured.id, std::rc::Rc::as_ptr(&captured.locals))) {
                        output.push((captured.clone(), false));
                    }
                }
                Target::Specialized { target, .. } | Target::Limited { target, .. } => {
                    if target.shared_identity().is_none_or(|key| nodes.insert(key)) {
                        pending.push(target);
                    }
                }
                Target::Composed(targets) => {
                    if targets
                        .shared_identity()
                        .is_none_or(|key| tables.insert(key))
                    {
                        pending.extend(targets.iter().rev());
                    }
                }
                // Scoped targets are checked using their native program's validator.
                _ => (),
            }
        }
    }
}

enum Node<'a> {
    Value(&'a Value),
    Target(&'a CallTarget),
}

enum Children<'a> {
    One(Option<Node<'a>>),
    Two(std::array::IntoIter<&'a Value, 2>),
    Values(slice::Iter<'a, Value>),
    Fields(slice::Iter<'a, (String, Value)>),
    Deque(vec_deque::Iter<'a, Value>),
    List(ListIter<'a>),
    Pairs {
        values: slice::Iter<'a, (Value, Value)>,
        next: Option<&'a Value>,
    },
    Ordered {
        values: crate::value::ordered::Iter<'a>,
        next: Option<&'a Value>,
        comparator: Option<&'a CallTarget>,
    },
    Priority {
        values: crate::value::priority_queue::PriorityQueueIter<'a>,
        next: Option<&'a Value>,
        comparator: Option<&'a CallTarget>,
    },
    Messages(slice::Iter<'a, MessageValue>),
    Targets(slice::Iter<'a, CallTarget>),
}

impl<'a> Iterator for Children<'a> {
    type Item = Node<'a>;
    fn next(&mut self) -> Option<Self::Item> {
        Some(match self {
            Self::One(value) => return value.take(),
            Self::Two(values) => Node::Value(values.next()?),
            Self::Values(values) => Node::Value(values.next()?),
            Self::Fields(values) => Node::Value(&values.next()?.1),
            Self::Deque(values) => Node::Value(values.next()?),
            Self::List(values) => Node::Value(values.next()?),
            Self::Pairs { values, next } => {
                if let Some(value) = next.take() {
                    return Some(Node::Value(value));
                }
                let (key, value) = values.next()?;
                *next = Some(value);
                Node::Value(key)
            }
            Self::Ordered {
                values,
                next,
                comparator,
            } => {
                if let Some(target) = comparator.take() {
                    return Some(Node::Target(target));
                }
                if let Some(value) = next.take() {
                    return Some(Node::Value(value));
                }
                let (key, value) = values.next()?;
                *next = Some(value);
                Node::Value(key)
            }
            Self::Priority {
                values,
                next,
                comparator,
            } => {
                if let Some(target) = comparator.take() {
                    return Some(Node::Target(target));
                }
                if let Some(value) = next.take() {
                    return Some(Node::Value(value));
                }
                let (key, value) = values.next()?;
                *next = Some(value);
                Node::Value(key)
            }
            Self::Messages(values) => Node::Value(&values.next()?.payload),
            Self::Targets(values) => Node::Target(values.next()?),
        })
    }
}

#[derive(Default)]
pub(super) struct Discovery {
    values: HashSet<CaptureIdentity>,
    nodes: HashSet<*const CallTarget>,
    tables: HashSet<*const Vec<CallTarget>>,
}

impl Discovery {
    pub(super) fn frames(
        &mut self,
        root: &Value,
        captured: &std::collections::HashMap<CaptureIdentity, crate::orchestration::ValueSnapshot>,
        output: &mut Vec<(Frame, bool)>,
    ) {
        let mut next = Some(Node::Value(root));
        let mut parents = Vec::new();
        loop {
            if let Some(node) = next.take() {
                let mut children = match node {
                    Node::Target(CallTarget::Lambda { captured, .. }) => {
                        output.push((captured.clone(), false));
                        Children::One(None)
                    }
                    Node::Target(CallTarget::Scoped(call)) => {
                        Children::One(Some(Node::Target(call.target_ref())))
                    }
                    Node::Target(
                        CallTarget::Limited { target, .. } | CallTarget::Specialized { target, .. },
                    ) => Children::One(
                        target
                            .shared_identity()
                            .is_none_or(|key| self.nodes.insert(key))
                            .then_some(Node::Target(target)),
                    ),
                    Node::Target(CallTarget::Composed(targets)) => {
                        if targets
                            .shared_identity()
                            .is_some_and(|key| !self.tables.insert(key))
                        {
                            continue;
                        }
                        Children::Targets(targets.iter())
                    }
                    Node::Target(_) => Children::One(None),
                    Node::Value(value) => {
                        if let Some(key) = CaptureIdentity::of(value)
                            && (captured.contains_key(&key) || !self.values.insert(key))
                        {
                            continue;
                        }
                        value_children(value)
                    }
                };
                if let Some(child) = children.next() {
                    parents.push(children);
                    next = Some(child);
                    continue;
                }
            }
            loop {
                let Some(parent) = parents.last_mut() else {
                    return;
                };
                if let Some(child) = parent.next() {
                    next = Some(child);
                    break;
                }
                parents.pop();
            }
        }
    }
}

fn value_children(value: &Value) -> Children<'_> {
    match value {
        Value::Callable(target) => Children::One(Some(Node::Target(target))),
        Value::Nominal { value, .. } | Value::Trust { value, .. } | Value::OptionSome(value) => {
            Children::One(Some(Node::Value(value)))
        }
        Value::Tuple(values) | Value::Variant { fields: values, .. } => {
            Children::Values(values.iter())
        }
        Value::Array(values) | Value::Stack(values) => Children::Values(values.borrow().iter()),
        Value::Slice(values) => Children::Values(values.borrow().iter()),
        Value::List(values) => Children::List(values.iter()),
        Value::Set(values) => Children::Values(values.borrow().iter()),
        Value::OrderedSet(values) => Children::Ordered {
            values: values.iter(),
            next: None,
            comparator: values.comparator(),
        },
        Value::OrderedMap(values) => Children::Ordered {
            values: values.iter(),
            next: None,
            comparator: values.comparator(),
        },
        Value::Map(values) => Children::Pairs {
            values: values.borrow().iter(),
            next: None,
        },
        Value::PriorityQueue(values) => Children::Priority {
            values: values.iter(),
            next: None,
            comparator: values.comparator(),
        },
        Value::Deque(values) | Value::Queue(values) => Children::Deque(values.borrow().iter()),
        Value::Record(values) => Children::Fields(values.borrow().iter()),
        Value::Range(value) => {
            Children::Two([value.start.as_ref(), value.end.as_ref()].into_iter())
        }
        Value::Message(value) => Children::One(Some(Node::Value(&value.payload))),
        Value::Conversation(value) => Children::Messages(value.messages.iter()),
        Value::MemorySelection { predicate, .. } => {
            Children::One(predicate.as_deref().map(Node::Value))
        }
        _ => Children::One(None),
    }
}
