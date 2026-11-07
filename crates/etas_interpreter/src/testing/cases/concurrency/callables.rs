use super::*;

fn generic_mapper(inputs: &str, output: &str) -> etas_frontend::CheckedProject {
    checked_project(&format!(
        r#"
module app.main;
import std.task.{{map_concurrent, run, Scheduler}};
flow keep<T>(value: T) -> T {{
    var index = 0;
    while index < 20000 limit Iterations(20000) {{ index = index + 1; }}
    return value;
}}
flow main() -> Array<{output}> {{
    return run((scheduler: Scheduler) => {{ return map_concurrent({inputs}, keep); }});
}}
"#
    ))
}

async fn assert_map_workers(checked: &etas_frontend::CheckedProject, nested: bool) {
    let (inline, _) = run_with_cpu_threads(checked, 1).await;
    let (parallel, telemetry) = run_with_cpu_threads(checked, 4).await;
    assert!(inline.diagnostics.is_empty(), "{:?}", inline.diagnostics);
    assert!(
        parallel.diagnostics.is_empty(),
        "{:?}",
        parallel.diagnostics
    );
    let expected = InterpValue::Array(
        [7, 3, 9, 1]
            .into_iter()
            .map(|value| {
                let value = InterpValue::i32(value);
                if nested {
                    InterpValue::Array(vec![value].into())
                } else {
                    value
                }
            })
            .collect::<Vec<_>>()
            .into(),
    );
    assert_eq!(inline.value(), Some(&expected));
    assert_eq!(parallel.value(), inline.value());
    let workers = parallel
        .events
        .iter()
        .filter_map(|event| match event {
            WorkflowEvent::TaskCpuCompleted { worker, .. } => Some(*worker),
            _ => None,
        })
        .collect::<std::collections::BTreeSet<_>>();
    assert!(workers.len() >= 2, "{workers:?}");
    assert_eq!(telemetry.len(), 4);
    assert!(
        telemetry
            .iter()
            .filter_map(|task| task.thread)
            .collect::<std::collections::HashSet<_>>()
            .len()
            >= 2
    );
}

#[tokio::test(flavor = "current_thread")]
async fn generic_named_cpu_tasks_preserve_bindings_order_and_worker_execution() {
    for (inputs, output, nested) in [
        ("[7, 3, 9, 1]", "i32", false),
        ("[[7], [3], [9], [1]]", "Array<i32>", true),
    ] {
        assert_map_workers(&generic_mapper(inputs, output), nested).await;
    }
}

#[tokio::test(flavor = "current_thread")]
async fn package_cpu_tasks_preserve_owner_order_and_worker_execution() {
    use etas_frontend::{
        ExternalPackageId, ProjectDependencySourcePackage, ProjectExternalPackageInput,
        ProjectInput, SourceInput, SourceKind,
    };
    let package = etas_code::PackageKey::from_canonical_fingerprint(
        etas_code::CodeFingerprint::of_bytes("etas.test.cpu-package", b"workers"),
    );
    let dependency = ExternalPackageId(7);
    let mut input = ProjectInput::single_source(SourceInput::anonymous(
        r#"
module app.main;
import dependency.jobs.burn;
import std.task.{run, map_concurrent, Scheduler};
flow main() -> Array<i32> { return run((scheduler: Scheduler) => { return map_concurrent([7, 3, 9, 1], burn); }); }
"#,
    ));
    input.sources.push(SourceInput { id: SourceId(1), path: None,
        text: "module dependency.jobs; public flow burn(value: i32) -> i32 { var step = 0; while step < 20000 limit Iterations(20000) { step = step + 1; } return value; }".into(),
        kind: SourceKind::DependencySourceOverlay { package: dependency, import_root: "dependency".into() } });
    input
        .environment
        .external_packages
        .push(ProjectExternalPackageInput {
            id: dependency,
            name: "workers".into(),
            version: "1.0.0".into(),
            edition: "2026".into(),
            import_root: "dependency".into(),
            std_compatibility: etas_std::standard_registry()
                .version()
                .compatibility_identity(),
        });
    input
        .environment
        .dependency_source_packages
        .push(ProjectDependencySourcePackage {
            package: dependency,
            import_root: "dependency".into(),
            definition_package: package,
        });
    let output = etas_frontend::Frontend.check_project(input);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.unwrap();
    assert_map_workers(&checked, false).await;
}

#[tokio::test(flavor = "current_thread")]
async fn generic_named_cpu_tasks_keep_the_shared_step_limit() {
    let checked = generic_mapper("[7, 3, 9, 1]", "i32");
    let mut diagnostics = Vec::new();
    for threads in [1, 4] {
        let mut options = RunOptions::default();
        options.cpu_threads = std::num::NonZeroUsize::new(threads).unwrap();
        options.execution_limits.max_steps = std::num::NonZeroU64::new(2000);
        let result = Interpreter
            .run_checked(
                &checked,
                EntryPoint {
                    item: checked.entry.unwrap(),
                },
                vec![],
                &FakeHost::new(HostServiceAvailability::default()),
                options,
            )
            .await
            .unwrap();
        assert!(result.value().is_none());
        assert_eq!(result.diagnostics.len(), 1, "{:?}", result.diagnostics);
        if threads == 4 {
            assert!(
                result
                    .events
                    .iter()
                    .any(|event| matches!(event, WorkflowEvent::TaskDispatched { .. }))
            );
        }
        diagnostics.push(result.diagnostics);
    }
    assert_eq!(diagnostics[0], diagnostics[1]);
}
