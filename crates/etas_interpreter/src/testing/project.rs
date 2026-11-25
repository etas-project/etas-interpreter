use etas_core::{DiagnosticCode, EffectDiagnosticCode, SourceId};
use etas_frontend::{
    CheckedProject, Frontend, ModulePath, ProjectCompileOptions, ProjectEntry,
    ProjectEnvironmentInput, ProjectInput, SourceInput, SourceKind,
};
use std::path::PathBuf;

pub(crate) fn checked_project(text: &str) -> CheckedProject {
    checked_project_sources(vec![(SourceId(0), "src/app/main.es", text)])
}

pub(super) fn prepared_tool_definitions(
    checked: &CheckedProject,
) -> std::sync::Arc<crate::api::code::PreparedCodeDefinitions> {
    use etas_frontend::code::{
        definition::{DefinitionCatalog, NativeDefinitionStore},
        identity::DefinitionOrigins,
        service::NativeCodeSyntaxOperations,
    };
    use std::sync::Arc;
    let package = etas_code::PackageKey::from_canonical_fingerprint(
        etas_code::CodeFingerprint::of_bytes("etas.test.source-tools", b"app"),
    );
    let origins = DefinitionOrigins {
        source_modules: DefinitionOrigins::declared_source_modules(checked, package)
            .expect("fixture source origins"),
        standard_package: etas_frontend::code::identity::standard_package_key(
            &checked.std_registry,
        )
        .expect("checked standard package origin"),
        external_definitions: DefinitionOrigins::checked_external_definitions(checked)
            .expect("checked dependency origins"),
    };
    let catalog = Arc::new(
        DefinitionCatalog::build(checked.clone(), &origins, Default::default())
            .expect("fixture checked tool catalog"),
    );
    let store = Arc::new(NativeDefinitionStore::new(
        catalog,
        Arc::new(NativeCodeSyntaxOperations::default()),
        Default::default(),
    ));
    Arc::new(crate::api::code::PreparedCodeDefinitions::new(store))
}

pub(crate) fn checked_project_with_options(
    text: &str,
    options: ProjectCompileOptions,
) -> CheckedProject {
    checked_project_sources_with_options(vec![(SourceId(0), "src/app/main.es", text)], options)
}

pub(super) fn std_value(
    checked: &CheckedProject,
    role: etas_std::RuntimeTypeRole,
    payload: crate::value::InterpValue,
) -> crate::value::InterpValue {
    crate::value::InterpValue::Nominal {
        ty: checked
            .types
            .known_std_types
            .runtime_type(role)
            .expect("checked std identity"),
        value: payload.into(),
    }
}

pub(super) fn session_id(checked: &CheckedProject, id: &str) -> crate::value::InterpValue {
    std_value(
        checked,
        etas_std::RuntimeTypeRole::SessionId,
        crate::value::InterpValue::String(id.into()),
    )
}

pub(super) fn checked_project_sources(sources: Vec<(SourceId, &str, &str)>) -> CheckedProject {
    checked_project_sources_with_environment(sources, ProjectEnvironmentInput::default())
}

pub(super) fn checked_project_with_environment(
    text: &str,
    environment: ProjectEnvironmentInput,
) -> CheckedProject {
    checked_project_sources_with_environment(
        vec![(SourceId(0), "src/app/main.es", text)],
        environment,
    )
}

pub(super) fn checked_project_sources_with_environment(
    sources: Vec<(SourceId, &str, &str)>,
    environment: ProjectEnvironmentInput,
) -> CheckedProject {
    checked_project_sources_with_options_and_environment(
        sources,
        ProjectCompileOptions::default(),
        environment,
    )
}

fn checked_project_sources_with_options(
    sources: Vec<(SourceId, &str, &str)>,
    options: ProjectCompileOptions,
) -> CheckedProject {
    checked_project_sources_with_options_and_environment(
        sources,
        options,
        ProjectEnvironmentInput::default(),
    )
}

fn checked_project_sources_with_options_and_environment(
    sources: Vec<(SourceId, &str, &str)>,
    options: ProjectCompileOptions,
    environment: ProjectEnvironmentInput,
) -> CheckedProject {
    let frontend = Frontend;
    let output = frontend.check_project(ProjectInput {
        project_root: PathBuf::from("/workspace/demo"),
        source_root: Some(PathBuf::from("/workspace/demo/src")),
        options,
        environment,
        sources: sources
            .into_iter()
            .map(|(id, path, text)| SourceInput {
                id,
                path: Some(PathBuf::from("/workspace/demo").join(path)),
                text: text.to_owned(),
                kind: SourceKind::SourceProjectFile,
            })
            .collect(),
        entry: ProjectEntry {
            module: Some(ModulePath {
                segments: vec!["app".to_owned(), "main".to_owned()],
            }),
            flow: "main".to_owned(),
        },
    });
    assert!(
        !output.diagnostics.iter().any(|diagnostic| {
            diagnostic.code != DiagnosticCode::Effect(EffectDiagnosticCode::RuntimeRequiredInPhase1)
        }),
        "{:?}",
        output.diagnostics
    );
    output.checked.expect("project should be checked")
}

/// Checks `text` with a stable package source identity and this interpreter's
/// residual capabilities as the selected execution target.
pub(crate) fn residual_project_output(text: &str) -> etas_frontend::ProjectOutput {
    Frontend.check_project(residual_project_input(text))
}

pub(crate) fn residual_project_input(text: &str) -> ProjectInput {
    let package = etas_code::PackageKey::from_name_and_version("app", "1.0.0")
        .expect("valid package identity");
    let mut environment = ProjectEnvironmentInput {
        local_source_package: Some(package),
        ..Default::default()
    };
    environment
        .stable_source_keys
        .push(etas_frontend::ProjectStableSourceKeyInput {
            source: SourceId(0),
            key: etas_code::StableSourceKey::new(
                etas_code::StableSourceOrigin::Package(package),
                &["src".into(), "app".into(), "main.es".into()],
            )
            .expect("valid stable source key"),
        });
    let options = ProjectCompileOptions {
        residual_target: crate::interpreter_residual_capabilities(),
        ..Default::default()
    };
    ProjectInput {
        project_root: PathBuf::from("/workspace/demo"),
        source_root: Some(PathBuf::from("/workspace/demo/src")),
        options,
        environment,
        sources: vec![SourceInput {
            id: SourceId(0),
            path: Some(PathBuf::from("/workspace/demo/src/app/main.es")),
            text: text.to_owned(),
            kind: SourceKind::SourceProjectFile,
        }],
        entry: ProjectEntry {
            module: Some(ModulePath {
                segments: vec!["app".to_owned(), "main".to_owned()],
            }),
            flow: "main".to_owned(),
        },
    }
}

/// Native lifecycle cases use the maintained Etas library, checked once for the
/// fixture. They do not extend the legacy registry with handwritten APIs.
pub(crate) fn checked_autonomous_project() -> &'static CheckedProject {
    static PROJECT: std::sync::OnceLock<CheckedProject> = std::sync::OnceLock::new();
    PROJECT.get_or_init(|| {
        let root = etas_std::library::source_checkout_manifest()
            .parent()
            .unwrap();
        let mut loaded = etas_frontend::ProjectSourceLoader::default()
            .load(etas_frontend::ProjectSourceLoadOptions {
                project_root: root.into(),
                roots: vec![root.join("library")],
                entry: ProjectEntry {
                    module: Some(ModulePath {
                        segments: vec!["app".into(), "main".into()],
                    }),
                    flow: "main".into(),
                },
                scope: etas_frontend::ProjectSourceLoadScope::FullSourceTree,
                source_kind: None,
                import_search_roots: Vec::new(),
                external_module_roots: Vec::new(),
            })
            .unwrap();
        let id = SourceId(
            loaded
                .input
                .sources
                .iter()
                .map(|source| source.id.0)
                .max()
                .unwrap()
                + 1,
        );
        loaded.input.sources.push(SourceInput {
            id,
            path: Some(root.join("library/app/main.es")),
            text: include_str!("fixtures/autonomous_lifecycle.es").into(),
            kind: SourceKind::SourceProjectFile,
        });
        let package =
            etas_code::PackageKey::from_name_and_version("source_lifecycle_fixture", "1.0.0")
                .unwrap();
        loaded.input.environment.local_source_package = Some(package);
        loaded.input.environment.stable_source_keys = loaded
            .input
            .sources
            .iter()
            .map(|source| etas_frontend::ProjectStableSourceKeyInput {
                source: source.id,
                key: etas_code::StableSourceKey::new(
                    etas_code::StableSourceOrigin::Package(package),
                    &source
                        .path
                        .as_ref()
                        .unwrap()
                        .strip_prefix(root)
                        .unwrap()
                        .components()
                        .map(|part| part.as_os_str().to_str().unwrap().to_owned())
                        .collect::<Vec<_>>(),
                )
                .unwrap(),
            })
            .collect();
        let output = Frontend.check_standard_library_source(loaded.input);
        assert!(
            !output
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.severity == etas_core::Severity::Error),
            "{:?}",
            output.diagnostics
        );
        output
            .checked
            .expect("maintained source std and lifecycle fixture are checked")
    })
}
