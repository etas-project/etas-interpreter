use std::sync::Arc;

use etas_code::{
    CallableId, DefinitionKey,
    contract::TraceInclusionWitness,
    descriptor::{
        DescriptorGraph, DeterminismInfo, EffectRowInfoRef, GenericOwner, StaticValueInfo,
        TraceInfoRef, TypeInfoRef,
    },
    diagnostic::DescriptorDetail,
};
use etas_types::{TypeId, facts::symbols::KnownCodeTypes};

mod serialize;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodeMetadataValue(Arc<MetadataData>);

#[derive(Debug, PartialEq, Eq)]
struct MetadataData {
    ty: TypeId,
    payload: CodeMetadataPayload,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CodeMetadataPayload {
    Agent(DefinitionKey),
    Type {
        graph: DescriptorGraph,
        root: TypeInfoRef,
    },
    GenericOwner(GenericOwner),
    Definition(DefinitionKey),
    Callable(CallableId),
    EffectRow {
        graph: DescriptorGraph,
        root: EffectRowInfoRef,
    },
    Actions {
        graph: DescriptorGraph,
        root: EffectRowInfoRef,
    },
    Trace {
        graph: DescriptorGraph,
        root: TraceInfoRef,
    },
    Determinism(DeterminismInfo),
    Detail(DescriptorDetail),
    TraceWitness(TraceInclusionWitness),
    Tool(DefinitionKey),
    StaticValue(Arc<StaticValueInfo>),
    TraceConstraint {
        graph: DescriptorGraph,
        value: Arc<etas_code::descriptor::TraceConstraintInfo>,
    },
}

impl CodeMetadataValue {
    pub(crate) fn new(
        known: &KnownCodeTypes,
        ty: TypeId,
        payload: CodeMetadataPayload,
    ) -> Result<Self, String> {
        Self::validate_payload(known, ty, &payload)?;
        Ok(Self(Arc::new(MetadataData { ty, payload })))
    }

    pub(crate) fn validate_role(&self, known: &KnownCodeTypes) -> Result<(), String> {
        Self::validate_payload(known, self.ty(), self.payload())
    }

    fn validate_payload(
        known: &KnownCodeTypes,
        ty: TypeId,
        payload: &CodeMetadataPayload,
    ) -> Result<(), String> {
        let expected = match payload {
            CodeMetadataPayload::Agent(identity) => {
                if identity.kind() != etas_code::DefinitionKind::Agent {
                    return Err("AgentId requires a checked nominal agent identity".into());
                }
                known.agent_id
            }
            CodeMetadataPayload::Type { graph, root } => {
                graph
                    .type_info(*root)
                    .ok_or("compiler metadata has an invalid type root")?;
                known.type_info
            }
            CodeMetadataPayload::GenericOwner(_) => known.generic_owner_info,
            CodeMetadataPayload::Tool(identity) => {
                if identity.kind() != etas_code::DefinitionKind::Tool {
                    return Err("ToolRef requires a checked tool declaration identity".into());
                }
                known.tool_ref
            }
            CodeMetadataPayload::StaticValue(_) => known.static_value_info,
            CodeMetadataPayload::TraceConstraint { graph, value } => {
                value.validate(graph).map_err(|error| error.to_string())?;
                known.trace_constraint_info
            }
            CodeMetadataPayload::Definition(_) => known.definition_id,
            CodeMetadataPayload::Callable(_) => known.callable_id,
            CodeMetadataPayload::EffectRow { graph, root } => {
                graph
                    .effect_row(*root)
                    .ok_or("compiler metadata has an invalid effect-row root")?;
                known.effect_row_info
            }
            CodeMetadataPayload::Actions { graph, root } => {
                graph
                    .effect_row(*root)
                    .ok_or("compiler metadata has an invalid action-row root")?;
                known.action_summary
            }
            CodeMetadataPayload::Trace { graph, root } => {
                graph
                    .trace(*root)
                    .ok_or("compiler metadata has an invalid trace root")?;
                known.trace_summary
            }
            CodeMetadataPayload::Determinism(_) => known.determinism,
            CodeMetadataPayload::Detail(_) => known.descriptor_detail,
            CodeMetadataPayload::TraceWitness(_) => known.trace_witness,
        };
        if expected != Some(ty) {
            return Err("compiler metadata differs from its checked standard type role".into());
        }
        Ok(())
    }
    pub fn ty(&self) -> TypeId {
        self.0.ty
    }
    pub(crate) fn payload(&self) -> &CodeMetadataPayload {
        &self.0.payload
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use etas_code::descriptor::{DescriptorGraphInput, EffectRowInfo};

    fn graph() -> DescriptorGraph {
        DescriptorGraph::new(
            DescriptorGraphInput {
                rows: vec![EffectRowInfo {
                    effects: Vec::new(),
                    tail: None,
                }],
                ..Default::default()
            },
            Default::default(),
        )
        .unwrap()
    }

    #[test]
    fn descriptor_role_and_graph_root_must_both_match() {
        let known = KnownCodeTypes {
            effect_row_info: Some(TypeId(11)),
            action_summary: Some(TypeId(12)),
            ..Default::default()
        };
        let payload = || CodeMetadataPayload::EffectRow {
            graph: graph(),
            root: EffectRowInfoRef(0),
        };
        assert!(CodeMetadataValue::new(&known, TypeId(11), payload()).is_ok());
        assert!(CodeMetadataValue::new(&known, TypeId(12), payload()).is_err());
        assert!(CodeMetadataValue::new(&KnownCodeTypes::default(), TypeId(11), payload()).is_err());
        assert!(
            CodeMetadataValue::new(
                &known,
                TypeId(11),
                CodeMetadataPayload::EffectRow {
                    graph: graph(),
                    root: EffectRowInfoRef(1)
                }
            )
            .is_err()
        );
    }

    #[test]
    fn type_descriptors_require_the_checked_role_and_an_existing_graph_node() {
        use etas_code::descriptor::{PrimitiveInfo, TypeInfo};
        let graph = DescriptorGraph::new(
            DescriptorGraphInput {
                types: vec![TypeInfo::Primitive(PrimitiveInfo::I32)],
                ..Default::default()
            },
            Default::default(),
        )
        .unwrap();
        let known = KnownCodeTypes {
            type_info: Some(TypeId(11)),
            definition_id: Some(TypeId(12)),
            ..Default::default()
        };
        let payload = |root| CodeMetadataPayload::Type {
            graph: graph.clone(),
            root: TypeInfoRef(root),
        };
        assert!(CodeMetadataValue::new(&known, TypeId(11), payload(0)).is_ok());
        assert!(CodeMetadataValue::new(&known, TypeId(12), payload(0)).is_err());
        assert!(CodeMetadataValue::new(&known, TypeId(11), payload(1)).is_err());
        assert!(
            CodeMetadataValue::new(&KnownCodeTypes::default(), TypeId(11), payload(0)).is_err()
        );
    }

    #[test]
    fn tool_descriptors_require_a_tool_identity_and_the_checked_tool_role() {
        use etas_code::{CodeFingerprint, DefinitionKind, PackageKey};
        let key = |kind| {
            DefinitionKey::new(
                PackageKey::from_canonical_fingerprint(CodeFingerprint::from_bytes([7; 32])),
                vec!["app".into()],
                vec!["helper".into()],
                kind,
            )
            .unwrap()
        };
        let known = KnownCodeTypes {
            tool_ref: Some(TypeId(11)),
            definition_id: Some(TypeId(12)),
            ..Default::default()
        };
        assert!(
            CodeMetadataValue::new(
                &known,
                TypeId(11),
                CodeMetadataPayload::Tool(key(DefinitionKind::Tool))
            )
            .is_ok()
        );
        assert!(
            CodeMetadataValue::new(
                &known,
                TypeId(11),
                CodeMetadataPayload::Tool(key(DefinitionKind::Flow))
            )
            .is_err()
        );
        assert!(
            CodeMetadataValue::new(
                &known,
                TypeId(12),
                CodeMetadataPayload::Tool(key(DefinitionKind::Tool))
            )
            .is_err()
        );
    }

    #[test]
    fn descriptor_json_preserves_the_typed_graph_and_root() {
        let known = KnownCodeTypes {
            effect_row_info: Some(TypeId(11)),
            ..Default::default()
        };
        let value = CodeMetadataValue::new(
            &known,
            TypeId(11),
            CodeMetadataPayload::EffectRow {
                graph: graph(),
                root: EffectRowInfoRef(0),
            },
        )
        .unwrap();
        let json =
            crate::api::codec::value_json(&crate::value::InterpValue::CodeMetadata(value)).unwrap();
        assert_eq!(json["kind"], "compiler_metadata");
        assert_eq!(json["schema"], 1);
        assert_eq!(json["value"]["EffectRow"]["root"], 0);
        assert!(json["value"]["EffectRow"]["graph"]["rows"][0]["effects"].is_array());
        assert!(json["value"]["EffectRow"]["graph"]["rows"][0]["tail"].is_null());
    }

    #[test]
    fn descriptor_json_does_not_export_unreachable_catalog_nodes() {
        use etas_code::descriptor::{FieldInfo, PrimitiveInfo, TraceInfo, TypeInfo};
        let graph = DescriptorGraph::new(
            DescriptorGraphInput {
                types: vec![
                    TypeInfo::Primitive(PrimitiveInfo::String),
                    TypeInfo::Record(vec![FieldInfo {
                        name: "private_catalog_field".into(),
                        ty: TypeInfoRef(0),
                    }]),
                ],
                rows: vec![
                    EffectRowInfo {
                        effects: Vec::new(),
                        tail: None,
                    },
                    EffectRowInfo {
                        effects: Vec::new(),
                        tail: None,
                    },
                ],
                traces: vec![TraceInfo::Empty],
                ..Default::default()
            },
            Default::default(),
        )
        .unwrap();
        let value = CodeMetadataValue::new(
            &KnownCodeTypes {
                effect_row_info: Some(TypeId(11)),
                ..Default::default()
            },
            TypeId(11),
            CodeMetadataPayload::EffectRow {
                graph,
                root: EffectRowInfoRef(1),
            },
        )
        .unwrap();
        let json =
            crate::api::codec::value_json(&crate::value::InterpValue::CodeMetadata(value)).unwrap();
        let exported = &json["value"]["EffectRow"];
        assert!(!json.to_string().contains("private_catalog_field"));
        assert_eq!(exported["graph"]["rows"].as_array().unwrap().len(), 1);
        for kind in ["types", "binders", "traces"] {
            assert!(exported["graph"][kind].as_array().unwrap().is_empty());
        }
        assert_eq!(exported["root"], 0);
    }
}
