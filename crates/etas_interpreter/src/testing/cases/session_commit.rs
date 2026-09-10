use super::super::*;
use etas_host::SessionClient;

#[tokio::test(flavor = "current_thread")]
async fn history_fence_survives_source_checkpoint_and_resume_after_append() {
    let checked = checked_project(
        r#"
module app.main;
import std.runtime.checkpoint;
import std.agent.message.Message;
import std.agent.session.Conversation;
flow main(ticket: string) -> Conversation ![Memory.read<SessionId>, Memory.write<SessionId>] {
    let session = SessionConfig.continue_or_new(ticket);
    let _first = Message.with_session(Message.new("original"), session);
    let history = Conversation.load(session);
    checkpoint("selected-history");
    return history;
}
"#,
    );
    let workspace = etas_host::TestWorkspace::create("history-fence-checkpoint").unwrap();
    let path = workspace.path().join("db");
    let mut host = FakeHost::new(availability(&[
        HostRequirementKind::DurableMemory,
        HostRequirementKind::Checkpoint,
    ]));
    host.persistent_session = Some(etas_host::SqliteSessionClient::open(&path).unwrap());
    let first = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            vec![InterpValue::String("fence".into())],
            &host,
            RunOptions::default(),
        )
        .await
        .unwrap();
    assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);
    let Some(InterpValue::Conversation(history)) = first.value() else {
        panic!("conversation result")
    };
    let original = history.history_fence.clone().expect("backend-issued fence");
    let encoded_value = crate::api::codec::value_json(first.value().unwrap());
    assert_eq!(encoded_value["history_fence"], original.as_token());
    assert_eq!(
        &crate::api::codec::value_from_json(&encoded_value).unwrap(),
        first.value().unwrap()
    );
    let mut foreign = encoded_value.clone();
    foreign["session"] = "another session".into();
    assert!(
        crate::api::codec::value_from_json(&foreign)
            .unwrap_err()
            .message()
            .contains("another session")
    );
    let artifact = crate::api::codec::checkpoint_artifact_json(
        &[std::path::PathBuf::from("history-fence.es")],
        "main",
        &first.checkpoints[0],
    )
    .unwrap();
    let checkpoint = crate::api::codec::checkpoint_from_json(&artifact, &checked).unwrap();
    drop(host);
    let client = etas_host::SqliteSessionClient::open(&path).unwrap();
    client
        .execute(etas_host::SessionRequest {
            id: HostRequestId(100),
            operation: etas_host::SessionOperation::Append {
                message: etas_host::SessionMessage {
                    id: "later".into(),
                    session: etas_host::SessionRef {
                        id: history.session.clone(),
                    },
                    from: None,
                    to: None,
                    role: etas_host::SessionMessageRole::User,
                    created_at: "2026-09-10T00:00:00Z".into(),
                    payload: HostValue::String("later".into()),
                    provenance: None,
                    dedup_key: None,
                },
            },
            authority: AuthorityContext::deny_all(),
            trace: TraceContext::root(TraceId(1)),
            budget: Default::default(),
        })
        .await
        .unwrap()
        .result
        .unwrap();
    let mut host = FakeHost::new(availability(&[
        HostRequirementKind::DurableMemory,
        HostRequirementKind::Checkpoint,
    ]));
    host.persistent_session = Some(client);
    let resumed = Interpreter
        .resume_checkpoint(&checked, &checkpoint, &host, RunOptions::default())
        .await
        .unwrap();
    assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
    assert_eq!(
        resumed.value(),
        first.value(),
        "restoring history cannot mint a newer publication fence"
    );
    assert_eq!(
        host.session_call_count(),
        0,
        "resume must restore the selected history, not re-read it"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn published_context_survives_source_history_checkpoint_and_resume() {
    use etas_host::session::{
        SessionContextContent, SessionContextPublication, SessionWriteOperation,
        SessionWriteRequest, SessionWriteResult,
    };
    let workspace = etas_host::TestWorkspace::create("published-context-checkpoint").unwrap();
    let client = etas_host::SqliteSessionClient::open(workspace.path().join("db")).unwrap();
    let session = etas_host::SessionRef {
        id: "continue_or_new:caller-context".into(),
    };
    let request = |operation| etas_host::SessionRequest {
        id: HostRequestId(100),
        operation,
        authority: AuthorityContext::deny_all(),
        trace: TraceContext::root(TraceId(1)),
        budget: Default::default(),
    };
    client
        .execute(request(etas_host::SessionOperation::Resolve {
            config: etas_host::SessionConfig {
                id: session.id.clone(),
                context: etas_host::ContextPolicy::SummaryPlusRecent { recent: 8 },
                retention: etas_host::RetentionPolicy::Days(90),
            },
        }))
        .await
        .unwrap()
        .result
        .unwrap();
    let etas_host::SessionResult::History { fence, .. } = client
        .execute(request(etas_host::SessionOperation::Load {
            session: session.clone(),
            context: etas_host::ContextPolicy::All,
            cursor: None,
            limit: Some(1),
        }))
        .await
        .unwrap()
        .result
        .unwrap()
    else {
        panic!("history")
    };
    let publication = SessionContextPublication::prepare(
        session,
        fence,
        SessionContextContent {
            text: "application-produced context".into(),
            provenance: [
                ("producer".into(), "caller".into()),
                ("tokenizer".into(), "caller-v1".into()),
            ]
            .into(),
        },
        &etas_host::StorageLimits::default(),
    )
    .unwrap();
    let response = client
        .write(SessionWriteRequest {
            id: HostRequestId(101),
            operation: SessionWriteOperation::PublishContext(Box::new(publication.clone())),
            authority: AuthorityContext::deny_all(),
            trace: TraceContext::root(TraceId(1)),
            budget: Default::default(),
        })
        .await
        .unwrap()
        .result
        .unwrap();
    assert!(matches!(
        response,
        SessionWriteResult::Context(etas_host::WriteOutcome::Committed(_))
    ));
    let checked = checked_project(
        r#"
module app.main;
import std.runtime.checkpoint;
import std.agent.session.Conversation;
flow main(ticket: string) -> Conversation ![Memory.read<SessionId>, Memory.write<SessionId>] {
    let base = SessionConfig.continue_or_new(ticket);
    let session = SessionConfig { id = base.id, context = SummaryPlusRecent(8), retention = Days(90) };
    let history = Conversation.load(session);
    checkpoint("context-selected");
    return history;
}
"#,
    );
    let mut host = FakeHost::new(availability(&[
        HostRequirementKind::DurableMemory,
        HostRequirementKind::Checkpoint,
    ]));
    host.persistent_session = Some(client);
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            vec![InterpValue::String("caller-context".into())],
            &host,
            RunOptions::default(),
        )
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let Some(InterpValue::Conversation(history)) = result.value() else {
        panic!("conversation")
    };
    let context = history
        .selected_context
        .as_ref()
        .expect("published context retained");
    assert_eq!(context.content, publication.content);
    assert_eq!(context.fence, publication.fence);
    let value = crate::api::codec::value_json(result.value().unwrap());
    assert_eq!(
        crate::api::codec::value_from_json(&value).unwrap(),
        *result.value().unwrap()
    );
    let mut malformed = value;
    malformed["selected_context"]
        .as_object_mut()
        .unwrap()
        .remove("provenance");
    assert!(crate::api::codec::value_from_json(&malformed).is_err());
    let artifact = crate::api::codec::checkpoint_artifact_json(
        &["context.es".into()],
        "main",
        &result.checkpoints[0],
    )
    .unwrap();
    let checkpoint = crate::api::codec::checkpoint_from_json(&artifact, &checked).unwrap();
    let calls = host.session_call_count();
    let resumed = Interpreter
        .resume_checkpoint(&checked, &checkpoint, &host, RunOptions::default())
        .await
        .unwrap();
    assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
    assert_eq!(resumed.value(), result.value());
    assert_eq!(
        host.session_call_count(),
        calls,
        "resume must not recreate context from history"
    );
}

fn program() -> etas_frontend::CheckedProject {
    checked_project(
        r#"
module app.main;
import std.runtime.{checkpoint};
import std.agent.message.Message;
flow main(ticket: SessionId) -> unit {
    let config = SessionConfig { id = ticket, context = SummaryPlusRecent(1),
        retention = Days(90) };
    let message = Message.new("hello");
    checkpoint("before");
    retry limit Attempts(3) { let _stored = Message.with_session(message, config); }
    checkpoint("after");
}
"#,
    )
}

#[tokio::test(flavor = "current_thread")]
async fn sqlite_append_resume_reuses_original_receipt_and_does_not_append_again() {
    let checked = program();
    let workspace = etas_host::TestWorkspace::create("session-receipt-resume").unwrap();
    let path = workspace.path().join("session.db");
    let mut host = FakeHost::new(availability(&[
        HostRequirementKind::DurableMemory,
        HostRequirementKind::Checkpoint,
    ]));
    host.persistent_session = Some(etas_host::SqliteSessionClient::open(&path).unwrap());
    let first = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            vec![InterpValue::String("receipt-session".into())],
            &host,
            RunOptions::default(),
        )
        .await
        .unwrap();
    assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);
    assert_eq!(first.checkpoints.len(), 2);
    assert!(first.checkpoints[0].storage.writes.is_empty());
    let receipts = &first.checkpoints[1].storage.writes;
    assert_eq!(
        receipts.len(),
        2,
        "resolve and append each retain commit evidence"
    );
    assert!(
        matches!(&receipts[0].evidence.status,etas_host::CommitStatus::Committed {revision,..} if revision.starts_with("sg1:"))
    );
    assert!(
        matches!(&receipts[1].evidence.status,etas_host::CommitStatus::Committed {revision,..} if revision.starts_with("sv1:"))
    );
    let sources = [std::path::PathBuf::from("session-receipt.es")];
    let artifact =
        crate::api::codec::checkpoint_artifact_json(&sources, "main", &first.checkpoints[0])
            .unwrap();
    let restored = crate::api::codec::checkpoint_from_json(&artifact, &checked).unwrap();
    let after =
        crate::api::codec::checkpoint_artifact_json(&sources, "main", &first.checkpoints[1])
            .unwrap();
    crate::api::codec::checkpoint_from_json(&after, &checked).unwrap();
    let mut malformed = after.clone();
    malformed["checkpoint"]["storage"]["writes"][0]["evidence"]["status"]["revision"] =
        "sg1:malformed".into();
    assert!(crate::api::codec::checkpoint_from_json(&malformed, &checked).is_err());
    let mut old = after;
    old["schema"] = "etas.cli.interpreter-checkpoint.v22".into();
    assert!(
        crate::api::codec::checkpoint_from_json(&old, &checked)
            .unwrap_err()
            .message()
            .contains("unsupported checkpoint artifact schema")
    );
    drop(host);
    let mut host = FakeHost::new(availability(&[
        HostRequirementKind::DurableMemory,
        HostRequirementKind::Checkpoint,
    ]));
    let client = etas_host::SqliteSessionClient::open(&path).unwrap();
    host.persistent_session = Some(client.clone());
    let resumed = Interpreter
        .resume_checkpoint(&checked, &restored, &host, RunOptions::default())
        .await
        .unwrap();
    assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
    assert_eq!(
        host.session_call_count(),
        2,
        "resolve plus actual idempotent append dispatch"
    );
    assert_eq!(resumed.checkpoints[0].storage.writes.len(), receipts.len());
    for (actual, expected) in resumed.checkpoints[0].storage.writes.iter().zip(receipts) {
        assert_eq!(actual.request, expected.request);
        assert_eq!(actual.evidence, expected.evidence);
    }
    let response = client
        .execute(etas_host::SessionRequest {
            id: HostRequestId(999),
            operation: etas_host::SessionOperation::Load {
                session: etas_host::SessionRef {
                    id: "receipt-session".into(),
                },
                context: etas_host::ContextPolicy::All,
                cursor: None,
                limit: None,
            },
            authority: AuthorityContext::deny_all(),
            trace: TraceContext::root(TraceId(1)),
            budget: Default::default(),
        })
        .await
        .unwrap()
        .result
        .unwrap();
    assert!(
        matches!(response,etas_host::SessionResult::History {messages,..} if messages.len()==1)
    );
}

#[tokio::test(flavor = "current_thread")]
async fn lost_session_append_acknowledgement_preserves_unknown_operation_for_reconciliation() {
    lost_acknowledgement(false).await;
}

#[tokio::test(flavor = "current_thread")]
async fn lost_session_resolve_acknowledgement_preserves_original_creation_evidence() {
    lost_acknowledgement(true).await;
}

async fn lost_acknowledgement(resolve: bool) {
    let checked = program();
    let mut host = FakeHost::new(availability(&[
        HostRequirementKind::DurableMemory,
        HostRequirementKind::Checkpoint,
    ]));
    host.lose_session_append_response = !resolve;
    host.lose_session_resolve_response = resolve;
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            vec![InterpValue::String("unknown-session".into())],
            &host,
            RunOptions::default(),
        )
        .await
        .unwrap();
    assert!(
        result
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("commit outcome is unknown")),
        "{:?}",
        result.diagnostics
    );
    assert_eq!(host.session_call_count(), if resolve { 1 } else { 2 });
    let operation = result
        .events
        .iter()
        .find_map(|event| match event {
            WorkflowEvent::StorageWrite {
                evidence:
                    etas_host::StorageWriteEvidence {
                        operation,
                        status: etas_host::CommitStatus::Unknown,
                    },
                ..
            } => Some(operation.clone()),
            _ => None,
        })
        .expect("unknown commit identity retained");
    let found = host
        .session
        .write(etas_host::session::SessionWriteRequest {
            id: HostRequestId(99),
            operation: etas_host::session::SessionWriteOperation::Reconcile {
                session: etas_host::SessionRef {
                    id: "unknown-session".into(),
                },
                operation,
            },
            authority: AuthorityContext::deny_all(),
            trace: TraceContext::root(TraceId(1)),
            budget: Default::default(),
        })
        .await
        .unwrap()
        .result
        .unwrap();
    match found {
        etas_host::session::SessionWriteResult::Receipt(etas_host::ReceiptLookup::Found(
            etas_host::session::SessionWriteReceipt::Resolve(receipt),
        )) if resolve => assert!(receipt.created),
        etas_host::session::SessionWriteResult::Receipt(etas_host::ReceiptLookup::Found(
            etas_host::session::SessionWriteReceipt::Append(_),
        )) if !resolve => {}
        other => panic!("unexpected reconciliation: {other:?}"),
    }
    assert_eq!(
        result.checkpoints.len(),
        1,
        "failed delivery must not run later checkpoint"
    );
}
