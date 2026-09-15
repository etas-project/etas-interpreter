use etas_hir::{HirExprId, HirItemId, SymbolId};

use crate::{
    control::Frame,
    intrinsic::dispatch::{CheckedPureIntrinsicCall, CheckedStdIntrinsicCall},
};

mod storage;
pub use storage::{CallTargetChildren, CallTargetLink};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CallTarget {
    FlowItem(HirItemId),
    AgentItem(HirItemId),
    ToolItem(HirItemId),
    SpecImplMethod(SymbolId),
    Lambda {
        expr: HirExprId,
        captured: Frame,
    },
    EnumVariant(SymbolId),
    NominalConstructor(etas_types::TypeId),
    PureIntrinsic(CheckedPureIntrinsicCall),
    StdIntrinsic(CheckedStdIntrinsicCall),
    Specialized {
        target: CallTargetLink,
        type_bindings: Vec<(String, etas_types::TypeId)>,
    },
    Limited {
        target: CallTargetLink,
        limits: Vec<crate::eval::limit::RuntimeLimit>,
    },
    Composed(CallTargetChildren),
}
