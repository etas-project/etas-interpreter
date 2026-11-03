//! Live residual monitor state owned by the evaluation machine.
//!
//! A scope instance is created for one invocation of a callable with an
//! installed plan. It is owned by that invocation's explicit machine frame,
//! or by the continuation that captured the frame while an effect request is
//! handled outside it. Attempt tags travel with a pending action and are the
//! single-dispatch permits of its committed transition.

mod approval;
mod lifecycle;
mod permit;
mod state;

pub(crate) use approval::is_standard_approval;
pub(crate) use lifecycle::{ResidualExitKind, ResidualExitRecord, ResidualRunLog};
pub(crate) use permit::{ResidualAttemptTag, ResidualObservedScope};
pub(crate) use state::{
    ResidualOutcomeStatus, ResidualPreview, ResidualScopeActivation, ResidualScopeInstance,
    ResidualStateLimits, SharedResidualScope,
};
