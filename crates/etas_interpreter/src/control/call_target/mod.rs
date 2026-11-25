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
    BoundMethod {
        target: CallTargetLink,
        receiver: crate::value::SharedValue,
        slot: u32,
    },
    EnumVariant(SymbolId),
    DerivedResponseDecoder(etas_types::TypeId),
    NominalConstructor(etas_types::TypeId),
    PureIntrinsic(CheckedPureIntrinsicCall),
    StdIntrinsic(CheckedStdIntrinsicCall),
    Specialized {
        target: CallTargetLink,
        type_bindings: Vec<(String, etas_types::TypeId)>,
        type_parameter_bindings: Vec<(etas_types::TypeId, etas_types::TypeId)>,
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

impl CallTarget {
    /// Instance identity never compares captured values or executes user code.
    pub(crate) fn same_instance(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::FlowItem(a), Self::FlowItem(b)) => a == b,
            (
                Self::Lambda {
                    expr: a,
                    captured: ac,
                },
                Self::Lambda {
                    expr: b,
                    captured: bc,
                },
            ) => a == b && ac.capture_identity() == bc.capture_identity(),
            (Self::Scoped(a), Self::Scoped(b)) => a.same_instance(b),
            (
                Self::BoundMethod {
                    target: a,
                    receiver: ar,
                    slot: ai,
                },
                Self::BoundMethod {
                    target: b,
                    receiver: br,
                    slot: bi,
                },
            ) => ai == bi && std::ptr::eq(ar.as_ref(), br.as_ref()) && a.same_instance(b),
            (Self::Specialized { target: a, .. }, Self::Specialized { target: b, .. })
            | (Self::Limited { target: a, .. }, Self::Limited { target: b, .. }) => {
                std::ptr::eq(&**a, &**b)
            }
            (Self::Composed(a), Self::Composed(b)) => std::ptr::eq(a.as_ptr(), b.as_ptr()),
            // These targets have a canonical checked identity and no captures.
            (Self::PureIntrinsic(a), Self::PureIntrinsic(b)) => a == b,
            (Self::StdIntrinsic(a), Self::StdIntrinsic(b)) => a == b,
            _ => false,
        }
    }
}
