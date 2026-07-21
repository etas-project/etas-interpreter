use etas_core::{DiagnosticCode, EffectDiagnosticCode, SourceId};
use etas_frontend::{
    CheckedProject, Frontend, ModulePath, ProjectCompileOptions, ProjectEntry,
    ProjectEnvironmentInput, ProjectInput, SourceInput, SourceKind,
};
use std::path::PathBuf;

pub(crate) fn checked_project(text: &str) -> CheckedProject {
    checked_project_sources(vec![(SourceId(0), "src/app/main.es", text)])
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
    let frontend = Frontend;
    let output = frontend.check_project(ProjectInput {
        project_root: PathBuf::from("/workspace/demo"),
        source_root: Some(PathBuf::from("/workspace/demo/src")),
        options: ProjectCompileOptions::default(),
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
