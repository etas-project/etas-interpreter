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
    ExternalModuleId, ExternalPackageId, ExternalSymbolId, ModulePath, ProjectEnvironmentInput,
    ProjectExternalActionTraceInput, ProjectExternalEffectRowInput,
    ProjectExternalEffectSummaryInput, ProjectExternalExportInput, ProjectExternalModuleInput,
    ProjectExternalPackageInput, ProjectExternalPublicMetadataInput,
    ProjectExternalToolSchemaInput, ProjectExternalToolSignatureInput, ProjectExternalTypeInput,
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

pub(crate) fn full_model_capabilities() -> ModelProviderCapabilities {
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
    declared_host_search_environment(include_schema, true)
}

fn checked_host_search_project(source: &str) -> etas_frontend::CheckedProject {
    let source = source
        .replace(
            "import std.agent.prompt.Prompt;",
            "import std.agent.prompt.Prompt;\nimport dep.tools.Search;",
        )
        .replace("agent Writer(", "@tools([Search])\nagent Writer(");
    checked_project_with_environment(&source, declared_host_search_environment(true, false))
}

fn declared_host_search_environment(
    include_schema: bool,
    has_query: bool,
) -> ProjectEnvironmentInput {
    let origin = etas_code::PackageKey::from_name_and_version("dep-tools", "1.0.0").unwrap();
    let parameters = if has_query { "query: string" } else { "" };
    let source =
        format!("module dep.tools; public tool Search({parameters}) -> string ![Network];");
    let mut input = etas_frontend::ProjectInput::single_source(
        etas_frontend::SourceInput::anonymous(source.clone()),
    );
    input.options.entry_policy = etas_frontend::EntryPolicy::Optional;
    input.environment.local_source_package = Some(origin);
    input
        .environment
        .tool_bindings
        .push(etas_frontend::ProjectToolBindingInput {
            tool: "dep.tools.Search".into(),
            provider: "fixture.search".into(),
            effect_row: vec!["Network".into()],
            action_row: Vec::new(),
        });
    let output = etas_frontend::Frontend.check_project(input);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let binding = etas_package_metadata::ToolBinding {
        tool: "dep.tools.Search".into(),
        kind: "host".into(),
        provider: "fixture.search".into(),
        effect_row: vec!["Network".into()],
        action_row: Vec::new(),
    };
    let artifact = etas_frontend::build_package_metadata_artifact_from_checked(
        etas_frontend::PackageMetadataBuildInput {
            definition_origin: Some(origin),
            package_id: "dep-tools".into(),
            package_version: "1.0.0".into(),
            package_edition: "2026".into(),
            compiler_version: etas_frontend::FRONTEND_COMPILER_VERSION.into(),
            source_payload_hash: etas_package_metadata::blake3_hash(source.as_bytes()),
            manifest_hash: etas_package_metadata::blake3_hash(b"dep-tools:fixture.search"),
            dependency_lock_hash: etas_package_metadata::blake3_hash(b""),
            bins: Vec::new(),
            dependencies: Vec::new(),
            tool_bindings: vec![binding],
        },
        output.checked.as_ref().expect("checked host tool package"),
    )
    .expect("published host tool declaration");
    let (_, metadata) = etas_package_metadata::package_metadata_from_artifact(
        std::path::Path::new("dep-tools.etasmeta"),
        &artifact.bytes,
    )
    .expect("host tool package metadata");
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
            scoped_callbacks: Vec::new(),
            package,
            types: Vec::new(),
            values: Vec::new(),
            enums: Vec::new(),
            flows: Vec::new(),
            agents: Vec::new(),
            tools: vec![ProjectExternalToolSignatureInput {
                parameter_defaults: Vec::new(),
                generic_params: Vec::new(),
                path: tool_path.clone(),
                param_names: if has_query {
                    vec!["query".into()]
                } else {
                    Vec::new()
                },
                input: if has_query {
                    vec![ProjectExternalTypeInput::Primitive("string".into())]
                } else {
                    Vec::new()
                },
                output: ProjectExternalTypeInput::Primitive("string".to_owned()),
                effects: Some(ProjectExternalEffectRowInput {
                    effects: vec![etas_frontend::ProjectExternalEffectRefInput {
                        path: vec!["std".into(), "effects".into(), "Network".into()],
                        args: Vec::new(),
                    }],
                    tail: None,
                }),
                visibility: "public".to_owned(),
            }],
            tool_schemas: if include_schema {
                vec![ProjectExternalToolSchemaInput {
                    path: tool_path.clone(),
                    schema_json: if has_query {
                        r#"{"type":"object","properties":{"query":{"type":"string"}},"required":["query"],"additionalProperties":false}"#.to_owned()
                    } else {
                        r#"{"type":"object","properties":{},"additionalProperties":false}"#
                            .to_owned()
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
                definition: None,
                residual_checks: vec![],
                behavior: None,
                handler_requests: None,
                returned_callables: None,
                default_handler_effects: None,
                item: tool_path,
                public_effects: ProjectExternalEffectRowInput {
                    effects: vec![etas_frontend::ProjectExternalEffectRefInput {
                        path: vec!["std".into(), "effects".into(), "Network".into()],
                        args: Vec::new(),
                    }],
                    tail: None,
                },
                requested_actions: ProjectExternalEffectRowInput::default(),
                handled_requested_actions: ProjectExternalEffectRowInput::default(),
                latent_flows: Vec::new(),
                action_trace: ProjectExternalActionTraceInput::Empty,
            }],
            action_summaries: Vec::new(),
            trace_spec_summaries: Vec::new(),
            re_exports: Vec::new(),
        }],
        external_definition_origins: vec![etas_frontend::ProjectExternalDefinitionOriginsInput {
            package,
            origins: metadata
                .public_metadata
                .definition_origins
                .expect("published package origins"),
            descriptors: metadata
                .public_metadata
                .definition_descriptors
                .map(std::sync::Arc::new),
        }],
        tool_bindings: vec![etas_frontend::ProjectToolBindingInput {
            tool: "dep.tools.Search".into(),
            provider: "fixture.search".into(),
            effect_row: vec!["Network".into()],
            action_row: Vec::new(),
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
