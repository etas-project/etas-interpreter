use std::{fmt, sync::Arc};

use etas_code::{
    definition::AgentDefinitionRef, descriptor::CallableHeaderInfo, syntax::SyntaxBlockRef,
};
use etas_frontend::code::definition::NativeDefinitionStore;

#[derive(Clone)]
pub struct AgentDefinitionValue(Arc<AgentDefinitionData>);

struct AgentDefinitionData {
    ty: etas_types::TypeId,
    program: etas_frontend::CheckedProgram,
    definition: AgentDefinitionRef,
    backend: Arc<NativeDefinitionStore>,
}

impl AgentDefinitionValue {
    pub(crate) fn new(
        ty: etas_types::TypeId,
        definition: AgentDefinitionRef,
        backend: Arc<NativeDefinitionStore>,
    ) -> Self {
        let program = backend.catalog().checked_program().clone();
        Self::with_abi(ty, definition, backend, program)
    }
    pub(crate) fn with_abi(
        ty: etas_types::TypeId,
        definition: AgentDefinitionRef,
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
    pub(crate) fn definition(&self) -> &AgentDefinitionRef {
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
        }))
    }
    pub(crate) fn ty(&self) -> etas_types::TypeId {
        self.0.ty
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
