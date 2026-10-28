use super::{InterpValue as Value, ListIter, MessageValue, identity::ValueIdentity};
use crate::control::{CallTarget, Frame};
use std::{
    collections::{HashSet, vec_deque},
    slice,
};

#[derive(Clone, Copy)]
pub(crate) enum Node<'a> {
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
pub(crate) struct FrameDiscovery {
    values: HashSet<ValueIdentity>,
    nodes: HashSet<*const CallTarget>,
    tables: HashSet<*const Vec<CallTarget>>,
}

impl FrameDiscovery {
    pub(crate) fn frames(
        &mut self,
        root: Node<'_>,
        follow_scoped: bool,
        skip: impl FnMut(&ValueIdentity) -> bool,
    ) -> Vec<Frame> {
        self.discover(root, follow_scoped, skip, |_, frame| frame.clone())
    }

    pub(crate) fn captures(&mut self, root: Node<'_>) -> Vec<(etas_hir::HirExprId, Frame)> {
        self.discover(root, false, |_| false, |expr, frame| (expr, frame.clone()))
    }

    fn discover<T>(
        &mut self,
        root: Node<'_>,
        follow_scoped: bool,
        mut skip: impl FnMut(&ValueIdentity) -> bool,
        mut capture: impl FnMut(etas_hir::HirExprId, &Frame) -> T,
    ) -> Vec<T> {
        let mut output = Vec::new();
        let mut next = Some(root);
        let mut parents = Vec::new();
        loop {
            if let Some(node) = next.take() {
                let mut children = match node {
                    Node::Target(CallTarget::Lambda { expr, captured }) => {
                        output.push(capture(*expr, captured));
                        Children::One(None)
                    }
                    Node::Target(CallTarget::Scoped(call)) => {
                        Children::One(follow_scoped.then_some(Node::Target(call.target_ref())))
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
                        if let Some(key) = ValueIdentity::of(value)
                            && (skip(&key) || !self.values.insert(key))
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
                    return output;
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
