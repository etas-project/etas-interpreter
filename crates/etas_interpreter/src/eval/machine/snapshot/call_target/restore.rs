use super::{CallTarget, CallTargetSnapshot, RestoreContext, restore_leaf};
use crate::eval::limit::RuntimeLimit;
use crate::{
    control::{CallTargetChildren, CallTargetLink},
    orchestration::{CallTargetSnapshotChildren, CallTargetSnapshotLink},
};
use etas_types::TypeId;

enum PendingParent {
    Specialized(Vec<(String, TypeId)>, Option<CallTargetSnapshotLink>),
    Limited(Vec<RuntimeLimit>, Option<CallTargetSnapshotLink>),
    Composed {
        remaining: std::vec::IntoIter<CallTargetSnapshot>,
        values: Vec<CallTarget>,
        source: Option<(*const Vec<CallTargetSnapshot>, CallTargetSnapshotChildren)>,
    },
}

pub(in crate::eval::machine::snapshot) fn restore_call_target(
    mut current: CallTargetSnapshot,
    context: &mut RestoreContext,
) -> Result<CallTarget, String> {
    let mut pending = Vec::new();
    loop {
        let mut value = match current {
            CallTargetSnapshot::Specialized {
                target,
                type_bindings,
            } => {
                let identity = target.shared_identity();
                if let Some((_, restored)) = identity.and_then(|key| context.call_nodes.get(&key)) {
                    CallTarget::Specialized {
                        target: restored.clone(),
                        type_bindings,
                    }
                } else {
                    pending.push(PendingParent::Specialized(
                        type_bindings,
                        identity.map(|_| target.clone()),
                    ));
                    current = target.into_value();
                    continue;
                }
            }
            CallTargetSnapshot::Limited { target, limits } => {
                let identity = target.shared_identity();
                if let Some((_, restored)) = identity.and_then(|key| context.call_nodes.get(&key)) {
                    CallTarget::Limited {
                        target: restored.clone(),
                        limits,
                    }
                } else {
                    pending.push(PendingParent::Limited(
                        limits,
                        identity.map(|_| target.clone()),
                    ));
                    current = target.into_value();
                    continue;
                }
            }
            CallTargetSnapshot::Composed(targets) => {
                let identity = targets.shared_identity();
                if let Some((_, restored)) = identity.and_then(|key| context.call_tables.get(&key))
                {
                    CallTarget::Composed(restored.clone())
                } else {
                    let source = identity.map(|key| (key, targets.clone()));
                    let mut remaining = targets.into_values().into_iter();
                    let count = remaining.len();
                    if let Some(first) = remaining.next() {
                        pending.push(PendingParent::Composed {
                            remaining,
                            values: Vec::with_capacity(count),
                            source,
                        });
                        current = first;
                        continue;
                    }
                    CallTarget::Composed(vec![].into())
                }
            }
            leaf => restore_leaf(leaf, context)?,
        };
        loop {
            match pending.pop() {
                Some(PendingParent::Specialized(type_bindings, source)) => {
                    let target: CallTargetLink = value.into();
                    if let Some(source) = source {
                        context
                            .call_nodes
                            .insert((&*source) as *const _, (source, target.clone()));
                    }
                    value = CallTarget::Specialized {
                        target,
                        type_bindings,
                    };
                }
                Some(PendingParent::Limited(limits, source)) => {
                    let target: CallTargetLink = value.into();
                    if let Some(source) = source {
                        context
                            .call_nodes
                            .insert((&*source) as *const _, (source, target.clone()));
                    }
                    value = CallTarget::Limited { target, limits };
                }
                Some(PendingParent::Composed {
                    mut remaining,
                    mut values,
                    source,
                }) => {
                    values.push(value);
                    if let Some(first) = remaining.next() {
                        pending.push(PendingParent::Composed {
                            remaining,
                            values,
                            source,
                        });
                        current = first;
                        break;
                    }
                    let children: CallTargetChildren = values.into();
                    if let Some((key, source)) = source {
                        context.call_tables.insert(key, (source, children.clone()));
                    }
                    value = CallTarget::Composed(children);
                }
                None => return Ok(value),
            }
        }
    }
}
