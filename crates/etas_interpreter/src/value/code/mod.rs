mod callable;
mod definition;
mod identity;
mod metadata;
mod program;
mod syntax;
pub use callable::ScopedCallableValue;
pub(crate) use callable::validate_snapshot_bounds;
pub(crate) use callable::validate_snapshot_signature;
pub(crate) use program::CheckedExecutionProgram;

pub use definition::{AgentDefinitionValue, CallableDefinitionValue};
pub(crate) use identity::CodeIdentitySnapshot;
pub(crate) use metadata::CodeMetadataPayload;
pub use metadata::CodeMetadataValue;
pub use syntax::SyntaxBlockValue;
mod agent;
pub use agent::AgentRevisionValue;
