use std::{fmt, sync::Arc};

use etas_code::{
    definition::DefinitionRef, descriptor::CallableHeaderInfo, syntax::SyntaxBlockRef,
};
use etas_frontend::code::definition::NativeDefinitionStore;

#[derive(Clone)]
pub struct AgentDefinitionValue(Arc<AgentDefinitionData>);

struct AgentDefinitionData {
    ty: etas_types::TypeId,
    program: etas_frontend::CheckedProgram,
    definition: DefinitionRef,
    backend: Arc<NativeDefinitionStore>,
}

impl AgentDefinitionValue {
    pub(crate) fn new(
        ty: etas_types::TypeId,
        definition: DefinitionRef,
        backend: Arc<NativeDefinitionStore>,
    ) -> Self {
        let program = backend.catalog().checked_program().clone();
        Self::with_abi(ty, definition, backend, program)
    }
    pub(crate) fn with_abi(
        ty: etas_types::TypeId,
        definition: DefinitionRef,
        backend: Arc<NativeDefinitionStore>,
        program: etas_frontend::CheckedProgram,
    ) -> Self {
        Self(Arc::new(AgentDefinitionData {
            ty,
            program,
            definition,
            backend,
        }))
    }
    pub(crate) fn ty(&self) -> etas_types::TypeId {
        self.0.ty
    }
    pub(crate) fn definition(&self) -> &DefinitionRef {
        &self.0.definition
    }
    pub(crate) fn backend(&self) -> &Arc<NativeDefinitionStore> {
        &self.0.backend
    }
    pub(crate) fn program(&self) -> &etas_frontend::CheckedProgram {
        &self.0.program
    }
    pub(crate) fn body(
        &self,
    ) -> Result<Option<SyntaxBlockRef>, etas_code::definition::DefinitionOperationError> {
        self.0
            .backend
            .resolve(&self.0.definition)
            .map(|view| view.body().cloned())
    }
}

impl fmt::Debug for AgentDefinitionValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AgentDefinitionValue")
            .field("identity", self.0.definition.identity())
            .field("revision", self.0.definition.revision())
            .finish_non_exhaustive()
    }
}
impl PartialEq for AgentDefinitionValue {
    fn eq(&self, other: &Self) -> bool {
        self.0.ty == other.0.ty
            && Arc::ptr_eq(&self.0.program, &other.0.program)
            && self.0.definition.is_same_object(&other.0.definition)
            && Arc::ptr_eq(&self.0.backend, &other.0.backend)
    }
}
impl Eq for AgentDefinitionValue {}

#[derive(Clone)]
pub struct CallableDefinitionValue(Arc<CallableDefinitionData>);
#[derive(Clone)]
struct CallableDefinitionData {
    ty: etas_types::TypeId,
    program: etas_frontend::CheckedProgram,
    header: CallableHeaderInfo,
    signature_graph: etas_code::descriptor::DescriptorGraph,
    analysis: Option<(
        etas_code::descriptor::DescriptorGraph,
        etas_code::descriptor::CallableAnalysisInfo,
    )>,
    body: Option<SyntaxBlockRef>,
    syntax: Arc<etas_frontend::code::service::NativeCodeSyntaxOperations>,
    backend: Arc<NativeDefinitionStore>,
    definition: Option<DefinitionRef>,
    method_snapshot: Option<etas_frontend::code::definition::NativeMethodSnapshot>,
}
impl CallableDefinitionValue {
    pub(crate) fn new(
        ty: etas_types::TypeId,
        header: CallableHeaderInfo,
        body: Option<SyntaxBlockRef>,
        backend: Arc<NativeDefinitionStore>,
    ) -> Result<Self, String> {
        let program = backend.catalog().checked_program().clone();
        let syntax = backend.syntax_operations().clone();
        let graph = backend
            .catalog()
            .callable_descriptors(&header.id)
            .ok_or("reflected callable has no owning descriptor graph")?
            .clone();
        Ok(Self::with_abi(
            ty, header, body, backend, program, syntax, graph,
        ))
    }
    pub(crate) fn with_abi(
        ty: etas_types::TypeId,
        header: CallableHeaderInfo,
        body: Option<SyntaxBlockRef>,
        backend: Arc<NativeDefinitionStore>,
        program: etas_frontend::CheckedProgram,
        syntax: Arc<etas_frontend::code::service::NativeCodeSyntaxOperations>,
        graph: etas_code::descriptor::DescriptorGraph,
    ) -> Self {
        Self(Arc::new(CallableDefinitionData {
            ty,
            program,
            signature_graph: graph.clone(),
            analysis: Some((graph, header.analysis.clone())),
            header,
            body,
            syntax,
            backend,
            definition: None,
            method_snapshot: None,
        }))
    }
    pub(crate) fn ty(&self) -> etas_types::TypeId {
        self.0.ty
    }
    pub(crate) fn backend(&self) -> &Arc<NativeDefinitionStore> {
        &self.0.backend
    }
    pub(crate) fn definition(&self) -> Result<&DefinitionRef, String> {
        self.0
            .definition
            .as_ref()
            .ok_or_else(|| "callable descriptor has no immutable definition snapshot".into())
    }
    pub(crate) fn with_definition(&self, definition: DefinitionRef) -> Result<Self, String> {
        if self.0.method_snapshot.is_some()
            || Some(definition.identity()) != self.0.header.id.as_declaration()
            || definition.identity().kind() != etas_code::DefinitionKind::Flow
        {
            return Err("callable snapshot does not identify this ordinary flow".into());
        }
        let view = self
            .0
            .backend
            .resolve(&definition)
            .map_err(|error| error.to_string())?;
        let analysis = match view.current_analysis() {
            Ok(analysis) => Some((
                view.current_analysis_descriptors()
                    .map_err(|error| {
                        format!("callable snapshot analysis is unavailable: {error:?}")
                    })?
                    .clone(),
                analysis.clone(),
            )),
            Err(etas_code::definition::AnalysisUnavailableReason::InputMismatch) => {
                return Err("callable snapshot analysis identity is inconsistent".into());
            }
            Err(_) => None,
        };
        Ok(Self(Arc::new(CallableDefinitionData {
            definition: Some(definition),
            body: view.body().cloned(),
            signature_graph: view.signature_descriptors().clone(),
            analysis,
            ..self.0.as_ref().clone()
        })))
    }
    pub(crate) fn with_method_snapshot(
        &self,
        snapshot: etas_frontend::code::definition::NativeMethodSnapshot,
    ) -> Result<Self, String> {
        let canonical = &snapshot.coordinates.revision.definition;
        let matches = self.id() == canonical
            || self
                .id()
                .as_method()
                .and_then(|selected| selected.implementation().selected_spec())
                .and_then(|spec| canonical.as_method()?.select_spec(spec.clone()).ok())
                .is_some_and(|method| &etas_code::CallableId::method(method) == self.id());
        if self.0.definition.is_some() || self.id().as_method().is_none() || !matches {
            return Err(
                "read-only method coordinates do not match the checked callable identity".into(),
            );
        }
        let expected = self
            .backend()
            .resolve(&snapshot.owner)
            .map_err(|error| error.to_string())?
            .method_snapshot(canonical)
            .map_err(|error| error.to_string())?;
        if expected.coordinates != snapshot.coordinates {
            return Err("method coordinates disagree with their owning immutable revision".into());
        }
        Ok(Self(Arc::new(CallableDefinitionData {
            method_snapshot: Some(snapshot),
            ..self.0.as_ref().clone()
        })))
    }
    pub(crate) fn revision(&self) -> Result<etas_code::RevisionInfo, String> {
        match &self.0.method_snapshot {
            Some(snapshot) => Ok(snapshot.coordinates.revision_info()),
            None => self
                .backend()
                .resolve(self.definition()?)
                .map(|view| view.revision().clone())
                .map_err(|error| error.to_string()),
        }
    }
    pub(crate) fn dependencies(&self) -> Result<etas_code::report::DependencyInfo, String> {
        match &self.0.method_snapshot {
            Some(snapshot) => Ok(snapshot.coordinates.dependencies.clone()),
            None => self
                .backend()
                .resolve(self.definition()?)
                .map_err(|error| error.to_string())?
                .dependency_info()
                .map_err(|error| error.to_string()),
        }
    }
    pub(crate) fn program(&self) -> &etas_frontend::CheckedProgram {
        &self.0.program
    }
    pub(crate) fn body(&self) -> Option<&SyntaxBlockRef> {
        self.0.body.as_ref()
    }
    pub(crate) fn syntax_operations(
        &self,
    ) -> &Arc<etas_frontend::code::service::NativeCodeSyntaxOperations> {
        &self.0.syntax
    }
    pub(crate) fn id(&self) -> &etas_code::CallableId {
        &self.0.header.id
    }
    pub(crate) fn header(&self) -> &CallableHeaderInfo {
        &self.0.header
    }
    pub(crate) fn descriptors(&self) -> &etas_code::descriptor::DescriptorGraph {
        &self.0.signature_graph
    }
    pub(crate) fn analysis(
        &self,
    ) -> Option<(
        &etas_code::descriptor::DescriptorGraph,
        &etas_code::descriptor::CallableAnalysisInfo,
    )> {
        self.0
            .analysis
            .as_ref()
            .map(|(graph, analysis)| (graph, analysis))
    }
    pub(crate) fn from_agent_method(
        ty: etas_types::TypeId,
        method: etas_frontend::code::definition::NativeAgentMethodView,
        backend: Arc<NativeDefinitionStore>,
        program: etas_frontend::CheckedProgram,
    ) -> Self {
        Self(Arc::new(CallableDefinitionData {
            ty,
            program,
            header: method.header,
            signature_graph: method.signature_descriptors,
            analysis: method.analysis,
            body: method.body,
            syntax: backend.syntax_operations().clone(),
            backend,
            definition: None,
            method_snapshot: Some(method.snapshot),
        }))
    }
    pub(crate) fn in_revision(
        &self,
        ty: etas_types::TypeId,
        program: etas_frontend::CheckedProgram,
    ) -> Self {
        Self(Arc::new(CallableDefinitionData {
            ty,
            program,
            header: self.0.header.clone(),
            body: self.0.body.clone(),
            backend: self.0.backend.clone(),
            syntax: self.0.syntax.clone(),
            signature_graph: self.0.signature_graph.clone(),
            analysis: self.0.analysis.clone(),
            definition: self.0.definition.clone(),
            method_snapshot: self.0.method_snapshot.clone(),
        }))
    }
}
impl fmt::Debug for CallableDefinitionValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CallableDefinitionValue")
            .field("id", &self.0.header.id)
            .finish_non_exhaustive()
    }
}
impl PartialEq for CallableDefinitionValue {
    fn eq(&self, other: &Self) -> bool {
        self.0.ty == other.0.ty
            && Arc::ptr_eq(&self.0.program, &other.0.program)
            && Arc::ptr_eq(&self.0.backend, &other.0.backend)
            && self.0.header == other.0.header
            && self.0.signature_graph == other.0.signature_graph
            && self.0.analysis == other.0.analysis
            && match (&self.0.method_snapshot, &other.0.method_snapshot) {
                (Some(a), Some(b)) => {
                    a.coordinates == b.coordinates && a.owner.is_same_object(&b.owner)
                }
                (None, None) => true,
                _ => false,
            }
            && match (&self.0.definition, &other.0.definition) {
                (Some(lhs), Some(rhs)) => lhs.is_same_object(rhs),
                (None, None) => true,
                _ => false,
            }
            && self.0.body.as_ref().map(|body| body.identity())
                == other.0.body.as_ref().map(|body| body.identity())
    }
}
impl Eq for CallableDefinitionValue {}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn code_definition_values_are_thin_shared_handles() {
        assert_eq!(size_of::<AgentDefinitionValue>(), size_of::<Arc<()>>());
        assert_eq!(size_of::<CallableDefinitionValue>(), size_of::<Arc<()>>());
    }
}
