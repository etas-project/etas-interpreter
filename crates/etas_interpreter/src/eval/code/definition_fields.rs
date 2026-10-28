use etas_code::{
    definition::AnalysisUnavailableReason,
    descriptor::{CallableAnalysisInfo, DescriptorGraph, TypeInfo},
};
use etas_types::{TypeId, TypeStore, facts::symbols::KnownCodeTypes};

use super::{
    annotation, behavior,
    project::{Projector, optional},
    report, signature, spec,
};
use crate::value::{
    AgentDefinitionValue, CallableDefinitionValue, CodeMetadataPayload as M, InterpValue as V,
};

pub(in crate::eval) fn agent_field(
    store: &TypeStore,
    known: &KnownCodeTypes,
    ty: TypeId,
    definition: &AgentDefinitionValue,
    field: &str,
) -> Result<V, String> {
    let view = definition
        .backend()
        .resolve(definition.definition())
        .map_err(|error| error.to_string())?;
    let p = Projector { store, known };
    match field {
        "annotations" => annotation::annotations(&p, ty, view.annotations()),
        "specs" => spec::attached(&p, ty, view.signature_descriptors(), view.specs()),
        "tools" => {
            let element = p.array_element(ty)?;
            view.declared_tools()
                .iter()
                .map(|tool| p.metadata(element, M::Tool(tool.clone())))
                .collect::<Result<Vec<_>, _>>()
                .map(|tools| V::Array(tools.into()))
        }
        "methods" => {
            let element = p.array_element(ty)?;
            if known.callable_definition != Some(element) {
                return Err("agent methods have no checked CallableDef element ABI".into());
            }
            let methods = definition
                .backend()
                .methods(definition.definition())
                .map_err(|error| error.to_string())?;
            Ok(V::Array(
                methods
                    .into_iter()
                    .map(|method| {
                        V::CallableDefinition(CallableDefinitionValue::from_agent_method(
                            element,
                            method,
                            definition.backend().clone(),
                            definition.program().clone(),
                        ))
                    })
                    .collect::<Vec<_>>()
                    .into(),
            ))
        }
        "revision" => report::revision_info(store, known, ty, view.revision()),
        "signature" => {
            let output = p.option(ty)?;
            view.signature()
                .map(|value| signature::signature(&p, output, view.signature_descriptors(), value))
                .transpose()
                .map(optional)
        }
        "identity" => p.metadata(ty, M::Agent(view.identity().clone())),
        "dependencies" => report::dependencies(
            &p,
            ty,
            &view.dependency_info().map_err(|error| error.to_string())?,
        ),
        "effects" => {
            let current = current_analysis(&view)?;
            behavior::effects(
                &p,
                ty,
                view.signature()
                    .and_then(|signature| signature.effects)
                    .map(|row| (view.signature_descriptors(), row)),
                current.map(|(graph, analysis)| (graph, &analysis.effects)),
            )
        }
        "determinism" => p.optional_metadata(
            ty,
            current_analysis(&view)?
                .and_then(|(_, value)| value.determinism)
                .map(M::Determinism),
        ),
        "trace" => p.optional_metadata(
            ty,
            current_analysis(&view)?.and_then(|(graph, value)| {
                value.trace.map(|root| M::Trace {
                    graph: graph.clone(),
                    root,
                })
            }),
        ),
        _ => Err("unsupported checked AgentDef field".into()),
    }
}

fn current_analysis(
    view: &etas_frontend::code::definition::NativeAgentDefinitionView,
) -> Result<Option<(&DescriptorGraph, &CallableAnalysisInfo)>, String> {
    match view.current_analysis() {
        Ok(analysis) => Ok(Some((
            view.current_analysis_descriptors()
                .map_err(|error| error.to_string())?,
            analysis,
        ))),
        Err(AnalysisUnavailableReason::InputMismatch) => {
            Err("definition analysis identity differs from its revision".into())
        }
        Err(_) => Ok(None),
    }
}

pub(in crate::eval) fn callable_field(
    store: &TypeStore,
    known: &KnownCodeTypes,
    ty: TypeId,
    definition: &CallableDefinitionValue,
    field: &str,
) -> Result<V, String> {
    let p = Projector { store, known };
    let graph = definition.descriptors();
    let header = definition.header();
    let Some(TypeInfo::Function(callable)) = graph.type_info(header.signature) else {
        return Err("CallableDef has no checked signature descriptor".into());
    };
    match field {
        "id" => p.metadata(ty, M::Callable(header.id.clone())),
        "signature" => signature::signature(&p, ty, graph, callable),
        "effects" => behavior::effects(
            &p,
            ty,
            callable.effects.map(|row| (graph, row)),
            definition
                .analysis()
                .map(|(graph, analysis)| (graph, &analysis.effects)),
        ),
        "determinism" => p.optional_metadata(
            ty,
            definition
                .analysis()
                .and_then(|(_, analysis)| analysis.determinism)
                .map(M::Determinism),
        ),
        "trace" => p.optional_metadata(
            ty,
            definition.analysis().and_then(|(graph, analysis)| {
                analysis.trace.map(|root| M::Trace {
                    graph: graph.clone(),
                    root,
                })
            }),
        ),
        _ => Err("unsupported checked CallableDef field".into()),
    }
}
