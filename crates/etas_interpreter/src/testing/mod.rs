use super::*;
mod host;
pub(crate) mod project;
use self::host::{FakeHost, availability};
use self::project::{checked_project, checked_project_sources, checked_project_with_environment};
use crate::eval::resolve_std_type;
use crate::host::HostServiceAvailability;
use crate::orchestration::{CompletedHostBoundary, WorkflowEvent};
use etas_core::{AnalysisDiagnosticCode, DiagnosticCode, SourceId};
use etas_effects::{EffectRow, ErrorConversionFact, HostRequirementKind, InterpreterSupport};
use etas_frontend::{
    ExternalModuleId, ExternalPackageId, ExternalSymbolId, ModulePath, ProjectEnvironmentInput,
    ProjectExternalExportInput, ProjectExternalModuleInput, ProjectExternalPackageInput,
    ProjectExternalPublicMetadataInput, ProjectExternalRecordFieldInput,
    ProjectExternalToolSchemaInput, ProjectExternalToolSignatureInput, ProjectExternalTypeInput,
};
use etas_hir::{
    HirArg, HirBlock, HirExpr, HirItem, HirStmt, PartialResolution, PartialResolutionReason,
    ResolveResult, SymbolDef, unresolved_path_from_segments,
};
use etas_host::HostErrorCode;
use etas_host::{
    ApprovalDecision, ApprovalRequest, AuthorityContext, Budget, HostActionGrant, HostFieldSchema,
    HostRequestId, HostSchema, HostValue, ModelContent, ModelName, ModelProviderCapabilities,
    ModelProviderId, ModelRole, PolicyDecision, SandboxPolicy, TokenBudget, ToolRef, ToolSchema,
    TraceContext, TraceId,
};
use etas_types::{NamedTypeRef, NominalTypeRef, Type};

fn full_model_capabilities() -> ModelProviderCapabilities {
    ModelProviderCapabilities {
        supports_forced_tool_output: true,
        supports_json_schema_response_format: true,
        supports_plain_json_text_instruction: true,
        supports_tool_call_loop: true,
        supports_required_tool_choice: true,
    }
}

fn boundary_policy_context(policy_ref: HostValue) -> etas_host::PolicyContext {
    etas_host::PolicyContext {
        active_trace_specs: Vec::new(),
        trace_spec_facts: Vec::new(),
        labels: Vec::new(),
        boundary_policy_ref: Some(policy_ref),
    }
}

fn external_search_tool_environment(include_schema: bool) -> ProjectEnvironmentInput {
    let package = ExternalPackageId(7);
    let module = ExternalModuleId(11);
    let symbol = ExternalSymbolId(13);
    let tool_path = vec!["dep".to_owned(), "tools".to_owned(), "Search".to_owned()];
    ProjectEnvironmentInput {
        external_packages: vec![ProjectExternalPackageInput {
            id: package,
            name: "dep-tools".to_owned(),
            version: "1.0.0".to_owned(),
            edition: "2026".to_owned(),
            import_root: "dep".to_owned(),
        }],
        external_modules: vec![ProjectExternalModuleInput {
            package: Some(package),
            id: module,
            path: ModulePath {
                segments: vec!["dep".to_owned(), "tools".to_owned()],
            },
            exports: vec![ProjectExternalExportInput {
                symbol,
                name: "Search".to_owned(),
                visibility: etas_hir::Visibility::Public,
            }],
        }],
        external_public_metadata: vec![ProjectExternalPublicMetadataInput {
            package,
            types: Vec::new(),
            values: Vec::new(),
            enums: Vec::new(),
            flows: Vec::new(),
            agents: Vec::new(),
            tools: vec![ProjectExternalToolSignatureInput {
                path: tool_path.clone(),
                param_names: vec!["input".to_owned()],
                input: vec![ProjectExternalTypeInput::Record {
                    fields: vec![ProjectExternalRecordFieldInput {
                        name: "query".to_owned(),
                        ty: ProjectExternalTypeInput::Primitive("string".to_owned()),
                    }],
                }],
                output: ProjectExternalTypeInput::Primitive("string".to_owned()),
                effects: None,
                visibility: "public".to_owned(),
            }],
            tool_schemas: if include_schema {
                vec![ProjectExternalToolSchemaInput {
                    path: tool_path,
                    schema_json: r#"{"type":"object","properties":{"query":{"type":"string"}},"required":["query"],"additionalProperties":false}"#
                        .to_owned(),
                }]
            } else {
                Vec::new()
            },
            effects: Vec::new(),
            actions: Vec::new(),
            trace_specs: Vec::new(),
            spec_signatures: Vec::new(),
            spec_impls: Vec::new(),
            type_spec_satisfactions: Vec::new(),
            callable_spec_satisfactions: Vec::new(),
            trace_spec_conformances: Vec::new(),
            effect_summaries: Vec::new(),
            action_summaries: Vec::new(),
            trace_spec_summaries: Vec::new(),
            re_exports: Vec::new(),
        }],
        ..ProjectEnvironmentInput::default()
    }
}

mod cases;
