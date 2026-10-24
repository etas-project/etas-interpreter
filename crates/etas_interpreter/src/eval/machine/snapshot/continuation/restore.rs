use super::{Continuation, ContinuationSnapshot, RestoreContext, restore_model_policy};
use crate::control::ContinuationLink;
use crate::orchestration::{
    ActiveHandlerArmRecord, ContinuationSnapshotLink, HandlerScopeId, LocalsSnapshot,
    ModelExecutionPolicySnapshot,
};

enum UnaryParent {
    RestoreModelPolicy(Box<ModelExecutionPolicySnapshot>),
    CallBoundary,
    HandlerDispatch,
    HandleBoundary {
        scope_id: HandlerScopeId,
        handlers: Vec<ActiveHandlerArmRecord>,
        span: etas_core::Span,
        frame: LocalsSnapshot,
    },
    ScopedModelPolicy(Box<ModelExecutionPolicySnapshot>),
}

enum PendingParent {
    Unary {
        parent: UnaryParent,
        source: Option<ContinuationSnapshotLink>,
    },
    ChainInner {
        outer: ContinuationSnapshotLink,
        source: Option<ContinuationSnapshotLink>,
    },
    ChainOuter {
        inner: ContinuationLink,
        source: Option<ContinuationSnapshotLink>,
    },
}

impl ContinuationSnapshot {
    pub(crate) fn restore_with(
        mut self,
        context: &mut RestoreContext,
    ) -> Result<Continuation, String> {
        let mut pending = Vec::new();
        'walk: loop {
            let mut value = 'restore: {
                let (parent, child) = match self {
                    Self::RestoreModelPolicy { previous, inner } => {
                        (UnaryParent::RestoreModelPolicy(previous), inner)
                    }
                    Self::CallBoundary { outer } => (UnaryParent::CallBoundary, outer),
                    Self::HandlerDispatch { outer } => (UnaryParent::HandlerDispatch, outer),
                    Self::HandleBoundary {
                        scope_id,
                        inner,
                        handlers,
                        span,
                        frame,
                    } => (
                        UnaryParent::HandleBoundary {
                            scope_id,
                            handlers,
                            span,
                            frame,
                        },
                        inner,
                    ),
                    Self::ScopedModelPolicy { policy, inner } => {
                        (UnaryParent::ScopedModelPolicy(policy), inner)
                    }
                    Self::Chain { inner, outer } => {
                        if let Some(inner) = restored_link(&inner, context) {
                            if let Some(outer) = restored_link(&outer, context) {
                                break 'restore Continuation::Chain { inner, outer };
                            }
                            pending.push(PendingParent::ChainOuter {
                                inner,
                                source: retain_shared(&outer),
                            });
                            self = outer.into_value();
                        } else {
                            let source = retain_shared(&inner);
                            pending.push(PendingParent::ChainInner { outer, source });
                            self = inner.into_value();
                        }
                        continue 'walk;
                    }
                    leaf => break 'restore leaf.restore_leaf(context)?,
                };
                if let Some(child) = restored_link(&child, context) {
                    break 'restore restore_parent(parent, child, context)?;
                }
                pending.push(PendingParent::Unary {
                    parent,
                    source: retain_shared(&child),
                });
                self = child.into_value();
                continue 'walk;
            };
            loop {
                match pending.pop() {
                    Some(PendingParent::Unary { parent, source }) => {
                        let child = completed_link(value, source, context);
                        value = restore_parent(parent, child, context)?;
                    }
                    Some(PendingParent::ChainInner { outer, source }) => {
                        let inner = completed_link(value, source, context);
                        if let Some(outer) = restored_link(&outer, context) {
                            value = Continuation::Chain { inner, outer };
                        } else {
                            pending.push(PendingParent::ChainOuter {
                                inner,
                                source: retain_shared(&outer),
                            });
                            self = outer.into_value();
                            break;
                        }
                    }
                    Some(PendingParent::ChainOuter { inner, source }) => {
                        value = Continuation::Chain {
                            inner,
                            outer: completed_link(value, source, context),
                        };
                    }
                    None => return Ok(value),
                }
            }
        }
    }
}

fn retain_shared(link: &ContinuationSnapshotLink) -> Option<ContinuationSnapshotLink> {
    link.shared_identity().map(|_| link.clone())
}

fn restored_link(
    link: &ContinuationSnapshotLink,
    context: &RestoreContext,
) -> Option<ContinuationLink> {
    link.shared_identity()
        .and_then(|key| context.continuations.get(&key))
        .map(|(_, restored)| restored.clone())
}

fn completed_link(
    value: Continuation,
    source: Option<ContinuationSnapshotLink>,
    context: &mut RestoreContext,
) -> ContinuationLink {
    let link: ContinuationLink = value.into();
    if let Some(source) = source {
        context.continuations.insert(
            std::ptr::NonNull::from(source.as_ref()),
            (source, link.clone()),
        );
    }
    link
}

fn restore_parent(
    parent: UnaryParent,
    child: ContinuationLink,
    context: &mut RestoreContext,
) -> Result<Continuation, String> {
    Ok(match parent {
        UnaryParent::RestoreModelPolicy(previous) => Continuation::RestoreModelPolicy {
            previous: Box::new(restore_model_policy(*previous)),
            inner: child,
        },
        UnaryParent::CallBoundary => Continuation::CallBoundary { outer: child },
        UnaryParent::HandlerDispatch => Continuation::HandlerDispatch { outer: child },
        UnaryParent::HandleBoundary {
            scope_id,
            handlers,
            span,
            frame,
        } => Continuation::HandleBoundary {
            scope_id,
            inner: child,
            handlers,
            span,
            frame: super::super::frame::restore_frame(frame, context)?,
        },
        UnaryParent::ScopedModelPolicy(policy) => Continuation::ScopedModelPolicy {
            policy: Box::new(restore_model_policy(*policy)),
            inner: child,
        },
    })
}
