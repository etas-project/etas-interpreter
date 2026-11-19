mod action;
mod agent_context;
mod agent_operation;
pub use agent_operation::CheckedAgentOperation;
pub(crate) use agent_operation::{AgentOperationKind, AgentOperationOrigin};
mod call_site;
mod call_target;
mod continuation;
mod fault;
mod frame;
mod generics;
pub(crate) mod handler_activation;
mod receiver;
pub(crate) mod residual;
mod scope;
mod signal;
pub use scope::ScopeExitGuard;

pub use action::{
    CheckedActionInvocation, DefaultActionBoundary, DirectoryListReduction, FileReadReduction,
    StreamReadReduction,
};
pub use agent_context::{AgentExecutionState, AgentRequestContext};
pub(crate) use call_site::{CallOrigin, ComposedInvocation, InvocationPath};
pub use call_site::{CallSite, CallSiteKind};
pub use call_target::CallTarget;
pub(crate) use call_target::{CallTargetChildren, CallTargetLink};

#[cfg(test)]
mod call_target_tests;
#[cfg(test)]
mod continuation_tests;

pub use continuation::{Continuation, ContinuationLink};
pub use fault::ExecutionFault;
pub use frame::Frame;
pub(crate) use frame::FrameLocals;
pub(crate) use frame::FrameValues;
pub(crate) use frame::{BindingIdentity, SharedBinding};
pub(crate) use frame::{FrameComparison, FrameComparisonStep};
pub(crate) use generics::{CheckedCallBindings, GenericBindings};
pub(crate) use receiver::ReceiverAccessSet;
pub use receiver::{ReceiverLease, ReceiverOutcome};
pub use signal::{
    CallPurity, CommandDecode, ConsoleDecode, ContinuationInput, ControlSignal, HostBoundaryDecode,
    HostBoundaryRequest, MemoryDecode, ModelDecode, PendingBlock, PendingCall, PendingCheckpoint,
    PendingCommand, PendingConsole, PendingContinuation, PendingExpr, PendingHostBoundary,
    PendingMemory, PendingModel, PendingPerform, PendingProgramExpr, PendingSession,
    PendingTaskControl, SessionDecode, SourceToolBinding,
};
