use super::super::*;

#[tokio::test(flavor = "current_thread")]
async fn resume_checkpoint_replays_memory_selection_prompt_data_after_version_validation() {
    let checked = checked_project(
        r#"
module app.main;
import std.agent.prompt.Prompt;
import std.runtime.{checkpoint};

alias ProjectMemorySchema = MemoryRegion<{
  Papers: Store<string, string>
}>;

let ProjectMemory =
  std.memory.region<ProjectMemorySchema>(
    stable_id = "project_memory",
    store = "project-main"
  );

flow main() -> Prompt {
  checkpoint("before-context");
  let selected = ProjectMemory.Papers.query("paper").limit(Tokens(2));
  let prompt = Prompt.new().data(selected);
  checkpoint("after-context");
  return prompt;
}
"#,
    );

    let first_host = FakeHost::new(availability(&[
        HostRequirementKind::DurableMemory,
        HostRequirementKind::Checkpoint,
    ]));
    for (key, value) in [("paper-1", "first"), ("paper-2", "second")] {
        first_host.seed_memory(
            "project_memory",
            &["Papers"],
            HostValue::String(key.to_owned()),
            HostValue::String(value.to_owned()),
        );
    }
    let first = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &first_host,
            RunOptions::default(),
        )
        .await;

    assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);
    assert_eq!(first_host.memory_call_count(), 1);
    assert_eq!(first.checkpoints.len(), 2);
    assert_eq!(
        first.checkpoints[1].resource_versions.versions,
        vec![
            crate::orchestration::ResourceVersionRecord {
                resource: "memory:project_memory:Papers:\"paper-1\"".to_owned(),
                version: "v1".to_owned(),
            },
            crate::orchestration::ResourceVersionRecord {
                resource: "memory:project_memory:Papers:\"paper-2\"".to_owned(),
                version: "v1".to_owned(),
            },
        ]
    );

    let mut replay_checkpoint = first.checkpoints[0].clone();
    replay_checkpoint.completed_host_boundaries =
        first.checkpoints[1].completed_host_boundaries.clone();
    replay_checkpoint.resource_versions = first.checkpoints[1].resource_versions.clone();

    let replay_host = FakeHost::new(availability(&[
        HostRequirementKind::DurableMemory,
        HostRequirementKind::Checkpoint,
    ]));
    for (key, value) in [("paper-1", "first"), ("paper-2", "second")] {
        replay_host.seed_memory(
            "project_memory",
            &["Papers"],
            HostValue::String(key.to_owned()),
            HostValue::String(value.to_owned()),
        );
    }
    let resumed = Interpreter
        .resume_checkpoint(
            &checked,
            &replay_checkpoint,
            &replay_host,
            RunOptions::default(),
        )
        .await;

    assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
    let Some(value::InterpValue::Prompt(messages)) = resumed.value else {
        panic!("expected prompt value, got {:?}", resumed.value);
    };
    assert_eq!(messages.len(), 1);
    assert!(messages[0].text.contains(r#""key":"paper-1""#));
    assert!(messages[0].text.contains(r#""value":"first""#));
    assert_eq!(
        replay_host.memory_call_count(),
        1,
        "resume should call memory only for version validation"
    );
    assert!(
        !resumed
            .events
            .iter()
            .any(|event| matches!(event, WorkflowEvent::HostRequestSent(_)))
    );
    assert!(
        !resumed
            .events
            .iter()
            .any(|event| matches!(event, WorkflowEvent::HostResponseReceived(_)))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn resume_checkpoint_rejects_stale_memory_selection_prompt_data_replay() {
    let checked = checked_project(
        r#"
module app.main;
import std.agent.prompt.Prompt;
import std.runtime.{checkpoint};

alias ProjectMemorySchema = MemoryRegion<{
  Papers: Store<string, string>
}>;

let ProjectMemory =
  std.memory.region<ProjectMemorySchema>(
    stable_id = "project_memory",
    store = "project-main"
  );

flow main() -> Prompt {
  checkpoint("before-context");
  let selected = ProjectMemory.Papers.query("paper").limit(Tokens(3));
  let prompt = Prompt.new().data(selected);
  checkpoint("after-context");
  return prompt;
}
"#,
    );

    let first_host = FakeHost::new(availability(&[
        HostRequirementKind::DurableMemory,
        HostRequirementKind::Checkpoint,
    ]));
    first_host.seed_memory(
        "project_memory",
        &["Papers"],
        HostValue::String("paper-1".to_owned()),
        HostValue::String("first".to_owned()),
    );
    let first = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &first_host,
            RunOptions::default(),
        )
        .await;

    assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);
    let mut replay_checkpoint = first.checkpoints[0].clone();
    replay_checkpoint.completed_host_boundaries =
        first.checkpoints[1].completed_host_boundaries.clone();
    replay_checkpoint.resource_versions = first.checkpoints[1].resource_versions.clone();

    let replay_host = FakeHost::new(availability(&[
        HostRequirementKind::DurableMemory,
        HostRequirementKind::Checkpoint,
    ]));
    replay_host.seed_memory(
        "project_memory",
        &["Papers"],
        HostValue::String("paper-1".to_owned()),
        HostValue::String("first".to_owned()),
    );
    replay_host.seed_memory(
        "project_memory",
        &["Papers"],
        HostValue::String("paper-2".to_owned()),
        HostValue::String("second".to_owned()),
    );
    let resumed = Interpreter
        .resume_checkpoint(
            &checked,
            &replay_checkpoint,
            &replay_host,
            RunOptions::default(),
        )
        .await;

    assert!(resumed.value.is_none());
    assert!(
        resumed
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("memory scan replay mismatch")),
        "{:?}",
        resumed.diagnostics
    );
    assert_eq!(
        replay_host.memory_call_count(),
        1,
        "resume should inspect memory once for stale replay validation"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn resume_checkpoint_reuses_completed_memory_get_boundary_result() {
    let checked = checked_project(
        r#"
module app.main;
import std.runtime.{checkpoint};

alias ProjectMemorySchema = MemoryRegion<{
  Papers: Store<string, string>
}>;

let ProjectMemory =
  std.memory.region<ProjectMemorySchema>(
    stable_id = "project_memory",
    store = "project-main"
  );

flow main() -> Option<string> {
  checkpoint("before-read");
  let paper = ProjectMemory.Papers.get("paper-1");
  checkpoint("after-read");
  return paper;
}
"#,
    );

    let first_host = FakeHost::new(availability(&[
        HostRequirementKind::DurableMemory,
        HostRequirementKind::Checkpoint,
    ]));
    first_host.seed_memory(
        "project_memory",
        &["Papers"],
        HostValue::String("paper-1".to_owned()),
        HostValue::String("draft".to_owned()),
    );
    let first = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &first_host,
            RunOptions::default(),
        )
        .await;

    assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);
    assert_eq!(first_host.memory_call_count(), 1);
    assert_eq!(first.checkpoints.len(), 2);
    assert_eq!(
        first.checkpoints[1].resource_versions.versions,
        vec![crate::orchestration::ResourceVersionRecord {
            resource: "memory:project_memory:Papers:\"paper-1\"".to_owned(),
            version: "v1".to_owned(),
        }]
    );

    let mut replay_checkpoint = first.checkpoints[0].clone();
    replay_checkpoint.completed_host_boundaries =
        first.checkpoints[1].completed_host_boundaries.clone();
    replay_checkpoint.resource_versions = first.checkpoints[1].resource_versions.clone();

    let replay_host = FakeHost::new(availability(&[
        HostRequirementKind::DurableMemory,
        HostRequirementKind::Checkpoint,
    ]));
    replay_host.seed_memory(
        "project_memory",
        &["Papers"],
        HostValue::String("paper-1".to_owned()),
        HostValue::String("draft".to_owned()),
    );
    let resumed = Interpreter
        .resume_checkpoint(
            &checked,
            &replay_checkpoint,
            &replay_host,
            RunOptions::default(),
        )
        .await;

    assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
    assert_eq!(
        resumed.value,
        Some(value::InterpValue::OptionSome(Box::new(
            value::InterpValue::String("draft".to_owned()),
        )))
    );
    assert_eq!(replay_host.memory_call_count(), 1);
    assert!(
        !resumed
            .events
            .iter()
            .any(|event| matches!(event, WorkflowEvent::HostRequestSent(_)))
    );
    assert!(
        !resumed
            .events
            .iter()
            .any(|event| matches!(event, WorkflowEvent::HostResponseReceived(_)))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn resume_checkpoint_rejects_stale_memory_get_replay() {
    let checked = checked_project(
        r#"
module app.main;
import std.runtime.{checkpoint};

alias ProjectMemorySchema = MemoryRegion<{
  Papers: Store<string, string>
}>;

let ProjectMemory =
  std.memory.region<ProjectMemorySchema>(
    stable_id = "project_memory",
    store = "project-main"
  );

flow main() -> Option<string> {
  checkpoint("before-read");
  let paper = ProjectMemory.Papers.get("paper-1");
  checkpoint("after-read");
  return paper;
}
"#,
    );

    let first_host = FakeHost::new(availability(&[
        HostRequirementKind::DurableMemory,
        HostRequirementKind::Checkpoint,
    ]));
    first_host.seed_memory(
        "project_memory",
        &["Papers"],
        HostValue::String("paper-1".to_owned()),
        HostValue::String("draft".to_owned()),
    );
    let first = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &first_host,
            RunOptions::default(),
        )
        .await;

    assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);
    let mut replay_checkpoint = first.checkpoints[0].clone();
    replay_checkpoint.completed_host_boundaries =
        first.checkpoints[1].completed_host_boundaries.clone();
    replay_checkpoint.resource_versions = first.checkpoints[1].resource_versions.clone();

    let replay_host = FakeHost::new(availability(&[
        HostRequirementKind::DurableMemory,
        HostRequirementKind::Checkpoint,
    ]));
    let resumed = Interpreter
        .resume_checkpoint(
            &checked,
            &replay_checkpoint,
            &replay_host,
            RunOptions::default(),
        )
        .await;

    assert!(resumed.value.is_none());
    assert!(
        resumed
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("memory version mismatch")),
        "{:?}",
        resumed.diagnostics
    );
    assert_eq!(replay_host.memory_call_count(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn resume_checkpoint_replays_absent_memory_get_after_absence_validation() {
    let checked = checked_project(
        r#"
module app.main;
import std.runtime.{checkpoint};

alias ProjectMemorySchema = MemoryRegion<{
  Papers: Store<string, string>
}>;

let ProjectMemory =
  std.memory.region<ProjectMemorySchema>(
    stable_id = "project_memory",
    store = "project-main"
  );

flow main() -> Option<string> {
  checkpoint("before-read");
  let paper = ProjectMemory.Papers.get("missing-paper");
  checkpoint("after-read");
  return paper;
}
"#,
    );

    let first_host = FakeHost::new(availability(&[
        HostRequirementKind::DurableMemory,
        HostRequirementKind::Checkpoint,
    ]));
    let first = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &first_host,
            RunOptions::default(),
        )
        .await;

    assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);
    assert_eq!(first_host.memory_call_count(), 1);
    assert!(first.checkpoints[1].resource_versions.versions.is_empty());

    let mut replay_checkpoint = first.checkpoints[0].clone();
    replay_checkpoint.completed_host_boundaries =
        first.checkpoints[1].completed_host_boundaries.clone();
    replay_checkpoint.resource_versions = first.checkpoints[1].resource_versions.clone();

    let replay_host = FakeHost::new(availability(&[
        HostRequirementKind::DurableMemory,
        HostRequirementKind::Checkpoint,
    ]));
    let resumed = Interpreter
        .resume_checkpoint(
            &checked,
            &replay_checkpoint,
            &replay_host,
            RunOptions::default(),
        )
        .await;

    assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
    assert_eq!(resumed.value, Some(value::InterpValue::OptionNone));
    assert_eq!(replay_host.memory_call_count(), 1);
    assert!(
        !resumed
            .events
            .iter()
            .any(|event| matches!(event, WorkflowEvent::HostRequestSent(_)))
    );
    assert!(
        !resumed
            .events
            .iter()
            .any(|event| matches!(event, WorkflowEvent::HostResponseReceived(_)))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn resume_checkpoint_rejects_absent_memory_get_when_key_appears() {
    let checked = checked_project(
        r#"
module app.main;
import std.runtime.{checkpoint};

alias ProjectMemorySchema = MemoryRegion<{
  Papers: Store<string, string>
}>;

let ProjectMemory =
  std.memory.region<ProjectMemorySchema>(
    stable_id = "project_memory",
    store = "project-main"
  );

flow main() -> Option<string> {
  checkpoint("before-read");
  let paper = ProjectMemory.Papers.get("missing-paper");
  checkpoint("after-read");
  return paper;
}
"#,
    );

    let first_host = FakeHost::new(availability(&[
        HostRequirementKind::DurableMemory,
        HostRequirementKind::Checkpoint,
    ]));
    let first = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &first_host,
            RunOptions::default(),
        )
        .await;

    assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);
    let mut replay_checkpoint = first.checkpoints[0].clone();
    replay_checkpoint.completed_host_boundaries =
        first.checkpoints[1].completed_host_boundaries.clone();
    replay_checkpoint.resource_versions = first.checkpoints[1].resource_versions.clone();

    let replay_host = FakeHost::new(availability(&[
        HostRequirementKind::DurableMemory,
        HostRequirementKind::Checkpoint,
    ]));
    replay_host.seed_memory(
        "project_memory",
        &["Papers"],
        HostValue::String("missing-paper".to_owned()),
        HostValue::String("new-draft".to_owned()),
    );
    let resumed = Interpreter
        .resume_checkpoint(
            &checked,
            &replay_checkpoint,
            &replay_host,
            RunOptions::default(),
        )
        .await;

    assert!(resumed.value.is_none());
    assert!(
        resumed.diagnostics.iter().any(|diagnostic| diagnostic
            .message
            .contains("memory absence replay mismatch")),
        "{:?}",
        resumed.diagnostics
    );
    assert_eq!(replay_host.memory_call_count(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn resume_checkpoint_replays_memory_scan_after_version_validation() {
    let checked = checked_project(
        r#"
module app.main;
import std.runtime.{checkpoint};

alias ProjectMemorySchema = MemoryRegion<{
  Papers: Store<string, string>
}>;

let ProjectMemory =
  std.memory.region<ProjectMemorySchema>(
    stable_id = "project_memory",
    store = "project-main"
  );

flow main() -> List<string> {
  checkpoint("before-keys");
  let keys = ProjectMemory.Papers.keys();
  checkpoint("after-keys");
  return keys;
}
"#,
    );

    let first_host = FakeHost::new(availability(&[
        HostRequirementKind::DurableMemory,
        HostRequirementKind::Checkpoint,
    ]));
    first_host.seed_memory(
        "project_memory",
        &["Papers"],
        HostValue::String("paper-1".to_owned()),
        HostValue::String("draft-1".to_owned()),
    );
    first_host.seed_memory(
        "project_memory",
        &["Papers"],
        HostValue::String("paper-2".to_owned()),
        HostValue::String("draft-2".to_owned()),
    );
    let first = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &first_host,
            RunOptions::default(),
        )
        .await;

    assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);
    assert_eq!(first_host.memory_call_count(), 1);
    assert_eq!(first.checkpoints.len(), 2);
    assert_eq!(
        first.checkpoints[1].resource_versions.versions,
        vec![
            crate::orchestration::ResourceVersionRecord {
                resource: "memory:project_memory:Papers:\"paper-1\"".to_owned(),
                version: "v1".to_owned(),
            },
            crate::orchestration::ResourceVersionRecord {
                resource: "memory:project_memory:Papers:\"paper-2\"".to_owned(),
                version: "v1".to_owned(),
            },
        ]
    );

    let mut replay_checkpoint = first.checkpoints[0].clone();
    replay_checkpoint.completed_host_boundaries =
        first.checkpoints[1].completed_host_boundaries.clone();
    replay_checkpoint.resource_versions = first.checkpoints[1].resource_versions.clone();

    let replay_host = FakeHost::new(availability(&[
        HostRequirementKind::DurableMemory,
        HostRequirementKind::Checkpoint,
    ]));
    replay_host.seed_memory(
        "project_memory",
        &["Papers"],
        HostValue::String("paper-1".to_owned()),
        HostValue::String("draft-1".to_owned()),
    );
    replay_host.seed_memory(
        "project_memory",
        &["Papers"],
        HostValue::String("paper-2".to_owned()),
        HostValue::String("draft-2".to_owned()),
    );
    let resumed = Interpreter
        .resume_checkpoint(
            &checked,
            &replay_checkpoint,
            &replay_host,
            RunOptions::default(),
        )
        .await;

    assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
    assert_eq!(
        resumed.value,
        Some(value::InterpValue::List(
            vec![
                value::InterpValue::String("paper-1".to_owned()),
                value::InterpValue::String("paper-2".to_owned()),
            ]
            .into()
        ))
    );
    assert_eq!(replay_host.memory_call_count(), 1);
    assert!(
        !resumed
            .events
            .iter()
            .any(|event| matches!(event, WorkflowEvent::HostRequestSent(_)))
    );
    assert!(
        !resumed
            .events
            .iter()
            .any(|event| matches!(event, WorkflowEvent::HostResponseReceived(_)))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn resume_checkpoint_rejects_stale_memory_scan_replay() {
    let checked = checked_project(
        r#"
module app.main;
import std.runtime.{checkpoint};

alias ProjectMemorySchema = MemoryRegion<{
  Papers: Store<string, string>
}>;

let ProjectMemory =
  std.memory.region<ProjectMemorySchema>(
    stable_id = "project_memory",
    store = "project-main"
  );

flow main() -> List<string> {
  checkpoint("before-keys");
  let keys = ProjectMemory.Papers.keys();
  checkpoint("after-keys");
  return keys;
}
"#,
    );

    let first_host = FakeHost::new(availability(&[
        HostRequirementKind::DurableMemory,
        HostRequirementKind::Checkpoint,
    ]));
    first_host.seed_memory(
        "project_memory",
        &["Papers"],
        HostValue::String("paper-1".to_owned()),
        HostValue::String("draft-1".to_owned()),
    );
    let first = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &first_host,
            RunOptions::default(),
        )
        .await;

    assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);
    let mut replay_checkpoint = first.checkpoints[0].clone();
    replay_checkpoint.completed_host_boundaries =
        first.checkpoints[1].completed_host_boundaries.clone();
    replay_checkpoint.resource_versions = first.checkpoints[1].resource_versions.clone();

    let replay_host = FakeHost::new(availability(&[
        HostRequirementKind::DurableMemory,
        HostRequirementKind::Checkpoint,
    ]));
    replay_host.seed_memory(
        "project_memory",
        &["Papers"],
        HostValue::String("paper-1".to_owned()),
        HostValue::String("draft-1".to_owned()),
    );
    replay_host.seed_memory(
        "project_memory",
        &["Papers"],
        HostValue::String("paper-2".to_owned()),
        HostValue::String("draft-2".to_owned()),
    );
    let resumed = Interpreter
        .resume_checkpoint(
            &checked,
            &replay_checkpoint,
            &replay_host,
            RunOptions::default(),
        )
        .await;

    assert!(resumed.value.is_none());
    assert!(
        resumed
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("memory scan replay mismatch")),
        "{:?}",
        resumed.diagnostics
    );
    assert_eq!(replay_host.memory_call_count(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn resume_checkpoint_replays_memory_put_through_host_again() {
    let checked = checked_project(
        r#"
module app.main;
import std.runtime.{checkpoint};

alias ProjectMemorySchema = MemoryRegion<{
  Papers: Store<string, string>
}>;

let ProjectMemory =
  std.memory.region<ProjectMemorySchema>(
    stable_id = "project_memory",
    store = "project-main"
  );

flow main() -> unit {
  checkpoint("before-write");
  ProjectMemory.Papers.put("paper-1", "draft");
  checkpoint("after-write");
  return;
}
"#,
    );

    let first_host = FakeHost::new(availability(&[
        HostRequirementKind::DurableMemory,
        HostRequirementKind::Checkpoint,
    ]));
    let first = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &first_host,
            RunOptions::default(),
        )
        .await;

    assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);
    assert_eq!(first_host.memory_call_count(), 1);
    assert_eq!(first.checkpoints.len(), 2);
    assert_eq!(
        first.checkpoints[1].resource_versions.versions,
        vec![crate::orchestration::ResourceVersionRecord {
            resource: "memory:project_memory:Papers:\"paper-1\"".to_owned(),
            version: "v1".to_owned(),
        }]
    );

    let mut replay_checkpoint = first.checkpoints[0].clone();
    replay_checkpoint.completed_host_boundaries =
        first.checkpoints[1].completed_host_boundaries.clone();

    let replay_host = FakeHost::new(availability(&[
        HostRequirementKind::DurableMemory,
        HostRequirementKind::Checkpoint,
    ]));
    let resumed = Interpreter
        .resume_checkpoint(
            &checked,
            &replay_checkpoint,
            &replay_host,
            RunOptions::default(),
        )
        .await;

    assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
    assert_eq!(resumed.value, Some(value::InterpValue::Unit));
    assert_eq!(replay_host.memory_call_count(), 1);
    assert_eq!(
        replay_host.memory_value(
            "project_memory",
            &["Papers"],
            &HostValue::String("paper-1".to_owned()),
        ),
        Some(HostValue::String("draft".to_owned()))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn resume_checkpoint_preserves_memory_selection_limit_value() {
    let checked = checked_project(
        r#"
module app.main;
import std.runtime.{checkpoint};

alias ProjectMemorySchema = MemoryRegion<{
  Papers: Store<string, string>
}>;

let ProjectMemory =
  std.memory.region<ProjectMemorySchema>(
    stable_id = "project_memory",
    store = "project-main"
  );

flow main() -> MemorySelection<string> {
  checkpoint("before-query");
  let selected = ProjectMemory.Papers.query("paper");
  let papers = selected.limit(Tokens(2));
  checkpoint("after-query");
  return papers;
}
"#,
    );

    let first_host = FakeHost::new(availability(&[
        HostRequirementKind::DurableMemory,
        HostRequirementKind::Checkpoint,
    ]));
    let first = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &first_host,
            RunOptions::default(),
        )
        .await;

    assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);
    assert_eq!(first_host.memory_call_count(), 0);
    assert_eq!(first.checkpoints.len(), 2);

    let mut replay_checkpoint = first.checkpoints[0].clone();
    replay_checkpoint.completed_host_boundaries =
        first.checkpoints[1].completed_host_boundaries.clone();

    let replay_host = FakeHost::new(availability(&[
        HostRequirementKind::DurableMemory,
        HostRequirementKind::Checkpoint,
    ]));
    let resumed = Interpreter
        .resume_checkpoint(
            &checked,
            &replay_checkpoint,
            &replay_host,
            RunOptions::default(),
        )
        .await;

    assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
    let (region_stable_id, path, kind, predicate, limit) = match resumed.value {
        Some(value::InterpValue::MemorySelection {
            region_stable_id,
            path,
            kind,
            predicate,
            limit,
            ..
        }) => (region_stable_id, path, kind, predicate, limit),
        other => panic!("expected memory selection value, got {other:?}"),
    };
    assert_eq!(region_stable_id, "project_memory");
    assert_eq!(path, vec!["Papers".to_owned()]);
    assert_eq!(kind, value::MemorySelectionKind::Query);
    assert_eq!(limit, Some(2));
    assert_eq!(
        predicate.as_deref(),
        Some(&value::InterpValue::String("paper".to_owned()))
    );
    assert_eq!(replay_host.memory_call_count(), 0);
    assert!(
        !resumed
            .events
            .iter()
            .any(|event| matches!(event, WorkflowEvent::HostRequestSent(_)))
    );
    assert!(
        !resumed
            .events
            .iter()
            .any(|event| matches!(event, WorkflowEvent::HostResponseReceived(_)))
    );
}
