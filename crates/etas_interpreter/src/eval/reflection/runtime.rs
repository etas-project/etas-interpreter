use super::*;
use crate::control::CallTarget;

pub(super) enum RuntimeReflectionError {
    Definition(etas_frontend::code::definition::NativeDefinitionError),
    Inconsistent(String),
}

impl From<etas_frontend::code::descriptor::ProjectionError> for RuntimeReflectionError {
    fn from(error: etas_frontend::code::descriptor::ProjectionError) -> Self {
        Self::Definition(etas_frontend::code::definition::NativeDefinitionError::Projection(error))
    }
}
impl From<etas_frontend::code::definition::NativeDefinitionError> for RuntimeReflectionError {
    fn from(error: etas_frontend::code::definition::NativeDefinitionError) -> Self {
        Self::Definition(error)
    }
}
impl From<etas_code::definition::DefinitionOperationError> for RuntimeReflectionError {
    fn from(error: etas_code::definition::DefinitionOperationError) -> Self {
        Self::Definition(error.into())
    }
}
impl From<&str> for RuntimeReflectionError {
    fn from(reason: &str) -> Self {
        Self::Inconsistent(reason.into())
    }
}
impl From<String> for RuntimeReflectionError {
    fn from(reason: String) -> Self {
        Self::Inconsistent(reason)
    }
}
impl RuntimeReflectionError {
    pub(super) fn into_signal(self, span: Span) -> ControlSignal {
        match self {
            Self::Definition(error) => code::definition_operation_failure(error, span),
            Self::Inconsistent(reason) => ControlSignal::missing_checked_fact(reason, span),
        }
    }
}

impl EvalContext<'_> {
    pub(super) fn reflect_agent_binding(
        &self,
        expr: HirExprId,
        ty: etas_types::TypeId,
        value: InterpValue,
    ) -> Result<InterpValue, RuntimeReflectionError> {
        let backend = self
            .code_definitions
            .as_ref()
            .ok_or("agent value reflection has no prepared definitions")?
            .definitions();
        match value {
            InterpValue::Callable(CallTarget::AgentItem(item)) => {
                if self.revision.is_some() {
                    return self.reflect_frozen_source(expr, ty, Some(item));
                }
                let view = backend.catalog().agent_value_reflection(expr, item)?;
                match view {
                    StaticReflectionView::Agent(_) => {
                        let definition = backend.reflect_agent_value(expr, item)?;
                        Ok(InterpValue::AgentDefinition(AgentDefinitionValue::new(
                            ty,
                            definition,
                            backend.clone(),
                        )))
                    }
                    StaticReflectionView::Callable(header) => {
                        let body = backend.catalog().retained_agent_value_body(
                            expr,
                            item,
                            backend.syntax_operations(),
                        )?;
                        Ok(InterpValue::CallableDefinition(
                            CallableDefinitionValue::new(
                                ty,
                                header.clone(),
                                body,
                                backend.clone(),
                            )?,
                        ))
                    }
                }
            }
            InterpValue::Callable(CallTarget::AgentRevision(value)) => {
                if let Some(agent) = value.frozen_agent() {
                    if !std::sync::Arc::ptr_eq(self.checked, value.source_program())
                        || !value.is_default_entry()
                    {
                        return Err(
                            "frozen agent reflection belongs to a different checked binding".into(),
                        );
                    }
                    return self.reflect_frozen_source(expr, ty, Some(agent));
                }
                let prepared = value.prepared();
                if !std::sync::Arc::ptr_eq(self.checked, value.source_program())
                    || !value.is_default_entry()
                {
                    return Err(
                        "reflected agent revision belongs to a different program or callable"
                            .into(),
                    );
                }
                let source_agent = value.source_agent()?;
                let view = backend
                    .catalog()
                    .agent_value_reflection(expr, source_agent)?;
                self.reflect_revision_view(expr, ty, prepared, Some(source_agent), view)
            }
            _ => Err("reflected binding is not a checked agent value".into()),
        }
    }

    pub(super) fn reflect_frozen_source(
        &self,
        expr: HirExprId,
        ty: etas_types::TypeId,
        agent: Option<HirItemId>,
    ) -> Result<InterpValue, RuntimeReflectionError> {
        let revision = self
            .revision
            .ok_or("source reflection requires its owning revision")?;
        let bindings = &revision.frozen_bindings;
        let backend = bindings.source();
        match bindings.reflect(self.checked, expr, agent)? {
            etas_frontend::code::definition::NativeSourceReflection::Agent(definition) => Ok(
                InterpValue::AgentDefinition(AgentDefinitionValue::with_abi(
                    ty,
                    definition,
                    backend.clone(),
                    self.checked.clone(),
                )),
            ),
            etas_frontend::code::definition::NativeSourceReflection::Callable(callable) => Ok(
                InterpValue::CallableDefinition(CallableDefinitionValue::with_abi(
                    ty,
                    callable.header,
                    callable.body,
                    backend.clone(),
                    self.checked.clone(),
                    backend.syntax_operations().clone(),
                    callable.graph,
                )),
            ),
        }
    }
}
