use etas_code::descriptor::{
    DescriptorGraph, DescriptorNode, EffectRowInfoRef, TraceInfoRef, TypeInfoRef,
};
use serde::{Serialize, Serializer, ser::Error};

use super::CodeMetadataPayload;

impl Serialize for CodeMetadataPayload {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Agent(value) => {
                serializer.serialize_newtype_variant("CodeMetadataPayload", 0, "Agent", value)
            }
            Self::Type { graph, root } => graph_payload(
                serializer,
                1,
                "Type",
                graph,
                DescriptorNode::Type(*root),
                TypeInfoRef(0),
            ),
            Self::GenericOwner(value) => serializer.serialize_newtype_variant(
                "CodeMetadataPayload",
                2,
                "GenericOwner",
                value,
            ),
            Self::Tool(value) => {
                serializer.serialize_newtype_variant("CodeMetadataPayload", 11, "Tool", value)
            }
            Self::StaticValue(value) => serializer.serialize_newtype_variant(
                "CodeMetadataPayload",
                12,
                "StaticValue",
                value,
            ),
            Self::TraceConstraint { graph, value } => {
                #[derive(Serialize)]
                struct Payload {
                    graph: DescriptorGraph,
                    value: etas_code::descriptor::TraceConstraintInfo,
                }
                let (graph, value) = value.project(graph).map_err(S::Error::custom)?;
                serializer.serialize_newtype_variant(
                    "CodeMetadataPayload",
                    13,
                    "TraceConstraint",
                    &Payload { graph, value },
                )
            }
            Self::Definition(value) => {
                serializer.serialize_newtype_variant("CodeMetadataPayload", 3, "Definition", value)
            }
            Self::Callable(value) => {
                serializer.serialize_newtype_variant("CodeMetadataPayload", 4, "Callable", value)
            }
            Self::EffectRow { graph, root } => graph_payload(
                serializer,
                5,
                "EffectRow",
                graph,
                DescriptorNode::Row(*root),
                EffectRowInfoRef(0),
            ),
            Self::Actions { graph, root } => graph_payload(
                serializer,
                6,
                "Actions",
                graph,
                DescriptorNode::Row(*root),
                EffectRowInfoRef(0),
            ),
            Self::Trace { graph, root } => graph_payload(
                serializer,
                7,
                "Trace",
                graph,
                DescriptorNode::Trace(*root),
                TraceInfoRef(0),
            ),
            Self::Determinism(value) => {
                serializer.serialize_newtype_variant("CodeMetadataPayload", 8, "Determinism", value)
            }
            Self::Detail(value) => {
                serializer.serialize_newtype_variant("CodeMetadataPayload", 9, "Detail", value)
            }
            Self::TraceWitness(value) => serializer.serialize_newtype_variant(
                "CodeMetadataPayload",
                10,
                "TraceWitness",
                value,
            ),
        }
    }
}

fn graph_payload<S: Serializer, R: Serialize>(
    serializer: S,
    index: u32,
    variant: &'static str,
    graph: &DescriptorGraph,
    root: DescriptorNode,
    projected_root: R,
) -> Result<S::Ok, S::Error> {
    #[derive(Serialize)]
    struct Payload<R> {
        graph: DescriptorGraph,
        root: R,
    }
    let payload = Payload {
        graph: graph.project(root).map_err(S::Error::custom)?,
        root: projected_root,
    };
    serializer.serialize_newtype_variant("CodeMetadataPayload", index, variant, &payload)
}
