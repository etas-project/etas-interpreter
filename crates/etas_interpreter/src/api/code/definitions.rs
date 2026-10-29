use std::sync::Arc;

use etas_frontend::{CheckedProject, code::definition::NativeDefinitionStore};

/// Explicit native composition: the driver/embedding supplies a catalog built
/// from canonical package origins. The evaluator never synthesizes those IDs.
pub struct PreparedCodeDefinitions {
    definitions: Arc<NativeDefinitionStore>,
}

impl PreparedCodeDefinitions {
    pub fn new(definitions: Arc<NativeDefinitionStore>) -> Self {
        Self { definitions }
    }
    pub fn definitions(&self) -> &Arc<NativeDefinitionStore> {
        &self.definitions
    }
    pub fn is_for(&self, project: &CheckedProject) -> bool {
        Arc::ptr_eq(self.definitions.catalog().checked_program(), project)
    }

    /// Reifies code data under this checked program's AgentDef ABI. It does not
    /// compile the definition, grant Code authority, or accept caller TypeIds.
    pub fn definition_value(
        &self,
        definition: etas_code::definition::DefinitionRef,
    ) -> Result<crate::api::InterpValue, etas_frontend::code::definition::NativeDefinitionError>
    {
        use etas_frontend::code::{definition::NativeDefinitionError, descriptor::ProjectionError};
        let view = self.definitions.resolve(&definition)?;
        let catalog = self.definitions.catalog();
        let ty = catalog
            .agent_component(view.identity())
            .and_then(|component| catalog.checked_program().types.agent_signature(component))
            .and_then(|signature| signature.definition_type)
            .ok_or_else(|| {
                NativeDefinitionError::Projection(ProjectionError::MissingIdentity(
                    "checked AgentDef ABI for definition value".into(),
                ))
            })?;
        Ok(crate::api::InterpValue::AgentDefinition(
            crate::value::AgentDefinitionValue::new(ty, definition, self.definitions.clone()),
        ))
    }
}

impl std::fmt::Debug for PreparedCodeDefinitions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreparedCodeDefinitions")
            .finish_non_exhaustive()
    }
}
