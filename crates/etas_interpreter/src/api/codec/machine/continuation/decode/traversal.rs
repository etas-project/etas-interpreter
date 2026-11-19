use super::*;
use crate::orchestration::{
    ContinuationSnapshotLink, HandlerScopeId, ModelExecutionPolicySnapshot,
};

enum PendingParent<'a> {
    RestoreModelPolicy(Box<ModelExecutionPolicySnapshot>),
    CallBoundary(etas_core::Span, Option<crate::control::CallOrigin>),
    HandlerDispatch(Option<crate::value::ContinuationTokenValue>),
    ReenterHandler,
    HandleBoundary {
        scope_id: HandlerScopeId,
        value: &'a Value,
    },
    ScopedModelPolicy(Box<ModelExecutionPolicySnapshot>),
    AgentExecutionScope(Box<crate::orchestration::AgentExecutionStateSnapshot>),
    ChainInner(&'a Value),
    ChainOuter(ContinuationSnapshotLink),
}

pub(crate) fn continuation_from_snapshot(
    limits: &etas_host::StorageLimits,
    mut value: &Value,
    checked: &etas_frontend::CheckedProject,
) -> Result<ContinuationSnapshot, String> {
    let mut pending = Vec::new();
    loop {
        let kind = required_str(value, "kind")?;
        match kind {
            "restore_model_policy" => {
                let previous = Box::new(model_policy_from_snapshot(required(value, "previous")?)?);
                pending.push(PendingParent::RestoreModelPolicy(previous));
                value = required(value, "inner")?;
                continue;
            }
            "call_boundary" => {
                let caller = serde::Deserialize::deserialize(required(value, "caller")?)
                    .map_err(|error| format!("invalid call origin: {error}"))?;
                pending.push(PendingParent::CallBoundary(
                    span_from_snapshot(required(value, "span")?)?,
                    caller,
                ));
                value = required(value, "outer")?;
                continue;
            }
            "handler_dispatch" => {
                let deferred = serde::Deserialize::deserialize(required(value, "deferred")?)
                    .map_err(|error| format!("invalid deferred continuation token: {error}"))?;
                pending.push(PendingParent::HandlerDispatch(deferred));
                value = required(value, "outer")?;
                continue;
            }
            "reenter_handler" => {
                pending.push(PendingParent::ReenterHandler);
                value = required(value, "boundary")?;
                continue;
            }
            "handle_boundary" => {
                let scope_id = HandlerScopeId(required_u32(value, "scope_id")?);
                pending.push(PendingParent::HandleBoundary { scope_id, value });
                value = required(value, "inner")?;
                continue;
            }
            "agent_execution_scope" => {
                let state = Box::new(super::super::super::agent_context::state_from_json(
                    required(value, "state")?,
                )?);
                pending.push(PendingParent::AgentExecutionScope(state));
                value = required(value, "inner")?;
                continue;
            }
            "scoped_model_policy" => {
                let policy = Box::new(model_policy_from_snapshot(required(value, "policy")?)?);
                pending.push(PendingParent::ScopedModelPolicy(policy));
                value = required(value, "inner")?;
                continue;
            }
            "chain" => {
                pending.push(PendingParent::ChainInner(value));
                value = required(value, "inner")?;
                continue;
            }
            _ => {}
        }

        let mut node = continuation_leaf_from_snapshot(limits, value, checked, kind)?;
        loop {
            node = match pending.pop() {
                Some(PendingParent::RestoreModelPolicy(previous)) => {
                    ContinuationSnapshot::RestoreModelPolicy {
                        previous,
                        inner: node.into(),
                    }
                }
                Some(PendingParent::CallBoundary(span, caller)) => {
                    ContinuationSnapshot::CallBoundary {
                        caller,
                        span,
                        outer: node.into(),
                    }
                }
                Some(PendingParent::HandlerDispatch(deferred)) => {
                    ContinuationSnapshot::HandlerDispatch {
                        outer: node.into(),
                        deferred,
                    }
                }
                Some(PendingParent::ReenterHandler) => ContinuationSnapshot::ReenterHandler {
                    boundary: node.into(),
                },
                Some(PendingParent::HandleBoundary { scope_id, value }) => {
                    ContinuationSnapshot::HandleBoundary {
                        scope_id,
                        inner: node.into(),
                        handlers: required(value, "handlers")?
                            .as_array()
                            .ok_or_else(|| {
                                "machine snapshot `handlers` must be an array".to_owned()
                            })?
                            .iter()
                            .map(handler_arm_from_snapshot)
                            .collect::<Result<Vec<_>, _>>()?,
                        span: span_from_snapshot(required(value, "span")?)?,
                        frame: locals_from_snapshot(limits, required(value, "frame")?)?,
                    }
                }
                Some(PendingParent::AgentExecutionScope(state)) => {
                    ContinuationSnapshot::AgentExecutionScope {
                        state,
                        inner: node.into(),
                    }
                }
                Some(PendingParent::ScopedModelPolicy(policy)) => {
                    ContinuationSnapshot::ScopedModelPolicy {
                        policy,
                        inner: node.into(),
                    }
                }
                Some(PendingParent::ChainInner(parent)) => {
                    // Do not read outer before inner succeeds: keep error order and
                    // let captured sibling ownership clean up any later failure.
                    pending.push(PendingParent::ChainOuter(node.into()));
                    value = required(parent, "outer")?;
                    break;
                }
                Some(PendingParent::ChainOuter(inner)) => ContinuationSnapshot::Chain {
                    inner,
                    outer: node.into(),
                },
                None => return Ok(node),
            };
        }
    }
}
