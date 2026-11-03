use etas_hir::{HirExprId, HirItemId, SymbolId};

use crate::{
    control::Frame,
    intrinsic::dispatch::{CheckedPureIntrinsicCall, CheckedStdIntrinsicCall},
};

mod storage;
pub use storage::{CallTargetChildren, CallTargetLink};

#[derive(Clone, Debug)]
pub enum CallTarget {
    FlowItem(HirItemId),
    AgentItem(HirItemId),
    AgentRevision(crate::value::AgentRevisionValue),
    Scoped(crate::value::ScopedCallableValue),
    ToolItem(HirItemId),
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
        effect_row_bindings: Vec<(String, etas_types::EffectRowRef)>,
    },
    Limited {
        target: CallTargetLink,
        limits: Vec<crate::eval::limit::RuntimeLimit>,
    },
    Composed(CallTargetChildren),
}

impl PartialEq for CallTarget {
    fn eq(&self, other: &Self) -> bool {
        crate::value::comparison::targets_equal(self, other)
    }
}
impl Eq for CallTarget {}
