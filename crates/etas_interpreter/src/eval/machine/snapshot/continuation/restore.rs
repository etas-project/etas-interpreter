use super::{Continuation, ContinuationSnapshot, RestoreContext, restore_model_policy};
use crate::control::ContinuationLink;
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
        inner: ContinuationLink,
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
                            inner: value.into(),
                        }
                    }
                    Some(PendingParent::CallBoundary) => Continuation::CallBoundary {
                        outer: value.into(),
                    },
                    Some(PendingParent::HandlerDispatch) => Continuation::HandlerDispatch {
                        outer: value.into(),
                    },
                    Some(PendingParent::HandleBoundary {
                        scope_id,
                        handlers,
                        span,
                        frame,
                    }) => {
                        // Normal link ownership also releases the inner tree if
                        // frame identity reconciliation fails.
                        let inner: ContinuationLink = value.into();
                        let frame = super::super::frame::restore_frame(frame, context)?;
                        Continuation::HandleBoundary {
                            scope_id,
                            inner,
                            handlers,
                            span,
                            frame,
                        }
                    }
                    Some(PendingParent::ScopedModelPolicy(policy)) => {
                        Continuation::ScopedModelPolicy {
                            policy: Box::new(restore_model_policy(*policy)),
                            inner: value.into(),
                        }
                    }
                    Some(PendingParent::ChainInner { outer }) => {
                        pending.push(PendingParent::ChainOuter {
                            inner: value.into(),
                        });
                        self = outer.into_value();
                        break;
                    }
                    Some(PendingParent::ChainOuter { inner }) => Continuation::Chain {
                        inner,
                        outer: value.into(),
                    },
                    None => return Ok(value),
                };
            }
        }
    }
}
