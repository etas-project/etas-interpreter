use super::{Continuation, ContinuationSnapshot, capture_model_policy};
use crate::orchestration::ContinuationSnapshotLink;

enum PendingParent<'a> {
    Unary(&'a Continuation),
    ChainInner { outer: &'a Continuation },
    ChainOuter { inner: ContinuationSnapshotLink },
}

impl ContinuationSnapshot {
    pub(crate) fn capture(mut current: &Continuation) -> Result<Self, String> {
        // Keep only the DFS frontier and completed siblings, never an intermediate
        // runtime tree or a second table of all captured snapshot nodes.
        let mut pending = Vec::new();
        loop {
            match current {
                Continuation::RestoreModelPolicy { inner, .. }
                | Continuation::HandleBoundary { inner, .. }
                | Continuation::ScopedModelPolicy { inner, .. } => {
                    pending.push(PendingParent::Unary(current));
                    current = inner;
                    continue;
                }
                Continuation::CallBoundary { outer } | Continuation::HandlerDispatch { outer } => {
                    pending.push(PendingParent::Unary(current));
                    current = outer;
                    continue;
                }
                Continuation::Chain { inner, outer } => {
                    pending.push(PendingParent::ChainInner { outer });
                    current = inner;
                    continue;
                }
                _ => {}
            }

            let mut value = Self::capture_leaf(current)?;
            loop {
                match pending.pop() {
                    Some(PendingParent::Unary(parent)) => value = capture_parent(parent, value)?,
                    Some(PendingParent::ChainInner { outer }) => {
                        pending.push(PendingParent::ChainOuter {
                            inner: value.into(),
                        });
                        current = outer;
                        break;
                    }
                    Some(PendingParent::ChainOuter { inner }) => {
                        value = Self::Chain {
                            inner,
                            outer: value.into(),
                        };
                    }
                    None => return Ok(value),
                }
            }
        }
    }
}

fn capture_parent(
    parent: &Continuation,
    child: ContinuationSnapshot,
) -> Result<ContinuationSnapshot, String> {
    Ok(match parent {
        Continuation::RestoreModelPolicy { previous, .. } => {
            ContinuationSnapshot::RestoreModelPolicy {
                previous: Box::new(capture_model_policy(previous)),
                inner: child.into(),
            }
        }
        Continuation::CallBoundary { .. } => ContinuationSnapshot::CallBoundary {
            outer: child.into(),
        },
        Continuation::HandlerDispatch { .. } => ContinuationSnapshot::HandlerDispatch {
            outer: child.into(),
        },
        Continuation::HandleBoundary {
            scope_id,
            handlers,
            span,
            frame,
            ..
        } => ContinuationSnapshot::HandleBoundary {
            scope_id: *scope_id,
            inner: child.into(),
            handlers: handlers.clone(),
            span: *span,
            frame: super::super::frame::capture_frame(frame)?,
        },
        Continuation::ScopedModelPolicy { policy, .. } => ContinuationSnapshot::ScopedModelPolicy {
            policy: Box::new(capture_model_policy(policy)),
            inner: child.into(),
        },
        _ => return Err("invalid unary continuation capture parent".into()),
    })
}
