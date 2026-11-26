use super::super::*;

#[tokio::test(flavor = "current_thread")]
async fn bounded_for_keeps_its_evaluated_limit_across_checkpoint_resume() {
    let checked = checked_project(
        r#"module app.main;
import std.runtime.checkpoint;
flow bound(value: usize) -> usize ![] { return value; }
flow main() -> i32 {
    var total: i32 = 0;
    @limits([Iterations(bound(2))])
    for value in [10, 20, 30] {
        total = total + value;
        checkpoint("bounded-for");
    }
    return total;
}"#,
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
    assert_eq!(result.value(), Some(&InterpValue::i32(30)));
    assert_eq!(result.checkpoints.len(), 2);
    for checkpoint in &result.checkpoints {
        let artifact =
            api::codec::checkpoint_artifact_json(&["main.es".into()], "main", checkpoint).unwrap();
        let decoded = api::codec::checkpoint_from_json(&artifact, &checked).unwrap();
        let resumed = Interpreter
            .resume_checkpoint(&checked, &decoded, &host, RunOptions::default())
            .await
            .unwrap();
        assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
        assert_eq!(resumed.value(), Some(&InterpValue::i32(30)));
    }
}

#[tokio::test(flavor = "current_thread")]
async fn record_update_snapshots_base_before_evaluating_replacements() {
    let checked = checked_project(
        r#"module app.main;
type Paper = { title: string, year: i32 };
flow main() -> i32 ![] {
    var paper = Paper { title = "draft", year = 2025 };
    let revised = paper { title = {
        paper = Paper { title = "changed", year = 2000 };
        "published"
    } };
    if revised.title == "published" && paper.title == "changed" {
        return revised.year - paper.year;
    }
    return -1;
}"#,
    );
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            vec![],
            &FakeHost::new(availability(&[])),
            RunOptions::default(),
        )
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&InterpValue::i32(25)));
}

#[tokio::test(flavor = "current_thread")]
async fn returned_handler_keeps_immutable_repository_capture_for_bound_method() {
    let checked = checked_project(
        r#"module app.main;
spec RecentProjects { flow remember(self, workspace: i32) -> i32 ![]; }
type NativeRecentProjects;
impl NativeRecentProjects ~ RecentProjects {
    flow remember(self, workspace: i32) -> i32 ![] { return workspace + 1; }
}
effect ProjectOperations { action remember(workspace: i32) -> i32; }
flow remember<T ~ RecentProjects>(recent: T, workspace: i32) -> i32 ![] {
    return recent.remember(workspace);
}
flow adapter() -> ![ProjectOperations => []] {
    let recent = NativeRecentProjects {};
    return handler {
        ProjectOperations.remember(workspace) => { resume remember(recent, workspace); }
    };
}
flow main() -> i32 ![] {
    return handle perform ProjectOperations.remember(41) with adapter();
}"#,
    );
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            vec![],
            &FakeHost::new(availability(&[])),
            RunOptions::default(),
        )
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&InterpValue::i32(42)));
}

#[tokio::test(flavor = "current_thread")]
async fn named_providers_select_each_field_before_the_type_fallback() {
    let mut input = etas_frontend::ProjectInput::single_source(
        etas_frontend::SourceInput::anonymous(
            r#"module std.effects;
public effect Inject<T> { action get(name: string) -> T; }
type Config = { @inject title: string, @inject author: string, @inject footer: string, count: i32 };
flow main() -> i32 ![] {
    let author = "Ada";
    let config = handle Config { count = 40 } with {
        string => "fallback";
        title: string => "Named";
        author,
    };
    let revised = config { count = config.count + 2 };
    if revised.title == "Named" && revised.author == "Ada" && revised.footer == "fallback" && config.count == 40 {
        return revised.count;
    }
    return -1;
}"#,
        ),
    );
    for (index, text) in [
        "module std.runtime.budget; public type Money = { private amount: f64, private currency: string };",
        "module std.runtime.time; public type Duration = { millis: u128 };",
    ].into_iter().enumerate() {
        let mut source = etas_frontend::SourceInput::anonymous(text);
        source.id = etas_core::SourceId(index as u32 + 1);
        input.sources.push(source);
    }
    let output = etas_frontend::Frontend.check_standard_library_source(input);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.unwrap();
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            vec![],
            &FakeHost::new(availability(&[])),
            RunOptions::default(),
        )
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&InterpValue::i32(42)));
}
