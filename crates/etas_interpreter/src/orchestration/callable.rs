use super::CheckpointCodeRevision;
use etas_types::TypeId;

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum CheckpointProgramReference {
    Source,
    Revision {
        revision: Box<CheckpointCodeRevision>,
    },
}

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
#[serde(deny_unknown_fields)]
pub(crate) struct CallableProjectionSnapshot {
    pub revision: CheckpointCodeRevision,
    pub to_candidate: bool,
    pub from: TypeId,
    pub to: TypeId,
}

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
#[serde(deny_unknown_fields)]
pub(crate) struct ScopedCallableMetadata {
    pub native_program: CheckpointProgramReference,
    pub native_type: TypeId,
    pub projections: Vec<CallableProjectionSnapshot>,
}

impl ScopedCallableMetadata {
    pub(crate) fn retained_bytes(&self) -> u64 {
        etas_core::memory::MemoryFootprint::of(self).estimated_bytes()
    }
}
