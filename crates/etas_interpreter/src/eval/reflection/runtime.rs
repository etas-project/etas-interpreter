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
    pub(super) fn reflect_callable_binding(
        &self,
        expr: HirExprId,
        ty: etas_types::TypeId,
        value: InterpValue,
    ) -> Result<InterpValue, RuntimeReflectionError> {
        if let InterpValue::Callable(CallTarget::Scoped(value)) = &value {
            return self.reflect_scoped_flow(expr, ty, value);
        }
        let backend = self
            .code_definitions
            .as_ref()
            .ok_or("flow value reflection has no prepared definitions")?
            .definitions();
        if !std::sync::Arc::ptr_eq(self.checked, backend.catalog().checked_program()) {
            return Err(
                "flow binding reflection requires the matching checked implementation program"
                    .into(),
            );
        }
        if let InterpValue::Callable(target @ CallTarget::Specialized { .. }) = &value {
            let Some(etas_types::CheckedReflectionFact {
                target: etas_types::CheckedReflectionTarget::CallableValue { signature, .. },
                ..
            }) = self.checked.types.reflections.get(&expr)
            else {
                return Err("generic reflection has no checked callable interface".into());
            };
            let Some(etas_types::Type::Function(expected)) =
                self.checked.type_store.get(*signature)
            else {
                return Err("generic reflection interface is not a checked function type".into());
            };
            let application = crate::value::ScopedCallableValue::checked_flow_application(
                self.checked,
                target,
                expected,
            )?;
            return self.reflect_native_application(expr, ty, &application);
        }
        let InterpValue::Callable(CallTarget::FlowItem(item)) = value else {
            return Err(
                "reflected binding is missing its checked ordinary-flow implementation".into(),
            );
        };
        if let Some(revision) = self.revision {
            let original = revision
                .frozen_bindings
                .frozen_flow_item(backend.catalog(), item)?;
            return self.reflect_frozen_flow_binding(expr, ty, &revision.frozen_bindings, original);
        }
        let header = backend.catalog().callable_value_reflection(expr, item)?;
        let body = backend.catalog().retained_callable_value_body(
            expr,
            item,
            backend.syntax_operations(),
        )?;
        Ok(InterpValue::CallableDefinition(
            CallableDefinitionValue::new(ty, header.clone(), body, backend.clone())?
                .with_definition(backend.reflect_flow_value(expr, item)?)?,
        ))
    }

    pub(super) fn reflect_native_application(
        &self,
        expr: HirExprId,
        ty: etas_types::TypeId,
        application: &etas_frontend::CheckedCallableApplication,
    ) -> Result<InterpValue, RuntimeReflectionError> {
        let backend = self
            .code_definitions
            .as_ref()
            .ok_or("bound flow reflection has no prepared definition catalog")?
            .definitions();
        let definition =
            backend.reflect_callable_application(expr, application, Default::default())?;
        let view = backend.resolve(&definition)?;
        let contract = view.environment()?.contract();
        let header = contract
            .entry()
            .and_then(|id| contract.callable(id))
            .ok_or("bound flow reflection has no frozen callable header")?
            .clone();
        Ok(InterpValue::CallableDefinition(
            CallableDefinitionValue::with_abi(
                ty,
                header,
                view.body().cloned(),
                backend.clone(),
                self.checked.clone(),
                backend.syntax_operations().clone(),
                view.signature_descriptors().clone(),
            )
            .with_definition(definition)?,
        ))
    }

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
                            )?
                            .with_method_snapshot(
                                backend.reflect_agent_value_method_snapshot(expr, item)?,
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
            etas_frontend::code::definition::NativeSourceReflection::Callable(callable) => {
                let value = CallableDefinitionValue::with_abi(
                    ty,
                    callable.header,
                    callable.body,
                    backend.clone(),
                    self.checked.clone(),
                    backend.syntax_operations().clone(),
                    callable.graph,
                );
                let value = match callable.definition {
                    Some(definition) => value.with_definition(definition)?,
                    None => value,
                };
                let value = match callable.method_snapshot {
                    Some(snapshot) => value.with_method_snapshot(snapshot)?,
                    None => value,
                };
                Ok(InterpValue::CallableDefinition(value))
            }
        }
    }

    pub(super) fn reflect_frozen_flow_binding(
        &self,
        expr: HirExprId,
        ty: etas_types::TypeId,
        bindings: &etas_frontend::code::definition::FrozenReflectionBindings,
        original: HirItemId,
    ) -> Result<InterpValue, RuntimeReflectionError> {
        let caller = self
            .code_definitions
            .as_ref()
            .ok_or("flow binding reflection has no checked caller catalog")?
            .definitions();
        let callable = bindings.reflect_flow_binding(caller.catalog(), expr, original)?;
        let backend = bindings.source();
        let definition = callable
            .definition
            .ok_or("frozen flow binding reflection has no native definition lease")?;
        Ok(InterpValue::CallableDefinition(
            CallableDefinitionValue::with_abi(
                ty,
                callable.header,
                callable.body,
                backend.clone(),
                self.checked.clone(),
                backend.syntax_operations().clone(),
                callable.graph,
            )
            .with_definition(definition)?,
        ))
    }
}
