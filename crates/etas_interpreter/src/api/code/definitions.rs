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

    /// Reifies a flow definition using a reflection site in the checked project.
    pub fn callable_definition_value_at(
        &self,
        expression: etas_hir::HirExprId,
        definition: etas_code::definition::DefinitionRef,
    ) -> Result<crate::api::InterpValue, etas_frontend::code::definition::NativeDefinitionError>
    {
        use etas_frontend::code::{
            definition::{NativeDefinitionError, StaticReflectionView},
            descriptor::ProjectionError,
        };
        let catalog = self.definitions.catalog();
        let view = self.definitions.resolve(&definition)?;
        let fact = catalog
            .checked_program()
            .checked_reflections()
            .get(&expression)
            .ok_or_else(|| {
                NativeDefinitionError::Projection(ProjectionError::MissingIdentity(
                    "checked callable reflection site".into(),
                ))
            })?;
        let StaticReflectionView::Callable(header) = catalog
            .static_reflection(expression)
            .map_err(NativeDefinitionError::Projection)?
        else {
            return Err(NativeDefinitionError::Projection(ProjectionError::Invalid(
                "reflection site does not identify a callable".into(),
            )));
        };
        if header.id.as_declaration() != Some(view.identity())
            || view.identity().kind() != etas_code::DefinitionKind::Flow
        {
            return Err(NativeDefinitionError::Projection(ProjectionError::Invalid(
                "reflection site and flow definition have different identities".into(),
            )));
        }
        let graph = catalog.callable_descriptors(&header.id).ok_or_else(|| {
            NativeDefinitionError::Projection(ProjectionError::MissingIdentity(
                "checked callable descriptor graph".into(),
            ))
        })?;
        let snapshot_type = view.signature_type().ok_or_else(|| {
            NativeDefinitionError::Projection(ProjectionError::MissingIdentity(
                "checked flow snapshot signature".into(),
            ))
        })?;
        let fingerprint = |graph: &etas_code::descriptor::DescriptorGraph, ty| {
            graph.type_fingerprint(ty).map_err(|error| {
                NativeDefinitionError::Projection(ProjectionError::Invalid(error.to_string()))
            })
        };
        if fingerprint(graph, header.signature)?
            != fingerprint(view.signature_descriptors(), snapshot_type)?
        {
            return Err(NativeDefinitionError::Projection(ProjectionError::Invalid(
                "reflection site and flow definition have different checked signatures".into(),
            )));
        }
        let value = crate::value::CallableDefinitionValue::new(
            fact.descriptor_type,
            header.clone(),
            view.body().cloned(),
            self.definitions.clone(),
        )
        .and_then(|value| value.with_definition(definition))
        .map_err(|error| NativeDefinitionError::Projection(ProjectionError::Invalid(error)))?;
        Ok(crate::api::InterpValue::CallableDefinition(value))
    }
}

impl std::fmt::Debug for PreparedCodeDefinitions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreparedCodeDefinitions")
            .finish_non_exhaustive()
    }
}
