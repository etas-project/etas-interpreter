mod targets;
mod type_bridge;
mod value_bridge;
pub(crate) use targets::{RevisionCallable, RevisionTargets};
pub(crate) use value_bridge::CallableValueProjection;
pub(crate) use value_bridge::projection::CheckedValueTypes;
pub(crate) use value_bridge::{ActionProjection, RevisionProjection};

pub use type_bridge::{RevisionTypeBridge, RevisionTypeBridgeError};
pub use value_bridge::{RevisionValueBridgeError, RevisionValueFailure};
