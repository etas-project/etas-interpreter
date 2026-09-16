use super::super::capture_context::CaptureContext;
use super::{Continuation, ContinuationSnapshot, capture_model_policy};
use crate::{control::ContinuationLink, orchestration::ContinuationSnapshotLink};

type Identity = Option<std::ptr::NonNull<Continuation>>;

enum PendingParent<'a> {
    Unary {
        parent: &'a Continuation,
        identity: Identity,
    },
    ChainInner {
        outer: &'a ContinuationLink,
        identity: Identity,
    },
    ChainOuter {
        inner: ContinuationSnapshotLink,
        identity: Identity,
    },
}

impl ContinuationSnapshot {
    #[cfg(test)]
    pub(crate) fn capture(current: &Continuation) -> Result<Self, String> {
        CaptureContext::default().continuation(current)
    }

    pub(in crate::eval::machine::snapshot) fn capture_with(
        mut current: &Continuation,
        context: &mut CaptureContext,
    ) -> Result<Self, String> {
        // Only completed shared children are memoized. Unique trees retain just
        // their DFS frontier, while shared DAGs are captured once per node.
        let mut pending = Vec::new();
        loop {
            let mut value = match current {
                Continuation::RestoreModelPolicy { inner, .. }
                | Continuation::HandleBoundary { inner, .. }
                | Continuation::ScopedModelPolicy { inner, .. }
                | Continuation::CallBoundary { outer: inner }
                | Continuation::HandlerDispatch { outer: inner } => {
                    if let Some(saved) = saved_link(inner, context) {
                        capture_parent(current, saved, context)?
                    } else {
                        pending.push(PendingParent::Unary {
                            parent: current,
                            identity: inner.shared_capture_identity(),
                        });
                        current = inner;
                        continue;
                    }
                }
                Continuation::Chain { inner, outer } => {
                    if let Some(inner) = saved_link(inner, context) {
                        if let Some(outer) = saved_link(outer, context) {
                            Self::Chain { inner, outer }
                        } else {
                            pending.push(PendingParent::ChainOuter {
                                inner,
                                identity: outer.shared_capture_identity(),
                            });
                            current = outer;
                            continue;
                        }
                    } else {
                        pending.push(PendingParent::ChainInner {
                            outer,
                            identity: inner.shared_capture_identity(),
                        });
                        current = inner;
                        continue;
                    }
                }
                _ => Self::capture_leaf(current, context)?,
            };
            loop {
                match pending.pop() {
                    Some(PendingParent::Unary { parent, identity }) => {
                        let child = completed_link(value, identity, context);
                        value = capture_parent(parent, child, context)?
                    }
                    Some(PendingParent::ChainInner { outer, identity }) => {
                        let inner = completed_link(value, identity, context);
                        if let Some(outer) = saved_link(outer, context) {
                            value = Self::Chain { inner, outer };
                        } else {
                            pending.push(PendingParent::ChainOuter {
                                inner,
                                identity: outer.shared_capture_identity(),
                            });
                            current = outer;
                            break;
                        }
                    }
                    Some(PendingParent::ChainOuter { inner, identity }) => {
                        value = Self::Chain {
                            inner,
                            outer: completed_link(value, identity, context),
                        };
                    }
                    None => return Ok(value),
                }
            }
        }
    }
}

fn saved_link(
    link: &ContinuationLink,
    context: &CaptureContext,
) -> Option<ContinuationSnapshotLink> {
    link.shared_capture_identity()
        .and_then(|key| context.continuations.get(&key))
        .cloned()
}

fn completed_link(
    value: ContinuationSnapshot,
    identity: Identity,
    context: &mut CaptureContext,
) -> ContinuationSnapshotLink {
    let link: ContinuationSnapshotLink = value.into();
    if let Some(identity) = identity {
        context.continuations.insert(identity, link.clone());
    }
    link
}

fn capture_parent(
    parent: &Continuation,
    child: ContinuationSnapshotLink,
    context: &mut CaptureContext,
) -> Result<ContinuationSnapshot, String> {
    Ok(match parent {
        Continuation::RestoreModelPolicy { previous, .. } => {
            ContinuationSnapshot::RestoreModelPolicy {
                previous: Box::new(capture_model_policy(previous)),
                inner: child,
            }
        }
        Continuation::CallBoundary { .. } => ContinuationSnapshot::CallBoundary { outer: child },
        Continuation::HandlerDispatch { .. } => {
            ContinuationSnapshot::HandlerDispatch { outer: child }
        }
        Continuation::HandleBoundary {
            scope_id,
            handlers,
            span,
            frame,
            ..
        } => ContinuationSnapshot::HandleBoundary {
            scope_id: *scope_id,
            inner: child,
            handlers: handlers.clone(),
            span: *span,
            frame: context.frame(frame)?,
        },
        Continuation::ScopedModelPolicy { policy, .. } => ContinuationSnapshot::ScopedModelPolicy {
            policy: Box::new(capture_model_policy(policy)),
            inner: child,
        },
        _ => return Err("invalid unary continuation capture parent".into()),
    })
}
