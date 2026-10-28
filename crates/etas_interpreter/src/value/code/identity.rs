use etas_code::{CallableId, DefinitionKey};
use etas_types::{TypeId, facts::symbols::KnownCodeTypes};

use super::{CodeMetadataPayload, CodeMetadataValue};

#[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(tag = "role", content = "identity", deny_unknown_fields)]
pub(crate) enum CodeIdentitySnapshot {
    Agent(DefinitionKey),
    Definition(DefinitionKey),
    Callable(CallableId),
    Tool(DefinitionKey),
}

impl CodeIdentitySnapshot {
    pub(crate) fn capture(value: &CodeMetadataValue) -> Result<Self, String> {
        Ok(match value.payload() {
            CodeMetadataPayload::Agent(identity) => Self::Agent(identity.clone()),
            CodeMetadataPayload::Definition(identity) => Self::Definition(identity.clone()),
            CodeMetadataPayload::Callable(identity) => Self::Callable(identity.clone()),
            CodeMetadataPayload::Tool(identity) => Self::Tool(identity.clone()),
            _ => {
                return Err(
                    "compiler descriptor persistence requires its validated graph artifact codec"
                        .into(),
                );
            }
        })
    }

    pub(crate) fn restore(
        &self,
        known: &KnownCodeTypes,
        ty: TypeId,
    ) -> Result<CodeMetadataValue, String> {
        let payload = match self {
            Self::Agent(identity) => CodeMetadataPayload::Agent(identity.clone()),
            Self::Definition(identity) => CodeMetadataPayload::Definition(identity.clone()),
            Self::Callable(identity) => CodeMetadataPayload::Callable(identity.clone()),
            Self::Tool(identity) => CodeMetadataPayload::Tool(identity.clone()),
        };
        CodeMetadataValue::new(known, ty, payload)
    }
}

#[cfg(test)]
mod tests;
