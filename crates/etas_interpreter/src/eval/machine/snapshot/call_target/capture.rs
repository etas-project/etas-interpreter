use super::{CallTarget, CallTargetSnapshot, capture_leaf};
use crate::eval::limit::RuntimeLimit;
use etas_types::TypeId;

enum PendingParent<'a> {
    Specialized(&'a [(String, TypeId)]),
    Limited(&'a [RuntimeLimit]),
    Composed {
        remaining: &'a [CallTarget],
        values: Vec<CallTargetSnapshot>,
    },
}

pub(in crate::eval::machine::snapshot) fn capture_call_target(
    mut current: &CallTarget,
) -> Result<CallTargetSnapshot, String> {
    let mut pending = Vec::new();
    loop {
        let mut value = match current {
            CallTarget::Specialized {
                target,
                type_bindings,
            } => {
                pending.push(PendingParent::Specialized(type_bindings));
                current = target;
                continue;
            }
            CallTarget::Limited { target, limits } => {
                pending.push(PendingParent::Limited(limits));
                current = target;
                continue;
            }
            CallTarget::Composed(targets) => {
                if let Some((first, remaining)) = targets.split_first() {
                    // This buffer becomes the final snapshot child table. Completed
                    // siblings release through snapshot ownership on any later error.
                    pending.push(PendingParent::Composed {
                        remaining,
                        values: Vec::with_capacity(targets.len()),
                    });
                    current = first;
                    continue;
                }
                CallTargetSnapshot::Composed(vec![].into())
            }
            leaf => capture_leaf(leaf)?,
        };
        loop {
            match pending.pop() {
                Some(PendingParent::Specialized(bindings)) => {
                    value = CallTargetSnapshot::Specialized {
                        target: value.into(),
                        type_bindings: bindings.to_vec(),
                    };
                }
                Some(PendingParent::Limited(limits)) => {
                    value = CallTargetSnapshot::Limited {
                        target: value.into(),
                        limits: limits.to_vec(),
                    };
                }
                Some(PendingParent::Composed {
                    remaining,
                    mut values,
                }) => {
                    values.push(value);
                    if let Some((first, remaining)) = remaining.split_first() {
                        pending.push(PendingParent::Composed { remaining, values });
                        current = first;
                        break;
                    }
                    value = CallTargetSnapshot::Composed(values.into());
                }
                None => return Ok(value),
            }
        }
    }
}
