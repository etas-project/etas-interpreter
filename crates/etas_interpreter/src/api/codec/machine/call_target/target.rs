use super::super::frame::{locals_from_snapshot, runtime_frame_from_snapshot};
use crate::{
    control::{CallTarget, Frame},
    orchestration::{CallTargetSnapshot, LocalsSnapshot},
};
use etas_hir::{HirExprId, HirItemId, SymbolId};
use etas_std::{IntrinsicDispatch, StdIntrinsicId};
use etas_types::TypeId;
use serde_json::Value;

pub(super) trait TargetValue: Sized {
    type Frame;
    fn frame(limits: &etas_host::StorageLimits, value: &Value) -> Result<Self::Frame, String>;
    fn build(target: DecodedTarget<Self::Frame, Self>) -> Self;
}

pub(super) enum DecodedTarget<F, T> {
    FlowItem(HirItemId),
    AgentItem(HirItemId),
    ToolItem(HirItemId),
    SpecImplMethod(SymbolId),
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
        intrinsic: StdIntrinsicId,
        dispatch: IntrinsicDispatch,
        parameter_types: Vec<TypeId>,
        result_type: TypeId,
    },
    Specialized {
        target: Box<T>,
        type_bindings: Vec<(String, TypeId)>,
    },
    Limited {
        target: Box<T>,
        limits: Vec<crate::eval::limit::RuntimeLimit>,
    },
    Composed(Vec<T>),
}

impl TargetValue for CallTargetSnapshot {
    type Frame = LocalsSnapshot;
    fn frame(limits: &etas_host::StorageLimits, value: &Value) -> Result<Self::Frame, String> {
        locals_from_snapshot(limits, value)
    }
    fn build(target: DecodedTarget<Self::Frame, Self>) -> Self {
        match target {
            DecodedTarget::FlowItem(value) => Self::FlowItem(value),
            DecodedTarget::AgentItem(value) => Self::AgentItem(value),
            DecodedTarget::ToolItem(value) => Self::ToolItem(value),
            DecodedTarget::SpecImplMethod(value) => Self::SpecImplMethod(value),
            DecodedTarget::EnumVariant(value) => Self::EnumVariant(value),
            DecodedTarget::NominalConstructor(value) => Self::NominalConstructor(value),
            DecodedTarget::Composed(value) => Self::Composed(value),
            DecodedTarget::Lambda { expr, captured } => Self::Lambda { expr, captured },
            DecodedTarget::Specialized {
                target,
                type_bindings,
            } => Self::Specialized {
                target,
                type_bindings,
            },
            DecodedTarget::Limited { target, limits } => Self::Limited { target, limits },

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
                intrinsic,
                dispatch,
                parameter_types,
                result_type,
            } => Self::StdIntrinsic {
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
    fn frame(limits: &etas_host::StorageLimits, value: &Value) -> Result<Self::Frame, String> {
        runtime_frame_from_snapshot(limits, value)
    }
    fn build(target: DecodedTarget<Self::Frame, Self>) -> Self {
        match target {
            DecodedTarget::FlowItem(value) => Self::FlowItem(value),
            DecodedTarget::AgentItem(value) => Self::AgentItem(value),
            DecodedTarget::ToolItem(value) => Self::ToolItem(value),
            DecodedTarget::SpecImplMethod(value) => Self::SpecImplMethod(value),
            DecodedTarget::EnumVariant(value) => Self::EnumVariant(value),
            DecodedTarget::NominalConstructor(value) => Self::NominalConstructor(value),
            DecodedTarget::Composed(value) => Self::Composed(value),
            DecodedTarget::Lambda { expr, captured } => Self::Lambda { expr, captured },
            DecodedTarget::Specialized {
                target,
                type_bindings,
            } => Self::Specialized {
                target,
                type_bindings,
            },
            DecodedTarget::Limited { target, limits } => Self::Limited { target, limits },

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
                intrinsic,
                dispatch,
                parameter_types,
                result_type,
            } => Self::StdIntrinsic(crate::intrinsic::dispatch::CheckedStdIntrinsicCall {
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
