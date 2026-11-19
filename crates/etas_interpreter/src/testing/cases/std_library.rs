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
            .into_checked_from_image(&path.parent().unwrap().join("installed-std"))
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
    let owner = PackageKey::from_name_and_version("std", "0.2.4").unwrap();
    for marker in ["first", "changed"] {
        std::fs::create_dir_all(source.parent().unwrap()).unwrap();
        let assembly =
            etas_std::standard_library_source(etas_std::StdLibrarySourceId::TextAssembly).source;
        let start = assembly.find("public flow join(").unwrap();
        let end = assembly.find("public flow repeat(").unwrap();
        let changed_join = assembly[start..end]
            .replace("return output;", &format!("return \"{marker}:\" + output;"));
        let text = format!(
            "module std.text;\nimport std.collections.Array;\nimport std.runtime.limits.Iterations;\n{changed_join}"
        ) + r#"
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
        let maintained = etas_std::library::source_checkout_manifest()
            .parent()
            .unwrap()
            .join("library/std");
        let collections = std::fs::read_to_string(maintained.join("collections/mod.es")).unwrap();
        let headers = &collections[..collections.find("// Insertion order").unwrap()];
        let iterator = &collections[collections.find("public type Iter<T>").unwrap()..];
        let algorithms =
            std::fs::read_to_string(maintained.join("collections/algorithms.es")).unwrap();
        let get = &algorithms[algorithms.find("    public flow get<").unwrap()
            ..algorithms.find("    public flow at<").unwrap()];
        let iteration =
            std::fs::read_to_string(maintained.join("collections/iteration.es")).unwrap();
        let iteration = &iteration[..iteration.find("impl List<T>").unwrap()];
        let collections = format!(
            "module std.collections;\n{}\n{iterator}\nimpl Array<T> {{\n{get}\n}}\n{}",
            headers.strip_prefix("module std.collections;").unwrap(),
            iteration.strip_prefix("module std.collections;").unwrap(),
        );
        let core = std::fs::read_to_string(maintained.join("core/mod.es")).unwrap();
        let errors = std::fs::read_to_string(maintained.join("runtime/error/complete.es")).unwrap();
        let index_error = errors
            .lines()
            .find(|line| line.starts_with("public type IndexError ="))
            .unwrap();
        let prelude = std::fs::read_to_string(maintained.join("prelude/mod.es")).unwrap();
        let prelude = prelude
            .lines()
            .filter_map(|line| {
                if line.starts_with("public import std.collections.") {
                    Some("public import std.collections.{Array, List, Iter};")
                } else if line.starts_with("public import std.io.") {
                    None
                } else {
                    Some(line)
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        let selected = [
            ("collections/mod.es", collections),
            (
                "runtime/limits/mod.es",
                std::fs::read_to_string(maintained.join("runtime/limits/mod.es")).unwrap(),
            ),
            (
                "runtime/budget/mod.es",
                std::fs::read_to_string(maintained.join("runtime/budget/mod.es")).unwrap(),
            ),
            (
                "runtime/time/mod.es",
                std::fs::read_to_string(maintained.join("runtime/time/mod.es")).unwrap(),
            ),
            (
                "option/mod.es",
                std::fs::read_to_string(maintained.join("option/mod.es")).unwrap(),
            ),
            (
                "result/mod.es",
                std::fs::read_to_string(maintained.join("result/mod.es")).unwrap(),
            ),
            ("core/mod.es", core),
            ("prelude/mod.es", prelude),
            (
                "runtime/error/mod.es",
                format!("module std.runtime.error;\n{index_error}"),
            ),
        ];
        for (offset, (relative, text)) in selected.into_iter().enumerate() {
            let path = root.join("library/std").join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, &text).unwrap();
            input.sources.push(SourceInput {
                id: SourceId(offset as u32 + 2),
                path: Some(path),
                text,
                kind: SourceKind::SourceProjectFile,
            });
        }
        input.project_root = root.clone();
        input.source_root = Some(root.join("library"));
        input.entry.flow = "runtime_probe".into();
        input.entry.module = Some(ModulePath {
            segments: vec!["std".into(), "text".into()],
        });
        input.environment.local_source_package = Some(owner);
        input.environment.stable_source_keys = input
            .sources
            .iter()
            .map(|source| {
                let relative = source.path.as_ref().unwrap().strip_prefix(&root).unwrap();
                ProjectStableSourceKeyInput {
                    source: source.id,
                    key: StableSourceKey::new(
                        StableSourceOrigin::Package(owner),
                        &relative
                            .components()
                            .map(|part| part.as_os_str().to_str().unwrap().to_owned())
                            .collect::<Vec<_>>(),
                    )
                    .unwrap(),
                }
            })
            .collect();
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
