use super::super::*;
use crate::testing::host::SessionContextFault;

#[tokio::test(flavor = "current_thread")]
async fn publication_history_and_conversation_share_context_with_configured_limits() {
    let checked = checked_project(
        r#"
module app.main;
import std.agent.session.{history_page, prepare_context, publish_context};
import std.runtime.checkpoint;
flow main(ticket: SessionId, text: string) -> bool {
    let config = SessionConfig { id = ticket, context = SummaryPlusRecent(8), retention = Days(90) };
    let initial = Conversation.load(config);
    let before = history_page(config, None(), 1);
    let content = SessionContextContent { text = text, provenance = {"producer" => "app-v2"} };
    let operation = prepare_context(config, before.fence, content);
    let outcome = publish_context(config, before.fence, content, operation);
    let page = history_page(config, None(), 1);
    let conversation = Conversation.load(config);
    checkpoint("published-view");
    return match outcome {
        WriteOutcome.Committed(receipt) => match page.published_context {
            Some(published) => match conversation.summary {
                Some(selected) => selected.content.text == content.text
                    && selected.content.provenance == published.content.provenance
                    && selected.fence == published.fence
                    && selected.version == published.version
                    && selected.version == receipt.context_version,
                _ => false,
            },
            _ => false,
        },
        _ => false,
    };
}
"#,
    );
    for sqlite in [false, true] {
        for wide in [false, true] {
            let workspace =
                etas_host::TestWorkspace::create("publication-selected-context").unwrap();
            let mut limits = etas_host::StorageLimits {
                max_receipt_retention_seconds: 3600,
                ..Default::default()
            };
            let text = if wide {
                limits.max_value_bytes *= 2;
                "x".repeat(etas_host::StorageLimits::default().max_value_bytes + 1)
            } else {
                "application summary".to_owned()
            };
            let mut host = FakeHost::new(availability(&[
                HostRequirementKind::DurableMemory,
                HostRequirementKind::Checkpoint,
            ]));
            host.session = etas_host::InMemorySessionClient::with_limits(limits.clone()).unwrap();
            if sqlite {
                host.persistent_session = Some(
                    etas_host::SqliteSessionClient::open_with_limits(
                        workspace.path().join("db"),
                        limits.clone(),
                    )
                    .unwrap(),
                );
            }
            let options = RunOptions {
                storage_limits: limits,
                ..Default::default()
            };
            let result = Interpreter
                .run_checked(
                    &checked,
                    EntryPoint {
                        item: checked.entry.unwrap(),
                    },
                    vec![
                        InterpValue::String("selected-context".into()),
                        InterpValue::String(text),
                    ],
                    &host,
                    options.clone(),
                )
                .await
                .unwrap();
            assert!(
                result.diagnostics.is_empty(),
                "{sqlite}: {:?}",
                result.diagnostics
            );
            assert_eq!(result.value(), Some(&InterpValue::Bool(true)), "{sqlite}");
            assert!(result.events.iter().any(|event| matches!(
                event,
                WorkflowEvent::SessionHistoryLoaded {
                    has_summary: true,
                    ..
                }
            )));
            let artifact = crate::api::codec::checkpoint_artifact_json(
                &["publication.es".into()],
                "main",
                &result.checkpoints[0],
            )
            .unwrap();
            let restored = crate::api::codec::checkpoint_from_json_with_limits(
                &options.storage_limits,
                &artifact,
                &checked,
            )
            .unwrap();
            let mut old = artifact.clone();
            old["schema"] = "etas.cli.interpreter-checkpoint.v28".into();
            assert!(
                crate::api::codec::checkpoint_from_json(&old, &checked)
                    .unwrap_err()
                    .message()
                    .contains("unsupported checkpoint artifact schema")
            );
            if wide {
                assert!(crate::api::codec::checkpoint_from_json(&artifact, &checked).is_err());
                let restricted = Interpreter
                    .resume_checkpoint(&checked, &restored, &host, RunOptions::default())
                    .await
                    .unwrap();
                assert!(
                    !restricted.diagnostics.is_empty(),
                    "current restore limits must apply"
                );
            }
            let resumed = Interpreter
                .resume_checkpoint(&checked, &restored, &host, options)
                .await
                .unwrap();
            assert!(
                resumed.diagnostics.is_empty(),
                "{sqlite}: {:?}",
                resumed.diagnostics
            );
            assert_eq!(resumed.value(), Some(&InterpValue::Bool(true)));
        }
    }
}

#[derive(Debug, Default)]
struct StopAtCheckpoint(std::sync::Mutex<Option<crate::api::RunControl>>);

#[tokio::test(flavor = "current_thread")]
async fn publication_backend_rejects_mismatched_receipt_contract() {
    let checked = program(
        r#"
        let outcome = publish_context(config, page.fence, content, operation);
        let after = history_page(config, None(), 1);
        return match outcome { WriteOutcome.NotCommitted(_, _) => after.published_context == None(), _ => false };
    "#,
    );
    for sqlite in [false, true] {
        let workspace = etas_host::TestWorkspace::create("publication-contract-mismatch").unwrap();
        let limits = etas_host::StorageLimits {
            max_receipt_retention_seconds: 3600,
            ..Default::default()
        };
        let mut host = FakeHost::new(availability(&[HostRequirementKind::DurableMemory]));
        host.session = etas_host::InMemorySessionClient::with_limits(limits.clone()).unwrap();
        if sqlite {
            host.persistent_session = Some(
                etas_host::SqliteSessionClient::open_with_limits(
                    workspace.path().join("db"),
                    limits,
                )
                .unwrap(),
            );
        }
        super::session_history::seed(&host, 2).await;
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
        assert!(
            result.diagnostics.is_empty(),
            "{sqlite}: {:?}",
            result.diagnostics
        );
        assert_eq!(result.value(), Some(&InterpValue::Bool(true)));
    }
}

#[tokio::test(flavor = "current_thread")]
async fn session_pages_use_configured_runtime_limits() {
    let checked = checked_project(
        r#"
module app.main;
import std.agent.session.history_page;
flow main() -> usize {
    let config = SessionConfig.continue_or_new("pages");
    let page = history_page(config, None(), 1001);
    return page.messages.len();
}
"#,
    );
    for sqlite in [false, true] {
        let workspace = etas_host::TestWorkspace::create("session-configured-page").unwrap();
        let limits = etas_host::StorageLimits {
            max_page_entries: 1500,
            ..Default::default()
        };
        let mut host = FakeHost::new(availability(&[HostRequirementKind::DurableMemory]));
        host.session = etas_host::InMemorySessionClient::with_limits(limits.clone()).unwrap();
        if sqlite {
            host.persistent_session = Some(
                etas_host::SqliteSessionClient::open_with_limits(
                    workspace.path().join("db"),
                    limits.clone(),
                )
                .unwrap(),
            );
        }
        super::session_history::seed(&host, 2).await;
        let result = Interpreter
            .run_checked(
                &checked,
                EntryPoint {
                    item: checked.entry.unwrap(),
                },
                vec![],
                &host,
                RunOptions {
                    storage_limits: limits,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert!(
            result.diagnostics.is_empty(),
            "{sqlite}: {:?}",
            result.diagnostics
        );
        assert_eq!(result.value(), Some(&InterpValue::usize(2)));
        let calls = host.session_call_count();
        let rejected = Interpreter
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
        assert!(!rejected.diagnostics.is_empty());
        assert_eq!(
            host.session_call_count(),
            calls,
            "rejected page must not dispatch"
        );
    }
}
impl StopAtCheckpoint {
    fn arm(&self, control: crate::api::RunControl) {
        *self.0.lock().unwrap() = Some(control);
    }
}
impl crate::api::RunEventObserver for StopAtCheckpoint {
    fn observe(&self, event: &WorkflowEvent) {
        if matches!(event, WorkflowEvent::CheckpointCreated(_)) {
            self.0
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .stop(etas_host::execution::CancellationReason::Requested)
                .unwrap();
        }
    }
}

fn program(body: &str) -> etas_frontend::CheckedProject {
    checked_project(&format!(
        r#"
module app.main;
import std.agent.session.{{history_page, prepare_context, publish_context, reconcile_context}};
import std.runtime.checkpoint;
flow main() -> bool {{
    let config = SessionConfig.continue_or_new("pages");
    let page = history_page(config, None(), 1);
    let content = SessionContextContent {{ text = "caller-produced context", provenance = {{"producer" => "application", "tokenizer" => "application-v1"}} }};
    let operation = prepare_context(config, page.fence, content);
    {body}
}}
"#
    ))
}

#[tokio::test(flavor = "current_thread")]
async fn session_publication_source_replay_reconcile_and_checkpoint_preserve_receipt() {
    let checked = program(
        r#"
    checkpoint("prepared-publication");
    let first = publish_context(config, page.fence, content, operation);
    let repeated = publish_context(config, page.fence, content, operation);
    let found = reconcile_context(config, operation);
    let after = history_page(config, None(), 1);
    checkpoint("published");
    let same = match first {
        WriteOutcome.Committed(original) => match repeated {
            WriteOutcome.Committed(receipt) => original.operation.key == operation.key
                && receipt.context_version == original.context_version && receipt.generation == original.generation,
            _ => false,
        },
        _ => false,
    };
    let recovered = match found {
        ReconcileResult.Found(ConfirmedOutcome.Committed(receipt)) => receipt.operation.key == operation.key,
        _ => false,
    };
    let visible = match after.published_context {Some(context) => context.content.text == content.text, _ => false};
    return same && recovered && visible;
    "#,
    );
    for sqlite in [false, true] {
        let workspace = etas_host::TestWorkspace::create("source-context-publication").unwrap();
        let mut host = FakeHost::new(availability(&[
            HostRequirementKind::DurableMemory,
            HostRequirementKind::Checkpoint,
        ]));
        if sqlite {
            host.persistent_session =
                Some(etas_host::SqliteSessionClient::open(workspace.path().join("db")).unwrap());
        }
        super::session_history::seed(&host, 2).await;
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
        assert!(
            result.diagnostics.is_empty(),
            "{sqlite}: {:?}",
            result.diagnostics
        );
        assert_eq!(result.value(), Some(&InterpValue::Bool(true)));
        assert_eq!(
            host.session_call_count(),
            5,
            "preparation does not dispatch"
        );
        let published = crate::api::codec::checkpoint_artifact_json(
            &["publication.es".into()],
            "main",
            &result.checkpoints[1],
        )
        .unwrap();
        crate::api::codec::checkpoint_from_json(&published, &checked).unwrap();
        let mut invalid = published.clone();
        invalid["checkpoint"]["storage"]["operations"] = serde_json::json!({});
        assert!(
            crate::api::codec::checkpoint_from_json(&invalid, &checked)
                .unwrap_err()
                .message()
                .contains("does not belong to its boundary occurrence")
        );
        let mut altered = published;
        altered["checkpoint"]["storage"]["writes"][0]["evidence"]["operation"]["request_fingerprint"] =
            "0".repeat(64).into();
        assert!(crate::api::codec::checkpoint_from_json(&altered, &checked).is_err());
        let checkpoint = &result.checkpoints[0];
        assert!(checkpoint.storage.writes.is_empty());
        let artifact = crate::api::codec::checkpoint_artifact_json(
            &["publication.es".into()],
            "main",
            checkpoint,
        )
        .unwrap();
        let restored = crate::api::codec::checkpoint_from_json(&artifact, &checked).unwrap();
        let resumed = Interpreter
            .resume_checkpoint(&checked, &restored, &host, RunOptions::default())
            .await
            .unwrap();
        assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
        assert_eq!(resumed.value(), result.value());
        assert_eq!(host.session_call_count(), 9);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn session_publication_source_unknown_is_query_only_after_lost_or_invalid_receipt() {
    let checked = program(
        r#"
    var unknown = false;
    retry limit Attempts(3) {
        let result = publish_context(config, page.fence, content, operation);
        unknown = match result {WriteOutcome.Unknown(op) => op.key == operation.key, _ => false};
    }
    checkpoint("unknown");
    let query = reconcile_context(config, operation);
    return unknown && match query {ReconcileResult.Found(ConfirmedOutcome.Committed(receipt)) => receipt.operation.fingerprint == operation.fingerprint, _ => false};
    "#,
    );
    for sqlite in [false, true] {
        for fault in [
            SessionContextFault::LostResponse,
            SessionContextFault::ForeignReceipt,
        ] {
            let workspace = etas_host::TestWorkspace::create("source-context-unknown").unwrap();
            let mut host = FakeHost::new(availability(&[
                HostRequirementKind::DurableMemory,
                HostRequirementKind::Checkpoint,
            ]));
            if sqlite {
                host.persistent_session = Some(
                    etas_host::SqliteSessionClient::open(workspace.path().join("db")).unwrap(),
                );
            }
            host.session_context_fault = Some(fault);
            super::session_history::seed(&host, 2).await;
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
            assert!(
                result.diagnostics.is_empty(),
                "{sqlite}: {:?}",
                result.diagnostics
            );
            assert_eq!(result.value(), Some(&InterpValue::Bool(true)));
            assert_eq!(
                host.session_call_count(),
                3,
                "one history read, one publish, one query"
            );
            let artifact = crate::api::codec::checkpoint_artifact_json(
                &["unknown.es".into()],
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
            assert_eq!(resumed.value(), Some(&InterpValue::Bool(true)));
            assert_eq!(
                host.session_call_count(),
                4,
                "resume performs only the reconciliation query"
            );
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn session_publication_source_rejects_changed_payload_before_dispatch() {
    let checked = program(
        r#"
    let altered = SessionContextContent {text = "different", provenance = {"producer" => "application"}};
    let outcome: Result<WriteOutcome<SessionContextReceipt,SessionContextRejection>,StorageError> = publish_context(config,page.fence,altered,operation)?;
    return match outcome {Err(error) => error.code == "InvalidRequest", _ => false};
    "#,
    );
    let host = FakeHost::new(availability(&[HostRequirementKind::DurableMemory]));
    super::session_history::seed(&host, 2).await;
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
    assert_eq!(result.value(), Some(&InterpValue::Bool(true)));
    assert_eq!(host.session_call_count(), 1, "only initial history read");
}

#[tokio::test(flavor = "current_thread")]
async fn session_publication_source_rejects_stale_history_after_resume_without_recomputing() {
    let checked = program(
        r#"
    checkpoint("before-publication");
    let result = publish_context(config,page.fence,content,operation);
    return match result {WriteOutcome.NotCommitted(op,SessionContextRejection.StaleHistory) => op.key == operation.key, _ => false};
    "#,
    );
    for sqlite in [false, true] {
        let workspace = etas_host::TestWorkspace::create("source-context-stale").unwrap();
        let mut host = FakeHost::new(availability(&[
            HostRequirementKind::DurableMemory,
            HostRequirementKind::Checkpoint,
        ]));
        if sqlite {
            host.persistent_session =
                Some(etas_host::SqliteSessionClient::open(workspace.path().join("db")).unwrap());
        }
        super::session_history::seed(&host, 2).await;
        // Stop at the checkpoint, before a publication can commit.
        let observer = std::sync::Arc::new(StopAtCheckpoint::default());
        let invocation = Interpreter.create_run(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            vec![],
            &host,
            RunOptions {
                event_observer: Some(observer.clone()),
                ..Default::default()
            },
        );
        observer.arm(invocation.control());
        let result = invocation.execute().await.unwrap();
        assert_eq!(host.session_call_count(), 1);
        super::session_history::append(&host, 2).await;
        let resumed = Interpreter
            .resume_checkpoint(
                &checked,
                &result.checkpoints[0],
                &host,
                RunOptions::default(),
            )
            .await
            .unwrap();
        assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
        assert_eq!(resumed.value(), Some(&InterpValue::Bool(true)));
        assert_eq!(
            host.session_call_count(),
            2,
            "publication conflicts without summary/model work"
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn session_publication_source_cancellation_keeps_commit_evidence() {
    let checked = program(
        r#"
    publish_context(config,page.fence,content,operation);
    publish_context(config,page.fence,content,operation);
    return false;
    "#,
    );
    let host = FakeHost::new(availability(&[HostRequirementKind::DurableMemory]));
    super::session_history::seed(&host, 2).await;
    let observer = std::sync::Arc::new(super::memory_commit::StopOnStorageCommit::default());
    let invocation = Interpreter.create_run(
        &checked,
        EntryPoint {
            item: checked.entry.unwrap(),
        },
        vec![],
        &host,
        RunOptions {
            event_observer: Some(observer.clone()),
            ..Default::default()
        },
    );
    observer.arm(invocation.control());
    let result = invocation.execute().await.unwrap();
    assert!(matches!(
        result.outcome,
        crate::api::RunOutcome::Cancelled(_)
    ));
    assert_eq!(host.session_call_count(), 2);
    assert!(result.events.iter().any(|event| matches!(
        event,
        WorkflowEvent::StorageWrite {
            evidence: etas_host::StorageWriteEvidence {
                status: etas_host::CommitStatus::Committed { .. },
                ..
            },
            ..
        }
    )));
}

#[tokio::test(flavor = "current_thread")]
async fn session_publication_source_current_policy_denial_does_not_mutate() {
    let checked = program(
        r#"
    let result: Result<WriteOutcome<SessionContextReceipt,SessionContextRejection>,StorageError> = publish_context(config,page.fence,content,operation)?;
    return match result {Err(error)=>error.code=="AuthorityDenied",_=>false};
    "#,
    );
    let host = FakeHost::new(availability(&[HostRequirementKind::DurableMemory]));
    super::session_history::seed(&host, 2).await;
    host.seed_policy_decision(PolicyDecision::Allow);
    host.seed_policy_decision(PolicyDecision::Deny {
        reason: "deny publication".into(),
    });
    let mut options = RunOptions::default();
    options.host_context.authority.policy =
        boundary_policy_context(HostValue::String("context-policy".into()));
    let result = Interpreter
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
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&InterpValue::Bool(true)));
    assert_eq!(host.session_call_count(), 1);
}
