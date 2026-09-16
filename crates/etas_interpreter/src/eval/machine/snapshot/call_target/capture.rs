use super::super::capture_context::CaptureContext;
use super::{CallTarget, CallTargetSnapshot, capture_leaf};
use crate::eval::limit::RuntimeLimit;
use crate::orchestration::{CallTargetSnapshotChildren, CallTargetSnapshotLink};
use etas_types::TypeId;

enum PendingParent<'a> {
    Specialized(&'a [(String, TypeId)], Option<*const CallTarget>),
    Limited(&'a [RuntimeLimit], Option<*const CallTarget>),
    Composed {
        remaining: &'a [CallTarget],
        values: Vec<CallTargetSnapshot>,
        identity: Option<*const Vec<CallTarget>>,
    },
}

pub(in crate::eval::machine::snapshot) fn capture_call_target(
    current: &CallTarget,
) -> Result<CallTargetSnapshot, String> {
    capture_call_target_with(current, &mut CaptureContext::default())
}

pub(in crate::eval::machine::snapshot) fn capture_call_target_with(
    mut current: &CallTarget,
    context: &mut CaptureContext,
) -> Result<CallTargetSnapshot, String> {
    let mut pending = Vec::new();
    loop {
        let mut value = match current {
            CallTarget::Specialized {
                target,
                type_bindings,
            } => {
                let identity = target.shared_identity();
                if let Some(saved) = identity.and_then(|key| context.call_nodes.get(&key)) {
                    CallTargetSnapshot::Specialized {
                        target: saved.clone(),
                        type_bindings: type_bindings.clone(),
                    }
                } else {
                    pending.push(PendingParent::Specialized(type_bindings, identity));
                    current = target;
                    continue;
                }
            }
            CallTarget::Limited { target, limits } => {
                let identity = target.shared_identity();
                if let Some(saved) = identity.and_then(|key| context.call_nodes.get(&key)) {
                    CallTargetSnapshot::Limited {
                        target: saved.clone(),
                        limits: limits.clone(),
                    }
                } else {
                    pending.push(PendingParent::Limited(limits, identity));
                    current = target;
                    continue;
                }
            }
            CallTarget::Composed(targets) => {
                let identity = targets.shared_identity();
                if let Some(saved) = identity.and_then(|key| context.call_tables.get(&key)) {
                    CallTargetSnapshot::Composed(saved.clone())
                } else if let Some((first, remaining)) = targets.split_first() {
                    // This buffer becomes the final snapshot child table. Completed
                    // siblings release through snapshot ownership on any later error.
                    pending.push(PendingParent::Composed {
                        remaining,
                        values: Vec::with_capacity(targets.len()),
                        identity,
                    });
                    current = first;
                    continue;
                } else {
                    CallTargetSnapshot::Composed(vec![].into())
                }
            }
            leaf => capture_leaf(leaf, context)?,
        };
        loop {
            match pending.pop() {
                Some(PendingParent::Specialized(bindings, identity)) => {
                    let target: CallTargetSnapshotLink = value.into();
                    if let Some(key) = identity {
                        context.call_nodes.insert(key, target.clone());
                    }
                    value = CallTargetSnapshot::Specialized {
                        target,
                        type_bindings: bindings.to_vec(),
                    };
                }
                Some(PendingParent::Limited(limits, identity)) => {
                    let target: CallTargetSnapshotLink = value.into();
                    if let Some(key) = identity {
                        context.call_nodes.insert(key, target.clone());
                    }
                    value = CallTargetSnapshot::Limited {
                        target,
                        limits: limits.to_vec(),
                    };
                }
                Some(PendingParent::Composed {
                    remaining,
                    mut values,
                    identity,
                }) => {
                    values.push(value);
                    if let Some((first, remaining)) = remaining.split_first() {
                        pending.push(PendingParent::Composed {
                            remaining,
                            values,
                            identity,
                        });
                        current = first;
                        break;
                    }
                    let children: CallTargetSnapshotChildren = values.into();
                    if let Some(key) = identity {
                        context.call_tables.insert(key, children.clone());
                    }
                    value = CallTargetSnapshot::Composed(children);
                }
                None => return Ok(value),
            }
        }
    }
}
