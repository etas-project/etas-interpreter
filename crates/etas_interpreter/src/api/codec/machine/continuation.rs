use etas_hir::{
    HirArg, HirBlockId, HirElseBranch, HirExprId, HirMatchArm, HirMatchArmBody, HirPatId,
    HirRangeBounds, HirStage, HirTypeId, ScopeId, SymbolId,
};
use serde_json::{Value, json};

use crate::{
    api::codec::value_json,
    control::Frame,
    orchestration::{
        ContinuationSnapshot, RetryAttemptId, RetryAttemptRecord, StaticMethodKindSnapshot,
    },
};

use super::{
    call_target::{call_target_snapshot_from_json, call_target_snapshots_from_json},
    frame::*,
    intrinsic::*,
    model::{model_policy_from_snapshot, model_policy_snapshot, optional_model_policy},
    value::*,
};

mod decode;
mod encode;
mod support;
#[cfg(test)]
mod tests;

pub(crate) use decode::continuation_from_snapshot;
pub(in crate::api::codec) use encode::write_continuation;
pub(in crate::api::codec) use support::runtime_limit_snapshot;
pub(super) use support::{frame_snapshot, runtime_limits_from_snapshot};
