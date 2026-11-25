use etas_hir::{HirExprId, HirItemId};

use super::{CallTarget, CheckedActionInvocation, Continuation, Frame, GenericBindings};
use crate::value::{AgentReference, InterpValue};

/// Engine-owned origins. These are not action payloads available to Etas code.
#[derive(Clone, Debug)]
pub(crate) enum ManagedAgentOrigin {
    Create {
        expr: HirExprId,
        bindings: GenericBindings,
    },
    Call {
        item: HirItemId,
        receiver: etas_types::TypeId,
        bindings: GenericBindings,
    },
}

impl ManagedAgentOrigin {
    pub(crate) fn name(&self) -> &'static str {
        match self {
            Self::Create { .. } => "create",
            Self::Call { .. } => "call",
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) enum ManagedAgentOperation {
    Initialize {
        expr: HirExprId,
        frame: Frame,
    },
    Submit {
        reference: AgentReference,
        updating: bool,
        autonomous: bool,
        target: CallTarget,
        args: Vec<InterpValue>,
    },
}

#[derive(Clone, Debug)]
pub struct PendingManagedAgent {
    pub(crate) proof: CheckedActionInvocation,
    pub(crate) origin: ManagedAgentOrigin,
    pub(crate) operation: ManagedAgentOperation,
    pub(crate) span: etas_core::Span,
    pub(crate) continuation: Continuation,
    pub(crate) autonomous_completion: Option<Box<Continuation>>,
    pub(crate) residual: Option<super::residual::ResidualAttemptTag>,
}

/// Suspended admission retains the issuing request's live accounts and grants.
#[derive(Clone, Debug)]
pub struct PendingManagedAuthorization {
    pub(crate) original: PendingManagedAgent,
    pub(crate) authorization: super::PolicyAuthorization,
    pub(crate) context: Option<crate::eval::request_context::AgentHostContext>,
    pub(crate) budget: etas_host::ExecutionBudget,
    pub(crate) request: Option<crate::driver::scheduler::TaskHandle>,
    pub(crate) event: crate::orchestration::WorkflowEvent,
}
