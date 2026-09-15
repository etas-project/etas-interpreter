use super::{CallTarget, CallTargetSnapshot, RestoreContext, restore_leaf};
use crate::eval::limit::RuntimeLimit;
use etas_types::TypeId;

enum PendingParent {
    Specialized(Vec<(String, TypeId)>),
    Limited(Vec<RuntimeLimit>),
    Composed {
        remaining: std::vec::IntoIter<CallTargetSnapshot>,
        values: RestoredTargets,
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
                pending.push(PendingParent::Specialized(type_bindings));
                current = target.into_value();
                continue;
            }
            CallTargetSnapshot::Limited { target, limits } => {
                pending.push(PendingParent::Limited(limits));
                current = target.into_value();
                continue;
            }
            CallTargetSnapshot::Composed(targets) => {
                let mut remaining = targets.into_values().into_iter();
                let count = remaining.len();
                if let Some(first) = remaining.next() {
                    pending.push(PendingParent::Composed {
                        remaining,
                        values: RestoredTargets(Vec::with_capacity(count)),
                    });
                    current = first;
                    continue;
                }
                CallTarget::Composed(vec![])
            }
            leaf => restore_leaf(leaf, context)?,
        };
        loop {
            match pending.pop() {
                Some(PendingParent::Specialized(type_bindings)) => {
                    value = CallTarget::Specialized {
                        target: Box::new(value),
                        type_bindings,
                    };
                }
                Some(PendingParent::Limited(limits)) => {
                    value = CallTarget::Limited {
                        target: Box::new(value),
                        limits,
                    };
                }
                Some(PendingParent::Composed {
                    mut remaining,
                    mut values,
                }) => {
                    values.0.push(value);
                    if let Some(first) = remaining.next() {
                        pending.push(PendingParent::Composed { remaining, values });
                        current = first;
                        break;
                    }
                    value = CallTarget::Composed(std::mem::take(&mut values.0));
                }
                None => return Ok(value),
            }
        }
    }
}

// This becomes the final runtime composition on success. On failure it owns
// completed siblings and releases their edges without recursive runtime Drop.
struct RestoredTargets(Vec<CallTarget>);

impl Drop for RestoredTargets {
    fn drop(&mut self) {
        crate::control::release_call_targets(std::mem::take(&mut self.0));
    }
}
