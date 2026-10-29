use super::{runtime::RuntimeReflectionError, *};

impl EvalContext<'_> {
    pub(super) fn reflect_scoped_flow(
        &self,
        expr: HirExprId,
        ty: etas_types::TypeId,
        value: &crate::value::ScopedCallableValue,
    ) -> Result<InterpValue, RuntimeReflectionError> {
        let Some(etas_types::CheckedReflectionFact {
            target: etas_types::CheckedReflectionTarget::CallableValue { signature, .. },
            ..
        }) = self.checked.types.reflections.get(&expr)
        else {
            return Err("flow revision reflection has no checked callable binding".into());
        };
        if !std::sync::Arc::ptr_eq(self.checked, value.presented_program())
            || value.presented_type() != *signature
        {
            return Err(
                "flow revision reflection differs from its checked caller interface".into(),
            );
        }
        let native = value
            .program()
            .definitions
            .as_ref()
            .ok_or("scoped flow reflection has no native declaration catalog")?
            .definitions();
        if !std::sync::Arc::ptr_eq(&value.program().checked, native.catalog().checked_program()) {
            return Err("scoped flow reflection has a different native checked program".into());
        }
        if value.program().revision.is_none()
            && std::sync::Arc::ptr_eq(self.checked, &value.program().checked)
            && let Some(application) = value.native_application()
            && !application.arguments().is_empty()
        {
            // A captured lambda may also carry its owner's application. Only
            // an ordinary-flow target can supply an editable flow definition.
            let application = crate::value::ScopedCallableValue::checked_flow_application(
                &value.program().checked,
                value.target_ref(),
                value.native_signature(),
            )?;
            return self.reflect_native_application(expr, ty, &application);
        }
        let crate::control::CallTarget::FlowItem(item) = value.target_ref() else {
            return Err("scoped flow reflection requires an ordinary-flow declaration".into());
        };
        let (backend, original) = match value.program().revision.as_ref() {
            Some(prepared) if *item == prepared.entry().item => {
                return self.reflect_compiled_flow_entry(expr, ty, prepared);
            }
            Some(prepared) => (
                prepared.frozen_bindings.source(),
                prepared
                    .frozen_bindings
                    .frozen_flow_item(native.catalog(), *item)?,
            ),
            None => {
                if native
                    .catalog()
                    .checked_program()
                    .definition_sources
                    .requires_revision_context()
                    || native.catalog().ordinary_flow_identity(*item).is_none()
                {
                    return Err("scoped source flow has no original checked declaration".into());
                }
                (native, *item)
            }
        };
        if !std::sync::Arc::ptr_eq(self.checked, backend.catalog().checked_program()) {
            let caller = self
                .revision
                .ok_or("scoped flow reflection has no checked caller-to-source binding")?;
            if !std::sync::Arc::ptr_eq(
                caller.frozen_bindings.source().catalog().checked_program(),
                backend.catalog().checked_program(),
            ) {
                return Err("scoped flow belongs to a different frozen source program".into());
            }
            return self.reflect_frozen_flow_binding(expr, ty, &caller.frozen_bindings, original);
        }
        let header = backend
            .catalog()
            .callable_value_reflection(expr, original)?;
        let body = backend.catalog().retained_callable_value_body(
            expr,
            original,
            backend.syntax_operations(),
        )?;
        Ok(InterpValue::CallableDefinition(
            CallableDefinitionValue::new(ty, header.clone(), body, backend.clone())?
                .with_definition(backend.reflect_flow_value(expr, original)?)?,
        ))
    }

    fn reflect_compiled_flow_entry(
        &self,
        expr: HirExprId,
        ty: etas_types::TypeId,
        prepared: &std::sync::Arc<crate::api::code::PreparedCodeRevision>,
    ) -> Result<InterpValue, RuntimeReflectionError> {
        if prepared.identity().kind() != etas_code::DefinitionKind::Flow {
            return Err(
                "reflected scoped callable is not the pinned ordinary-flow revision entry".into(),
            );
        }
        let backend = self
            .code_definitions
            .as_ref()
            .ok_or("flow revision reflection has no prepared caller catalog")?
            .definitions();
        let source = backend
            .catalog()
            .item_owner(&prepared.targets.default)
            .ok_or("flow revision has no canonical declaration in the caller catalog")?;
        let header = backend.catalog().callable_value_reflection(expr, source)?;
        if header.id != prepared.targets.default {
            return Err("flow revision reflection differs from its canonical target".into());
        }
        let reflected = prepared
            .reflection
            .as_ref()
            .ok_or("compiled flow reflection requires its native definition lease")?;
        let view = reflected.backend.resolve(&reflected.definition)?;
        let environment = view.environment()?;
        if environment.identity() != prepared.identity()
            || environment.fingerprint() != prepared.frozen_environment().fingerprint()
            || environment.contract().fingerprint()
                != prepared.frozen_environment().contract().fingerprint()
        {
            return Err("compiled flow definition lease differs from its frozen contract".into());
        }
        let graph = backend
            .catalog()
            .callable_descriptors(&header.id)
            .ok_or("flow revision reflection has no checked signature graph")?
            .clone();
        let definition = CallableDefinitionValue::with_abi(
            ty,
            header.clone(),
            None,
            reflected.backend.clone(),
            self.checked.clone(),
            reflected.backend.syntax_operations().clone(),
            graph,
        )
        .with_definition(reflected.definition.clone())?;
        Ok(InterpValue::CallableDefinition(definition))
    }

    pub(super) fn reflect_revision_view(
        &self,
        expr: HirExprId,
        ty: etas_types::TypeId,
        prepared: &std::sync::Arc<crate::api::code::PreparedCodeRevision>,
        source_agent: Option<HirItemId>,
        view: StaticReflectionView<'_>,
    ) -> Result<InterpValue, RuntimeReflectionError> {
        let reflected = prepared
            .reflection
            .as_ref()
            .ok_or("compiled agent reflection requires its native definition lease")?;
        match view {
            StaticReflectionView::Agent(header) => {
                if header.identity != *prepared.identity() {
                    return Err("reflected agent disagrees with its pinned nominal identity".into());
                }
                Ok(InterpValue::AgentDefinition(
                    AgentDefinitionValue::with_abi(
                        ty,
                        reflected.definition.clone(),
                        reflected.backend.clone(),
                        self.checked.clone(),
                    ),
                ))
            }
            StaticReflectionView::Callable(header) => {
                let backend = self
                    .code_definitions
                    .as_ref()
                    .ok_or("revision reflection has no prepared current catalog")?
                    .definitions();
                let current = prepared.definitions.definitions();
                let canonical = backend
                    .catalog()
                    .canonical_callable(&header.id)
                    .ok_or("reflected method has no checked canonical source binding")?;
                let candidate = current
                    .catalog()
                    .callable(canonical)
                    .ok_or("reflected method is absent from the pinned candidate catalog")?;
                let source_body = match source_agent {
                    Some(item) => backend.catalog().retained_agent_value_body(
                        expr,
                        item,
                        backend.syntax_operations(),
                    )?,
                    None => backend
                        .catalog()
                        .retained_body(expr, backend.syntax_operations())?,
                };
                let body = if *canonical == prepared.targets.default && source_body.is_some() {
                    reflected
                        .backend
                        .resolve(&reflected.definition)?
                        .body()
                        .cloned()
                } else {
                    source_body
                };
                let owner = reflected.backend.resolve(&reflected.definition)?;
                let snapshot = owner.method_snapshot(canonical)?;
                let mut candidate = candidate.clone();
                candidate.id = header.id.clone();
                Ok(InterpValue::CallableDefinition(
                    CallableDefinitionValue::with_abi(
                        ty,
                        candidate.clone(),
                        body,
                        reflected.backend.clone(),
                        self.checked.clone(),
                        backend.syntax_operations().clone(),
                        current
                            .catalog()
                            .callable_descriptors(canonical)
                            .ok_or("pinned callable has no descriptor graph")?
                            .clone(),
                    )
                    .with_method_snapshot(snapshot)?,
                ))
            }
        }
    }
}
