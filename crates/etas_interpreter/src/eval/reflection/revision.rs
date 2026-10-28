use super::{runtime::RuntimeReflectionError, *};

impl EvalContext<'_> {
    pub(super) fn reflect_revision_view(
        &self,
        expr: HirExprId,
        ty: etas_types::TypeId,
        prepared: &std::sync::Arc<crate::api::code::PreparedAgentRevision>,
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
                let candidate = current
                    .catalog()
                    .callable(&header.id)
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
                let body = if header.id == prepared.targets.default && source_body.is_some() {
                    reflected
                        .backend
                        .resolve(&reflected.definition)?
                        .body()
                        .cloned()
                } else {
                    source_body
                };
                Ok(InterpValue::CallableDefinition(
                    CallableDefinitionValue::with_abi(
                        ty,
                        candidate.clone(),
                        body,
                        current.clone(),
                        self.checked.clone(),
                        backend.syntax_operations().clone(),
                        current
                            .catalog()
                            .callable_descriptors(&candidate.id)
                            .ok_or("pinned callable has no descriptor graph")?
                            .clone(),
                    ),
                ))
            }
        }
    }
}
