use super::{CallTarget, CallTargetSnapshot, RestoreContext, restore_leaf};
use crate::eval::limit::RuntimeLimit;
use crate::{
    control::{CallTargetChildren, CallTargetLink},
    orchestration::{CallTargetSnapshotChildren, CallTargetSnapshotLink},
};
use etas_types::TypeId;

enum PendingParent {
    Scoped {
        metadata: Box<crate::orchestration::ScopedCallableMetadata>,
        parent: etas_frontend::CheckedProgram,
        native: crate::value::CheckedExecutionProgram,
        projections: Vec<std::sync::Arc<crate::plan::code::CallableValueProjection>>,
    },
    Specialized(
        Vec<(String, TypeId)>,
        Vec<(String, etas_types::EffectRowRef)>,
        Option<CallTargetSnapshotLink>,
    ),
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
            CallTargetSnapshot::Scoped { metadata, target } => {
                let parent = context
                    .revision_program
                    .clone()
                    .ok_or("checkpoint callable has no current checked program")?;
                let revisions = context
                    .revisions
                    .as_ref()
                    .ok_or("checkpoint callable requires freshly prepared revisions")?;
                let projections = revisions.callable_projections(&metadata, &parent)?;
                let native = revisions.execution_callable(&metadata, &target)?;
                context.select_program(native.checked.clone());
                pending.push(PendingParent::Scoped {
                    metadata,
                    parent,
                    native,
                    projections,
                });
                current = target.into_value();
                continue;
            }
            CallTargetSnapshot::Specialized {
                target,
                type_bindings,
                effect_row_bindings,
            } => {
                let identity = target.shared_identity();
                if let Some((_, restored)) =
                    identity.and_then(|key| context.caches.call_nodes.get(&key))
                {
                    CallTarget::Specialized {
                        target: restored.clone(),
                        type_bindings,
                        effect_row_bindings,
                    }
                } else {
                    pending.push(PendingParent::Specialized(
                        type_bindings,
                        effect_row_bindings,
                        identity.map(|_| target.clone()),
                    ));
                    current = target.into_value();
                    continue;
                }
            }
            CallTargetSnapshot::Limited { target, limits } => {
                let identity = target.shared_identity();
                if let Some((_, restored)) =
                    identity.and_then(|key| context.caches.call_nodes.get(&key))
                {
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
                if let Some((_, restored)) =
                    identity.and_then(|key| context.caches.call_tables.get(&key))
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
                Some(PendingParent::Scoped {
                    metadata,
                    parent,
                    native,
                    projections,
                }) => {
                    context.select_program(parent);
                    value =
                        CallTarget::Scoped(crate::value::ScopedCallableValue::restore_checkpoint(
                            native,
                            metadata.native_type,
                            value,
                            projections,
                        )?);
                }
                Some(PendingParent::Specialized(type_bindings, effect_row_bindings, source)) => {
                    let target: CallTargetLink = value.into();
                    if let Some(source) = source {
                        context
                            .caches
                            .call_nodes
                            .insert((&*source) as *const _, (source, target.clone()));
                    }
                    value = CallTarget::Specialized {
                        target,
                        type_bindings,
                        effect_row_bindings,
                    };
                }
                Some(PendingParent::Limited(limits, source)) => {
                    let target: CallTargetLink = value.into();
                    if let Some(source) = source {
                        context
                            .caches
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
                        context
                            .caches
                            .call_tables
                            .insert(key, (source, children.clone()));
                    }
                    value = CallTarget::Composed(children);
                }
                None => return Ok(value),
            }
        }
    }
}
