use super::*;
mod aggregate_performance;
pub(crate) mod allocation;
mod execution_performance;
mod host;
pub(crate) mod project;
mod subprocess;
pub(crate) use self::host::FakeHost;
use self::host::availability;
use self::project::{
    checked_project, checked_project_sources, checked_project_with_environment,
    checked_project_with_options, session_id, std_value,
};
use crate::eval::resolve_std_type;
use crate::host::HostServiceAvailability;
use crate::orchestration::{CompletedHostBoundary, WorkflowEvent};
use etas_core::{AnalysisDiagnosticCode, DiagnosticCode, SourceId};
use etas_effects::{EffectRow, ErrorConversionFact, HostRequirementKind, InterpreterSupport};
use etas_frontend::{
    EntryPolicy, ExternalModuleId, ExternalPackageId, ExternalSymbolId, Frontend, ModulePath,
    ProjectEnvironmentInput, ProjectExternalActionTraceInput, ProjectExternalEffectRowInput,
    ProjectExternalEffectSummaryInput, ProjectExternalExportInput, ProjectExternalModuleInput,
    ProjectExternalPackageInput, ProjectExternalPublicMetadataInput,
    ProjectExternalToolSchemaInput, ProjectExternalToolSignatureInput, ProjectExternalTypeInput,
    ProjectInput, SourceInput,
};
use etas_hir::{
    HirArg, HirBlock, HirExpr, HirItem, HirStmt, PartialResolution, PartialResolutionReason,
    ResolveResult, SymbolDef, unresolved_path_from_segments,
};
use etas_host::HostErrorCode;
use etas_host::{
    ApprovalDecision, ApprovalRequest, ApprovalResponse, AuthorityContext, Budget, HostActionGrant,
    HostFieldSchema, HostRequestId, HostSchema, HostValue, ModelContent, ModelName,
    ModelProviderCapabilities, ModelProviderId, ModelRole, PolicyDecision, SandboxPolicy,
    TokenBudget, ToolRef, ToolSchema, TraceContext, TraceId,
};
use etas_types::{NamedTypeRef, NominalTypeRef, Type};
pub(crate) use subprocess::in_subprocess;

pub(crate) fn edit_checked_hir<R>(
    checked: &mut etas_frontend::CheckedProject,
    edit: impl FnOnce(&mut etas_hir::HirProgram) -> R,
) -> Result<R, etas_hir::HirTreeIndexError> {
    let mut editor = checked.hir.clone().edit();
    let result = edit(editor.program_mut());
    let revision = editor.finish()?;
    std::sync::Arc::make_mut(checked).hir = revision;
    Ok(result)
}

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
    search_tool_environment(include_schema, true)
}

fn search_tool_environment(include_schema: bool, with_query: bool) -> ProjectEnvironmentInput {
    let package = ExternalPackageId(7);
    let module = ExternalModuleId(11);
    let symbol = ExternalSymbolId(13);
    let tool_path = vec!["dep".to_owned(), "tools".to_owned(), "Search".to_owned()];
    let source = if with_query {
        "module dep.tools; public tool Search(query: string) -> string ![] { return query; }"
    } else {
        "module dep.tools; public tool Search() -> string ![] { return \"native result\"; }"
    };
    let mut input = ProjectInput::single_source(SourceInput::anonymous(source));
    input.options.entry_policy = EntryPolicy::Optional;
    input
        .environment
        .tool_bindings
        .push(etas_frontend::ProjectToolBindingInput {
            tool: "dep.tools.Search".into(),
            provider: "test-native".into(),
            effect_row: vec![],
            action_row: vec![],
        });
    let output = Frontend.check_project(input);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    use etas_frontend::code::{
        definition::DefinitionCatalog,
        identity::{DefinitionIdentityIndex, DefinitionOrigins},
    };
    let checked = output.checked.unwrap();
    let package_key = etas_code::PackageKey::from_name_and_version("dep-tools", "1.0.0").unwrap();
    let origins = DefinitionOrigins {
        source_modules: DefinitionOrigins::declared_source_modules(&checked, package_key).unwrap(),
        standard_package: etas_frontend::code::identity::standard_package_key(
            &checked.std_registry,
        )
        .unwrap(),
        external_definitions: Default::default(),
    };
    let identities = DefinitionIdentityIndex::build(&checked, &origins).unwrap();
    let (item, native_symbol) = checked
        .hir
        .items
        .iter()
        .find_map(|(item, declaration)| match declaration {
            HirItem::Tool(tool) => Some((item, tool.symbol)),
            _ => None,
        })
        .unwrap();
    let definition = identities.symbol(native_symbol).unwrap().clone();
    let descriptors = DefinitionCatalog::exported_descriptors(
        checked,
        &origins,
        identities,
        &[item].into(),
        Default::default(),
    )
    .unwrap();
    let portable_origins = etas_code::identity::PackageDefinitionOrigins {
        package: package_key,
        modules: vec![etas_code::identity::DefinitionModuleOrigin {
            path: vec!["dep".into(), "tools".into()],
            canonical_path: vec!["dep".into(), "tools".into()],
        }],
        declarations: vec![etas_code::identity::DefinitionBindingOrigin {
            module: vec!["dep".into(), "tools".into()],
            declaration: vec!["Search".into()],
            definition,
        }],
    };
    ProjectEnvironmentInput {
        external_definition_origins: vec![etas_frontend::ProjectExternalDefinitionOriginsInput {
            package,
            origins: portable_origins,
            descriptors: Some(descriptors),
        }],
        tool_bindings: vec![etas_frontend::ProjectToolBindingInput {
            tool: "dep.tools.Search".into(),
            provider: "test-native".into(),
            effect_row: vec![],
            action_row: vec![],
        }],
        external_packages: vec![ProjectExternalPackageInput {
            id: package,
            name: "dep-tools".to_owned(),
            version: "1.0.0".to_owned(),
            edition: "2026".to_owned(),
            import_root: "dep".to_owned(),
            std_compatibility: etas_std::standard_registry()
                .version()
                .compatibility_identity(),
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
                generic_params: Vec::new(),
                path: tool_path.clone(),
                param_names: if with_query {
                    vec!["query".into()]
                } else {
                    vec![]
                },
                input: if with_query {
                    vec![ProjectExternalTypeInput::Primitive("string".into())]
                } else {
                    vec![]
                },
                output: ProjectExternalTypeInput::Primitive("string".to_owned()),
                effects: Some(ProjectExternalEffectRowInput::default()),
                visibility: "public".to_owned(),
            }],
            tool_schemas: if include_schema {
                vec![ProjectExternalToolSchemaInput {
                    path: tool_path.clone(),
                    schema_json: if with_query {
                        r#"{"type":"object","properties":{"query":{"type":"string"}},"required":["query"],"additionalProperties":false}"#.into()
                    } else {
                        r#"{"type":"object","properties":{},"additionalProperties":false}"#.into()
                    },
                }]
            } else {
                Vec::new()
            },
            effects: Vec::new(),
            actions: Vec::new(),
            trace_specs: Vec::new(),
            spec_signatures: Vec::new(),
            spec_impls: Vec::new(),
            methods: Vec::new(),
            type_spec_satisfactions: Vec::new(),
            callable_spec_satisfactions: Vec::new(),
            trace_spec_conformances: Vec::new(),
            effect_summaries: vec![ProjectExternalEffectSummaryInput {
                residual_checks: vec![],
                behavior: None,
                handler_requests: None,
                returned_callables: None,
                default_handler_effects: None,
                item: tool_path,
                public_effects: ProjectExternalEffectRowInput::default(),
                requested_actions: ProjectExternalEffectRowInput::default(),
                handled_requested_actions: ProjectExternalEffectRowInput::default(),
                latent_flows: Vec::new(),
                action_trace: ProjectExternalActionTraceInput::Empty,
            }],
            action_summaries: Vec::new(),
            trace_spec_summaries: Vec::new(),
            re_exports: Vec::new(),
        }],
        ..ProjectEnvironmentInput::default()
    }
}

mod cases;
pub(crate) fn synthetic_span() -> etas_core::Span {
    etas_core::Span::new(
        etas_core::SourceId(0),
        etas_core::TextRange::empty(etas_core::TextSize(0)),
    )
}
