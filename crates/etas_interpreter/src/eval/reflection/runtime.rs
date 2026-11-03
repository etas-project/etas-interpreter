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
    pub(super) fn reflection_lexical_application(
        &self,
        expr: HirExprId,
        frame: &Frame,
    ) -> Result<Option<etas_frontend::CheckedCallableApplication>, RuntimeReflectionError> {
        let owner = self
            .checked
            .hir
            .index()
            .expr_item
            .get(&expr)
            .copied()
            .ok_or("reflection has no checked lexical owner")?;
        let signature = match self.checked.types.item_signatures.get(&owner) {
            Some(
                etas_types::ItemSignature::Flow(signature)
                | etas_types::ItemSignature::Tool(signature),
            ) => Some(signature),
            Some(etas_types::ItemSignature::Agent(signature)) => signature.callable(),
            _ => None,
        };
        let Some(signature) = signature.filter(|signature| !signature.generic_params.is_empty())
        else {
            return Ok(None);
        };
        let types = frame
            .sorted_type_bindings()
            .into_iter()
            .filter(|(name, _)| {
                signature.generic_params.iter().any(|param| {
                    param.name == *name && param.kind == etas_types::CallableGenericParamKind::Type
                })
            })
            .collect::<Vec<_>>();
        let rows = frame
            .generic_bindings()
            .sorted_rows()
            .into_iter()
            .filter(|(name, _)| {
                signature.generic_params.iter().any(|param| {
                    param.name == *name
                        && param.kind == etas_types::CallableGenericParamKind::Effect
                })
            })
            .collect::<Vec<_>>();
        self.checked
            .checked_callable_application(owner, &types, &rows)
            .map(Some)
            .map_err(|error| RuntimeReflectionError::Inconsistent(error.to_string()))
    }

    pub(super) fn reflect_external_application(
        &self,
        expr: HirExprId,
        ty: etas_types::TypeId,
        lexical: Option<&etas_frontend::CheckedCallableApplication>,
    ) -> Result<InterpValue, RuntimeReflectionError> {
        let prepared = self
            .code_definitions
            .as_ref()
            .ok_or("external reflection has no prepared definitions")?;
        let candidate = prepared.definitions();
        let (backend, definition) = match self.revision {
            Some(revision) => revision
                .frozen_bindings
                .reflect_external_callable_application(
                    candidate,
                    expr,
                    lexical,
                    Default::default(),
                )?,
            None => (
                candidate.clone(),
                candidate.reflect_external_callable_application(
                    expr,
                    lexical,
                    Default::default(),
                )?,
            ),
        };
        let view = backend.resolve(&definition)?;
        let header = view
            .callable_header()
            .ok_or("external application has no checked callable header")?
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

    pub(super) fn reflect_callable_binding(
        &self,
        expr: HirExprId,
        ty: etas_types::TypeId,
        signature: etas_types::TypeId,
        value: InterpValue,
        lexical: Option<&etas_frontend::CheckedCallableApplication>,
    ) -> Result<InterpValue, RuntimeReflectionError> {
        if let InterpValue::Callable(CallTarget::Scoped(value)) = &value {
            return self.reflect_scoped_flow(expr, ty, signature, value, lexical);
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
            let Some(etas_types::Type::Function(expected)) = self.checked.type_store.get(signature)
            else {
                return Err("generic reflection interface is not a checked function type".into());
            };
            let application = crate::value::ScopedCallableValue::checked_flow_application(
                self.checked,
                target,
                expected,
            )?;
            if let Some(revision) = self.revision
                && application.owner() == revision.entry().item
            {
                let application = backend.checked_callable_reflection_application(
                    expr,
                    &application,
                    lexical,
                    Default::default(),
                )?;
                return self.reflect_compiled_flow_entry(expr, ty, revision, Some(&application));
            }
            return self.reflect_native_application_in_context(expr, ty, &application, lexical);
        }
        let InterpValue::Callable(CallTarget::FlowItem(item)) = value else {
            return Err(
                "reflected binding is missing its checked ordinary-flow implementation".into(),
            );
        };
        if let Some(revision) = self.revision {
            if item == revision.entry().item {
                if !std::sync::Arc::ptr_eq(self.checked, revision.type_bridge().candidate_program())
                {
                    return Err("flow revision reflection has a different checked program".into());
                }
                if lexical.is_some() {
                    let Some(etas_types::Type::Function(expected)) =
                        self.checked.type_store.get(signature)
                    else {
                        return Err("reflection interface is not a checked function type".into());
                    };
                    let application = crate::value::ScopedCallableValue::checked_flow_application(
                        self.checked,
                        &CallTarget::FlowItem(item),
                        expected,
                    )?;
                    let application = backend.checked_callable_reflection_application(
                        expr,
                        &application,
                        lexical,
                        Default::default(),
                    )?;
                    return self.reflect_compiled_flow_entry(
                        expr,
                        ty,
                        revision,
                        Some(&application),
                    );
                }
                return self.reflect_compiled_flow_entry(expr, ty, revision, None);
            }
        }
        if lexical.is_some() {
            let Some(etas_types::Type::Function(expected)) = self.checked.type_store.get(signature)
            else {
                return Err("reflection interface is not a checked function type".into());
            };
            let application = crate::value::ScopedCallableValue::checked_flow_application(
                self.checked,
                &CallTarget::FlowItem(item),
                expected,
            )?;
            return self.reflect_native_application_in_context(expr, ty, &application, lexical);
        }
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

    pub(super) fn validate_revision_application(
        &self,
        revision: &crate::api::code::PreparedCodeRevision,
        application: &etas_frontend::CheckedCallableApplication,
    ) -> Result<(), RuntimeReflectionError> {
        let environment = revision.frozen_environment();
        let unparameterized;
        let frozen = match environment.callable_application() {
            Some(application) => application,
            None => {
                unparameterized = environment
                    .checked()
                    .checked_callable_application(environment.declaration(), &[], &[])
                    .map_err(|error| RuntimeReflectionError::Inconsistent(error.to_string()))?;
                &unparameterized
            }
        };
        let bridge = revision.type_bridge();
        if !std::sync::Arc::ptr_eq(application.program(), bridge.candidate_program())
            || application.arguments().len() != frozen.arguments().len()
        {
            return Err("compiled flow application differs from its frozen environment".into());
        }
        for (original, actual) in frozen.arguments().iter().zip(application.arguments()) {
            let matches = match (original, actual) {
                (
                    etas_types::CallableGenericArg::Type(original),
                    etas_types::CallableGenericArg::Type(actual),
                ) => bridge
                    .runtime_to_candidate(frozen.program(), *original)
                    .is_ok_and(|ty| ty == *actual),
                (
                    etas_types::CallableGenericArg::EffectRow(original),
                    etas_types::CallableGenericArg::EffectRow(actual),
                ) => bridge
                    .runtime_row_to_candidate(frozen.program(), original.clone())
                    .is_ok_and(|mut row| {
                        let mut actual = actual.clone();
                        row.effects.sort();
                        row.effects.dedup();
                        actual.effects.sort();
                        actual.effects.dedup();
                        row == actual
                    }),
                _ => false,
            };
            if !matches {
                return Err("compiled flow arguments differ from its frozen environment".into());
            }
        }
        let source = frozen.signature();
        let target = application.signature();
        if source.params.len() != target.params.len()
            || source
                .params
                .iter()
                .zip(&target.params)
                .any(|(source, target)| {
                    !bridge
                        .runtime_to_candidate(frozen.program(), *source)
                        .is_ok_and(|ty| ty == *target)
                })
            || !bridge
                .runtime_to_candidate(frozen.program(), source.output)
                .is_ok_and(|ty| ty == target.output)
        {
            return Err("compiled flow interface differs from its frozen environment".into());
        }
        let mut source_row = source
            .effects
            .clone()
            .map(|row| {
                bridge
                    .runtime_row_to_candidate(frozen.program(), row)
                    .map_err(|error| RuntimeReflectionError::Inconsistent(error.to_string()))
            })
            .transpose()?;
        let mut target_row = target.effects.clone();
        for row in [&mut source_row, &mut target_row].into_iter().flatten() {
            row.effects.sort();
            row.effects.dedup();
        }
        if source_row != target_row {
            return Err(
                "compiled flow effect interface differs from its frozen environment".into(),
            );
        }
        Ok(())
    }

    pub(super) fn reflect_native_application_in_context(
        &self,
        expr: HirExprId,
        ty: etas_types::TypeId,
        application: &etas_frontend::CheckedCallableApplication,
        lexical: Option<&etas_frontend::CheckedCallableApplication>,
    ) -> Result<InterpValue, RuntimeReflectionError> {
        let backend = self
            .code_definitions
            .as_ref()
            .ok_or("bound flow reflection has no prepared definition catalog")?
            .definitions();
        let (backend, definition) = if let Some(revision) = self.revision {
            let bindings = &revision.frozen_bindings;
            let (backend, definition) = bindings.reflect_callable_application(
                backend,
                expr,
                application,
                lexical,
                Default::default(),
            )?;
            (backend, definition)
        } else {
            let definition = backend.reflect_callable_application_in_context(
                expr,
                application,
                lexical,
                Default::default(),
            )?;
            (backend.clone(), definition)
        };
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
