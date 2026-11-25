use std::collections::VecDeque;

/// One boundary can require several independent policy contexts and approvals.
#[derive(Clone, Debug)]
pub(crate) struct PolicyAuthorization {
    pub(crate) policies: VecDeque<(etas_host::HostValue, etas_host::PolicyContext)>,
    pub(crate) subject: etas_host::PolicySubject,
    pub(crate) approval: Option<etas_host::ApprovalRequest>,
    pub(crate) requested_grants: Option<Vec<etas_host::HostActionGrant>>,
}

/// Prepared native requests retain their replay key across authorization.
#[derive(Clone, Debug)]
pub(crate) enum NativeBoundaryTarget {
    Console {
        pending: super::PendingConsole,
        key: etas_host::replay::ReplayFingerprint,
    },
    Command {
        pending: super::PendingCommand,
        key: etas_host::replay::ReplayFingerprint,
    },
    Memory {
        pending: super::PendingMemory,
        key: etas_host::replay::ReplayFingerprint,
    },
    Host {
        pending: super::PendingHostBoundary,
        key: Option<etas_host::replay::ReplayFingerprint>,
    },
    Model(super::PendingModel),
    Tool(crate::eval::machine::PendingTool),
    Session {
        pending: super::PendingSession,
        key: etas_host::replay::ReplayFingerprint,
    },
    Compiler {
        invocation: crate::eval::CompilerInvocation,
        perform: super::PendingPerform,
    },
}

impl NativeBoundaryTarget {
    pub(crate) fn span(&self) -> etas_core::Span {
        match self {
            Self::Console { pending, .. } => pending.span,
            Self::Command { pending, .. } => pending.span,
            Self::Memory { pending, .. } => pending.span,
            Self::Host { pending, .. } => pending.span,
            Self::Model(pending) => pending.span,
            Self::Tool(pending) => pending.span,
            Self::Session { pending, .. } => pending.span,
            Self::Compiler { perform, .. } => perform.span,
        }
    }
    pub(crate) fn continuation_mut(&mut self) -> Option<&mut super::Continuation> {
        Some(match self {
            Self::Console { pending, .. } => &mut pending.continuation,
            Self::Command { pending, .. } => &mut pending.continuation,
            Self::Memory { pending, .. } => &mut pending.continuation,
            Self::Host { pending, .. } => &mut pending.continuation,
            Self::Model(pending) => &mut pending.continuation,
            Self::Tool(_) => return None,
            Self::Session { pending, .. } => &mut pending.continuation,
            Self::Compiler { perform, .. } => &mut perform.continuation,
        })
    }
}

#[derive(Clone, Debug)]
pub struct PendingNativeAuthorization {
    pub(crate) target: NativeBoundaryTarget,
    pub(crate) checks: VecDeque<PolicyAuthorization>,
    pub(crate) context: Option<crate::eval::request_context::AgentHostContext>,
    pub(crate) budget: etas_host::ExecutionBudget,
}

/// Only the authorization driver creates this transient dispatch capability.
#[derive(Clone, Debug)]
pub struct AuthorizedNativeBoundary {
    pub(crate) target: NativeBoundaryTarget,
    pub(crate) context: Option<crate::eval::request_context::AgentHostContext>,
}
