use super::super::frame::{
    DecodeRoot, FrameDecodeCache, runtime_frame_cache, snapshot_frame_cache,
};
use crate::{
    control::{CallTarget, Frame},
    orchestration::{CallTargetSnapshot, LocalsSnapshot},
};
use etas_hir::{HirExprId, HirItemId, SymbolId};
use etas_std::{IntrinsicDispatch, StdIntrinsicId};
use etas_types::TypeId;
use serde_json::Value;

pub(super) trait TargetValue: Sized {
    type Frame: Clone;
    fn frame_cache(
        limits: &etas_host::StorageLimits,
        root: DecodeRoot<'_>,
    ) -> Result<FrameDecodeCache<Self::Frame>, String>;
    fn frame(value: &Value, cache: &FrameDecodeCache<Self::Frame>) -> Result<Self::Frame, String>;
    fn build(target: DecodedTarget<Self::Frame, Self>) -> Self;
    fn agent_reference(
        reference: crate::orchestration::CheckpointAgentReference,
    ) -> Result<Self, String>;
    fn scoped(
        metadata: Box<crate::orchestration::ScopedCallableMetadata>,
        target: Self,
    ) -> Result<Self, String>;
}

pub(super) enum DecodedTarget<F, T> {
    FlowItem(HirItemId),
    AgentItem(HirItemId),
    ToolItem(HirItemId),
    EnumVariant(SymbolId),
    NominalConstructor(TypeId),
    Lambda {
        expr: HirExprId,
        captured: F,
    },
    PureIntrinsic {
        intrinsic: StdIntrinsicId,
        parameter_types: Vec<TypeId>,
        result_type: TypeId,
    },
    StdIntrinsic {
        origin: HirExprId,
        substitutions: Vec<Vec<(String, TypeId)>>,
        intrinsic: StdIntrinsicId,
        dispatch: IntrinsicDispatch,
        parameter_types: Vec<TypeId>,
        result_type: TypeId,
    },
    Specialized {
        target: T,
        type_bindings: Vec<(String, TypeId)>,
        effect_row_bindings: Vec<(String, etas_types::EffectRowRef)>,
    },
    Limited {
        target: T,
        limits: Vec<crate::eval::limit::RuntimeLimit>,
    },
    Composed(Vec<T>),
}

impl TargetValue for CallTargetSnapshot {
    type Frame = LocalsSnapshot;
    fn frame_cache(
        limits: &etas_host::StorageLimits,
        root: DecodeRoot<'_>,
    ) -> Result<FrameDecodeCache<Self::Frame>, String> {
        snapshot_frame_cache(limits, root)
    }
    fn scoped(
        metadata: Box<crate::orchestration::ScopedCallableMetadata>,
        target: Self,
    ) -> Result<Self, String> {
        Ok(Self::Scoped {
            metadata,
            target: target.into(),
        })
    }
    fn agent_reference(
        reference: crate::orchestration::CheckpointAgentReference,
    ) -> Result<Self, String> {
        Ok(Self::AgentRevision(Box::new(reference)))
    }
    fn frame(value: &Value, cache: &FrameDecodeCache<Self::Frame>) -> Result<Self::Frame, String> {
        cache.get(value)
    }
    fn build(target: DecodedTarget<Self::Frame, Self>) -> Self {
        match target {
            DecodedTarget::FlowItem(value) => Self::FlowItem(value),
            DecodedTarget::AgentItem(value) => Self::AgentItem(value),
            DecodedTarget::ToolItem(value) => Self::ToolItem(value),
            DecodedTarget::EnumVariant(value) => Self::EnumVariant(value),
            DecodedTarget::NominalConstructor(value) => Self::NominalConstructor(value),
            DecodedTarget::Composed(value) => Self::Composed(value.into()),
            DecodedTarget::Lambda { expr, captured } => Self::Lambda { expr, captured },
            DecodedTarget::Specialized {
                target,
                type_bindings,
                effect_row_bindings,
            } => Self::Specialized {
                target: target.into(),
                type_bindings,
                effect_row_bindings,
            },
            DecodedTarget::Limited { target, limits } => Self::Limited {
                target: target.into(),
                limits,
            },

            DecodedTarget::PureIntrinsic {
                intrinsic,
                parameter_types,
                result_type,
            } => Self::PureIntrinsic {
                intrinsic,
                parameter_types,
                result_type,
            },
            DecodedTarget::StdIntrinsic {
                origin,
                substitutions,
                intrinsic,
                dispatch,
                parameter_types,
                result_type,
            } => Self::StdIntrinsic {
                origin,
                substitutions,
                intrinsic,
                dispatch,
                parameter_types,
                result_type,
            },
        }
    }
}

impl TargetValue for CallTarget {
    type Frame = Frame;
    fn frame_cache(
        limits: &etas_host::StorageLimits,
        root: DecodeRoot<'_>,
    ) -> Result<FrameDecodeCache<Self::Frame>, String> {
        runtime_frame_cache(limits, root)
    }
    fn scoped(
        _: Box<crate::orchestration::ScopedCallableMetadata>,
        _: Self,
    ) -> Result<Self, String> {
        Err("checkpoint scoped callable requires checked native revision restoration".into())
    }
    fn agent_reference(_: crate::orchestration::CheckpointAgentReference) -> Result<Self, String> {
        Err("checkpoint agent reference requires checked native revision restoration".into())
    }
    fn frame(value: &Value, cache: &FrameDecodeCache<Self::Frame>) -> Result<Self::Frame, String> {
        cache.get(value)
    }
    fn build(target: DecodedTarget<Self::Frame, Self>) -> Self {
        match target {
            DecodedTarget::FlowItem(value) => Self::FlowItem(value),
            DecodedTarget::AgentItem(value) => Self::AgentItem(value),
            DecodedTarget::ToolItem(value) => Self::ToolItem(value),
            DecodedTarget::EnumVariant(value) => Self::EnumVariant(value),
            DecodedTarget::NominalConstructor(value) => Self::NominalConstructor(value),
            DecodedTarget::Composed(value) => Self::Composed(value.into()),
            DecodedTarget::Lambda { expr, captured } => Self::Lambda { expr, captured },
            DecodedTarget::Specialized {
                target,
                type_bindings,
                effect_row_bindings,
            } => Self::Specialized {
                target: target.into(),
                type_bindings,
                effect_row_bindings,
            },
            DecodedTarget::Limited { target, limits } => Self::Limited {
                target: target.into(),
                limits,
            },

            DecodedTarget::PureIntrinsic {
                intrinsic,
                parameter_types,
                result_type,
            } => Self::PureIntrinsic(crate::intrinsic::dispatch::CheckedPureIntrinsicCall {
                intrinsic,
                parameter_types,
                result_type,
            }),
            DecodedTarget::StdIntrinsic {
                origin,
                substitutions,
                intrinsic,
                dispatch,
                parameter_types,
                result_type,
            } => Self::StdIntrinsic(crate::intrinsic::dispatch::CheckedStdIntrinsicCall {
                origin,
                substitutions,
                identity: crate::intrinsic::dispatch::StdIntrinsicIdentity {
                    intrinsic,
                    dispatch,
                },
                parameter_types,
                result_type,
            }),
        }
    }
}
