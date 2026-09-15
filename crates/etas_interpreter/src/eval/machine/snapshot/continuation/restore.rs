use super::{Continuation, ContinuationSnapshot, RestoreContext, restore_model_policy};
use crate::orchestration::{
    ActiveHandlerArmRecord, ContinuationSnapshotLink, HandlerScopeId, LocalsSnapshot,
    ModelExecutionPolicySnapshot,
};

enum PendingParent {
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
    ChainInner {
        outer: ContinuationSnapshotLink,
    },
    ChainOuter {
        inner: RestoredEdge,
    },
}

impl ContinuationSnapshot {
    pub(crate) fn restore_with(
        mut self,
        context: &mut RestoreContext,
    ) -> Result<Continuation, String> {
        let mut pending = Vec::new();
        loop {
            match self {
                Self::RestoreModelPolicy { previous, inner } => {
                    pending.push(PendingParent::RestoreModelPolicy(previous));
                    self = inner.into_value();
                    continue;
                }
                Self::CallBoundary { outer } => {
                    pending.push(PendingParent::CallBoundary);
                    self = outer.into_value();
                    continue;
                }
                Self::HandlerDispatch { outer } => {
                    pending.push(PendingParent::HandlerDispatch);
                    self = outer.into_value();
                    continue;
                }
                Self::HandleBoundary {
                    scope_id,
                    inner,
                    handlers,
                    span,
                    frame,
                } => {
                    pending.push(PendingParent::HandleBoundary {
                        scope_id,
                        handlers,
                        span,
                        frame,
                    });
                    self = inner.into_value();
                    continue;
                }
                Self::ScopedModelPolicy { policy, inner } => {
                    pending.push(PendingParent::ScopedModelPolicy(policy));
                    self = inner.into_value();
                    continue;
                }
                Self::Chain { inner, outer } => {
                    pending.push(PendingParent::ChainInner { outer });
                    self = inner.into_value();
                    continue;
                }
                _ => {}
            }

            let mut value = self.restore_leaf(context)?;
            loop {
                value = match pending.pop() {
                    Some(PendingParent::RestoreModelPolicy(previous)) => {
                        Continuation::RestoreModelPolicy {
                            previous: Box::new(restore_model_policy(*previous)),
                            inner: Box::new(value),
                        }
                    }
                    Some(PendingParent::CallBoundary) => Continuation::CallBoundary {
                        outer: Box::new(value),
                    },
                    Some(PendingParent::HandlerDispatch) => Continuation::HandlerDispatch {
                        outer: Box::new(value),
                    },
                    Some(PendingParent::HandleBoundary {
                        scope_id,
                        handlers,
                        span,
                        frame,
                    }) => {
                        // Frame identity reconciliation can fail after the inner tree
                        // was restored. Keep that tree guarded until it succeeds.
                        let inner = RestoredEdge::new(value);
                        let frame = super::super::frame::restore_frame(frame, context)?;
                        Continuation::HandleBoundary {
                            scope_id,
                            inner: inner.into_box(),
                            handlers,
                            span,
                            frame,
                        }
                    }
                    Some(PendingParent::ScopedModelPolicy(policy)) => {
                        Continuation::ScopedModelPolicy {
                            policy: Box::new(restore_model_policy(*policy)),
                            inner: Box::new(value),
                        }
                    }
                    Some(PendingParent::ChainInner { outer }) => {
                        pending.push(PendingParent::ChainOuter {
                            inner: RestoredEdge::new(value),
                        });
                        self = outer.into_value();
                        break;
                    }
                    Some(PendingParent::ChainOuter { inner }) => Continuation::Chain {
                        inner: inner.into_box(),
                        outer: Box::new(value),
                    },
                    None => return Ok(value),
                };
            }
        }
    }
}

// The Box becomes the final runtime edge on success. On an error, this owner
// releases completed continuation branches without relying on recursive Drop.
struct RestoredEdge(Option<Box<Continuation>>);

impl RestoredEdge {
    fn new(value: Continuation) -> Self {
        Self(Some(Box::new(value)))
    }

    fn into_box(mut self) -> Box<Continuation> {
        self.0.take().expect("live restored continuation edge")
    }
}

impl Drop for RestoredEdge {
    fn drop(&mut self) {
        let mut next = self.0.take();
        let mut pending = Vec::new();
        while let Some(node) = next.take().or_else(|| pending.pop()) {
            match *node {
                Continuation::CallBoundary { outer } | Continuation::HandlerDispatch { outer } => {
                    next = Some(outer)
                }
                Continuation::RestoreModelPolicy { inner, .. }
                | Continuation::HandleBoundary { inner, .. }
                | Continuation::ScopedModelPolicy { inner, .. } => next = Some(inner),
                Continuation::Chain { inner, outer } => {
                    next = Some(inner);
                    pending.push(outer);
                }
                _ => {}
            }
        }
    }
}
