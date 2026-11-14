use super::super::*;
use etas_code::{PackageKey, StableSourceKey, StableSourceOrigin};
use etas_frontend::{
    Frontend, ProjectInput, ProjectStableSourceKeyInput, SourceInput, SourceKind,
    library_hir::LibraryHirImage,
};

#[test]
fn stored_std_library_executes_generic_and_spec_bodies_in_another_process() {
    const IMAGE: &str = "ETAS_STD_EXECUTION_IMAGE";
    const EXPECTED: &str = "ETAS_STD_EXECUTION_EXPECTED";
    if let Some(path) = std::env::var_os(IMAGE) {
        let path = std::path::PathBuf::from(path);
        let image = LibraryHirImage::decode(&std::fs::read(&path).unwrap()).unwrap();
        let loaded = image
            .into_checked(
                &path.parent().unwrap().join("installed-std"),
                std::sync::Arc::new(etas_std::standard_registry()),
            )
            .unwrap();
        let host = FakeHost::new(HostServiceAvailability::default());
        let result = crate::api::run_checked_blocking(
            &loaded,
            EntryPoint {
                item: loaded.entry.unwrap(),
            },
            vec![],
            &host,
            RunOptions::default(),
        )
        .unwrap();
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        assert_eq!(
            result.value(),
            Some(&InterpValue::String(
                std::env::var(EXPECTED).unwrap().into()
            ))
        );
        assert_eq!(host.model_call_count(), 0);
        return;
    }

    let root = std::env::temp_dir().join(format!("etas-std-body-{}", std::process::id()));
    let source = root.join("library/std/text/probe.es");
    let owner = PackageKey::from_name_and_version("std", "0.2.3").unwrap();
    for marker in ["first", "changed"] {
        std::fs::create_dir_all(source.parent().unwrap()).unwrap();
        let assembly =
            etas_std::standard_library_source(etas_std::StdLibrarySourceId::TextAssembly).source;
        let start = assembly.find("public flow join(").unwrap();
        let end = assembly.find("public flow repeat(").unwrap();
        let changed_join = assembly[start..end]
            .replace("return output;", &format!("return \"{marker}:\" + output;"));
        let text = format!("{}{}{}", &assembly[..start], changed_join, &assembly[end..])
            + r#"
flow identity<T>(value: T) -> T { return value; }
spec Reader { flow read(self) -> string ![]; }
type Box = { value: string };
impl Box ~ Reader {
    public flow read(self) -> string ![] { return self.value; }
}
flow read<S ~ Reader>(value: S) -> string ![] { return value.read(); }
public flow runtime_probe() -> string {
    return read(Box { value = identity(join(["lo", "aded"], "")) });
}
"#;
        std::fs::write(&source, text).unwrap();
        let mut input = ProjectInput::single_source(SourceInput {
            id: SourceId(1),
            path: Some(source.clone()),
            text: std::fs::read_to_string(&source).unwrap(),
            kind: SourceKind::SourceProjectFile,
        });
        input.project_root = root.clone();
        input.source_root = Some(root.join("library"));
        input.entry.flow = "runtime_probe".into();
        input.entry.module = Some(ModulePath {
            segments: vec!["std".into(), "text".into()],
        });
        input.environment.local_source_package = Some(owner);
        input.environment.stable_source_keys = vec![ProjectStableSourceKeyInput {
            source: SourceId(1),
            key: StableSourceKey::new(
                StableSourceOrigin::Package(owner),
                &[
                    "library".into(),
                    "std".into(),
                    "text".into(),
                    "probe.es".into(),
                ],
            )
            .unwrap(),
        }];
        let output = Frontend.check_standard_library_source(input);
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        let image = LibraryHirImage::from_checked(&output.checked.unwrap(), owner, &root).unwrap();
        let path = root.join("std.body");
        std::fs::write(&path, image.encode().unwrap()).unwrap();
        // The receiving process has the artifact only. It cannot compile the
        // original .es file, and a body change must affect the executed value.
        std::fs::remove_dir_all(root.join("library")).unwrap();
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                std::thread::current().name().unwrap(),
                "--nocapture",
            ])
            .env(IMAGE, &path)
            .env(EXPECTED, format!("{marker}:loaded"))
            .output()
            .unwrap();
        assert!(
            child.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&child.stdout),
            String::from_utf8_lossy(&child.stderr)
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn wide_while_iteration_limit_survives_checkpoint_codec_and_resume() {
    let checked = checked_project(
        r#"
module app.main;
import std.runtime.checkpoint;
flow main() -> i32 {
    var count = 0;
    while count < 2 limit Iterations(18446744073709551615) {
        checkpoint("wide-loop");
        count = count + 1;
    }
    return count;
}
"#,
    );
    let host = FakeHost::new(availability(&[HostRequirementKind::Checkpoint]));
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            vec![],
            &host,
            RunOptions::default(),
        )
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let artifact = crate::api::codec::checkpoint_artifact_json(
        &["main.es".into()],
        "main",
        &result.checkpoints[0],
    )
    .unwrap();
    let checkpoint = crate::api::codec::checkpoint_from_json(&artifact, &checked).unwrap();
    let resumed = Interpreter
        .resume_checkpoint(&checked, &checkpoint, &host, RunOptions::default())
        .await
        .unwrap();
    assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
    assert_eq!(
        resumed.value(),
        Some(&InterpValue::Number(value::NumericValue::i32(2)))
    );
}
