use super::super::*;
use etas_host::replay::{
    MemoryReplayKeyProvider, ReplayKey, ReplayKeyError, ReplayKeyId, ReplayKeyProvider,
};
use std::{num::NonZeroUsize, sync::Arc};

const SOURCE: &str = r#"
module app.main;
import std.runtime.checkpoint;
effect Approval { action request(reason: string) -> bool; }
flow ask() -> bool { return perform Approval.request("private-replay-reason"); }
flow main() -> bool {
    let approved = ask();
    checkpoint("saved");
    return approved;
}
"#;

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn checkpoint_key_store_inside_program_filesystem_authority_is_rejected_before_execution() {
    let checked = checked_project(SOURCE);
    let workspace = etas_host::TestWorkspace::create("interpreter-private-replay-keys").unwrap();
    let provider =
        etas_host::replay::DirectoryReplayKeyProvider::open(&workspace.path().join("keys"))
            .unwrap();
    let mut options = RunOptions {
        replay_key_provider: Arc::new(provider),
        ..Default::default()
    };
    options
        .host_context
        .authority
        .sandbox
        .filesystem
        .read_roots
        .push(etas_host::WorkspaceRoot::new(workspace.path()).unwrap());
    let host = FakeHost::new(availability(&[
        HostRequirementKind::Approval,
        HostRequirementKind::Checkpoint,
    ]));
    let rejected = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            vec![],
            &host,
            options,
        )
        .await
        .unwrap();
    assert!(rejected.value().is_none());
    assert!(
        rejected.diagnostics.iter().any(|diagnostic| diagnostic
            .message
            .contains("accessible to program filesystem authority")),
        "{:?}",
        rejected.diagnostics
    );
    assert_eq!(host.approval_call_count(), 0);
    assert!(rejected.checkpoints.is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn checkpoint_is_not_published_when_its_replay_key_cannot_be_retained() {
    let checked = checked_project(SOURCE);
    let provider = Arc::new(MemoryReplayKeyProvider::new(NonZeroUsize::MIN));
    let occupied = provider.create().unwrap();
    provider.retain(&occupied).unwrap();
    let host = FakeHost::new(availability(&[
        HostRequirementKind::Approval,
        HostRequirementKind::Checkpoint,
    ]));
    let rejected = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            vec![],
            &host,
            RunOptions {
                replay_key_provider: provider.clone(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(rejected.value().is_none());
    assert!(
        rejected
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("provider capacity exhausted")),
        "{:?}",
        rejected.diagnostics
    );
    assert_eq!(host.approval_call_count(), 1);
    assert!(rejected.checkpoints.is_empty());
    assert_eq!(provider.resolve(occupied.id()).unwrap().id(), occupied.id());
}

#[tokio::test(flavor = "current_thread")]
async fn checkpoint_replay_requires_original_provider_key_without_redispatching_approval() {
    let checked = checked_project(SOURCE);
    let provider = Arc::new(MemoryReplayKeyProvider::new(NonZeroUsize::MIN));
    let options = RunOptions {
        replay_key_provider: provider.clone(),
        ..Default::default()
    };
    let host = FakeHost::new(availability(&[
        HostRequirementKind::Approval,
        HostRequirementKind::Checkpoint,
    ]));
    let first = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            vec![],
            &host,
            options.clone(),
        )
        .await
        .unwrap();
    assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);
    let mut artifact = crate::api::codec::checkpoint_artifact_json(
        &["main.es".into()],
        "main",
        &first.checkpoints[0],
    )
    .unwrap();
    artifact["code_sources"] = serde_json::json!([]);
    let decoded = crate::api::codec::checkpoint_from_json_with_limits(
        &options.storage_limits,
        &artifact,
        &checked,
    )
    .unwrap();
    let restored = Interpreter
        .resume_checkpoint(&checked, &decoded, &host, options.clone())
        .await
        .unwrap();
    assert!(
        restored.diagnostics.is_empty(),
        "{:?}",
        restored.diagnostics
    );
    assert_eq!(restored.value(), Some(&InterpValue::Bool(true)));
    assert_eq!(host.approval_call_count(), 1);

    let absent = RunOptions {
        replay_key_provider: Arc::new(MemoryReplayKeyProvider::new(NonZeroUsize::MIN)),
        ..options.clone()
    };
    for source in [false, true] {
        let rejected = if source {
            let source =
                crate::api::codec::SourceCheckpoint::from_document(artifact.clone(), &checked)
                    .unwrap();
            Interpreter
                .create_source_resume(&checked, &source, &host, absent.clone())
                .execute()
                .await
                .unwrap()
        } else {
            Interpreter
                .resume_checkpoint(&checked, &decoded, &host, absent.clone())
                .await
                .unwrap()
        };
        assert!(rejected.value().is_none());
        assert!(
            rejected
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("replay key")
                    && diagnostic.message.contains("unavailable")),
            "{:?}",
            rejected.diagnostics
        );
        assert_eq!(host.approval_call_count(), 1);
    }
    provider.remove(decoded.replay_key_id).unwrap();
    let rejected = Interpreter
        .resume_checkpoint(&checked, &decoded, &host, options)
        .await
        .unwrap();
    assert!(rejected.value().is_none());
    assert_eq!(host.approval_call_count(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn checkpoint_codec_rejects_plaintext_replay_keys_and_unbound_key_ids() {
    let checked = checked_project(SOURCE);
    let host = FakeHost::new(availability(&[
        HostRequirementKind::Approval,
        HostRequirementKind::Checkpoint,
    ]));
    let first = Interpreter
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
    let artifact = crate::api::codec::checkpoint_artifact_json(
        &["main.es".into()],
        "main",
        &first.checkpoints[0],
    )
    .unwrap();
    for field in ["key", "replay_key_id"] {
        let mut bad = artifact.clone();
        if field == "key" {
            bad["checkpoint"]["completed_host_boundaries"][0][field] =
                "approval:private-replay-reason".into();
        } else {
            bad["checkpoint"][field] = "../unbound-key".into();
        }
        let error = crate::api::codec::checkpoint_from_json_with_limits(
            &Default::default(),
            &bad,
            &checked,
        )
        .unwrap_err();
        assert!(error.to_string().contains("invalid Replay"), "{error}");
        assert!(!error.to_string().contains("private-replay-reason"));
    }
}

#[tokio::test(flavor = "current_thread")]
async fn model_tool_loop_replays_the_original_invocation_only_when_its_request_matches() {
    let checked = checked_project(
        r#"
module app.main;
import std.agent.prompt.Prompt;
import std.runtime.checkpoint;
agent Writer(input: string) -> string { return perform infer(Prompt.new().user(Public(input))); }
flow main(input: string) -> string {
    checkpoint("before");
    let answer = Writer.run(input);
    checkpoint("after");
    return answer;
}
"#,
    );
    let mut options = RunOptions::default();
    options.host_context.authority.grants = vec![HostActionGrant::allow("Agentic", "infer")];
    options.model_policy.provider_capabilities = Some(full_model_capabilities());
    options.model_policy.tools = vec![ToolSchema {
        tool: ToolRef::anonymous_test("Search"),
        input: HostSchema::Record(vec![]),
        output: Some(HostSchema::String),
    }];
    let services = availability(&[
        HostRequirementKind::Agentic,
        HostRequirementKind::ToolCall,
        HostRequirementKind::Checkpoint,
    ]);
    let first_host = FakeHost::new(services);
    first_host.seed_model_response_tool_call("Search");
    first_host.seed_tool_response_once(HostValue::String("tool result".into()));
    first_host.seed_model_response_text("final model answer");
    let first = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            vec![InterpValue::String("original prompt".into())],
            &first_host,
            options.clone(),
        )
        .await
        .unwrap();
    assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);
    assert_eq!(first_host.model_call_count(), 2);
    assert_eq!(first_host.tool_requests().len(), 1);
    let mut replay = first.checkpoints[0].clone();
    replay.completed_host_boundaries = first.checkpoints[1].completed_host_boundaries.clone();
    let replay_host = FakeHost::new(services);
    let restored = Interpreter
        .resume_checkpoint(&checked, &replay, &replay_host, options.clone())
        .await
        .unwrap();
    assert!(
        restored.diagnostics.is_empty(),
        "{:?}",
        restored.diagnostics
    );
    assert_eq!(
        restored.value(),
        Some(&InterpValue::String("final model answer".into()))
    );
    assert_eq!(replay_host.model_call_count(), 0);
    assert!(replay_host.tool_requests().is_empty());
    replay.model_policy.model = etas_host::ModelName("changed-model".into());
    let rejected = Interpreter
        .resume_checkpoint(&checked, &replay, &replay_host, options)
        .await
        .unwrap();
    assert!(rejected.value().is_none());
    assert!(
        rejected
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("fingerprint does not match")),
        "{:?}",
        rejected.diagnostics
    );
    assert_eq!(
        replay_host.model_call_count(),
        0,
        "mismatched request must not be redispatched"
    );
    assert!(replay_host.tool_requests().is_empty());
}

#[derive(Debug)]
struct WrongKeyProvider;
impl ReplayKeyProvider for WrongKeyProvider {
    fn validate_authority(&self, _: &etas_host::AuthorityContext) -> Result<(), ReplayKeyError> {
        Ok(())
    }
    fn create(&self) -> Result<Arc<ReplayKey>, ReplayKeyError> {
        ReplayKey::generate().map(Arc::new)
    }
    fn retain(&self, _: &Arc<ReplayKey>) -> Result<(), ReplayKeyError> {
        Ok(())
    }
    fn resolve(&self, _: ReplayKeyId) -> Result<Arc<ReplayKey>, ReplayKeyError> {
        self.create()
    }
}

#[tokio::test(flavor = "current_thread")]
async fn checkpoint_replay_rejects_provider_returning_a_different_key_identity() {
    let checked = checked_project(SOURCE);
    let host = FakeHost::new(availability(&[
        HostRequirementKind::Approval,
        HostRequirementKind::Checkpoint,
    ]));
    let first = Interpreter
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
    let rejected = Interpreter
        .resume_checkpoint(
            &checked,
            &first.checkpoints[0],
            &host,
            RunOptions {
                replay_key_provider: Arc::new(WrongKeyProvider),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(rejected.value().is_none());
    assert!(
        rejected
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("different key identity")),
        "{:?}",
        rejected.diagnostics
    );
    assert_eq!(host.approval_call_count(), 1);
}
