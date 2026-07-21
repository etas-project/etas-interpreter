use std::sync::Arc;

use etas_hir::{
    HirArg, HirBlockId, HirElseBranch, HirExprId, HirFieldInit, HirMapEntry, HirMatchArm,
    HirMatchArmBody, HirPatId, HirRangeBounds, HirStage, HirTypeId, ResolveResult, ScopeId,
    SymbolId,
};
use serde_json::{Value, json};

use crate::{
    api::codec::{value_from_json, value_json},
    control::{Continuation, Frame, StaticMethodKind},
    orchestration::{RetryAttemptId, RetryAttemptRecord},
    plan::SlotLayoutTable,
};

use super::{
    call_target::{call_target_from_snapshot, call_target_snapshot, call_targets_from_snapshot},
    frame::*,
    intrinsic::*,
    model::{model_policy_from_snapshot, model_policy_snapshot, optional_model_policy},
    value::*,
};

mod decode;
mod encode;
mod support;

pub(crate) use decode::continuation_from_snapshot;
pub(crate) use encode::continuation_snapshot;
pub(super) use support::{frame_snapshot, runtime_limit_snapshot, runtime_limits_from_snapshot};
