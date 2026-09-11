use super::super::*;
use etas_host::{SessionClient, SessionOperation, SessionRef, SessionRequest, SessionResult};

fn request(operation: SessionOperation) -> SessionRequest {
    SessionRequest {
        id: HostRequestId(1000),
        operation,
        authority: AuthorityContext::deny_all(),
        trace: TraceContext::root(TraceId(1)),
        budget: Default::default(),
    }
}

async fn execute(host: &FakeHost, operation: SessionOperation) -> SessionResult {
    let response = match &host.persistent_session {
        Some(client) => client.execute(request(operation)).await,
        None => host.session.execute(request(operation)).await,
    };
    response.unwrap().result.unwrap()
}

pub(super) async fn seed(host: &FakeHost, count: usize) {
    execute(
        host,
        SessionOperation::Resolve {
            config: etas_host::SessionConfig {
                id: "continue_or_new:pages".into(),
                context: etas_host::ContextPolicy::All,
                retention: etas_host::RetentionPolicy::Forever,
            },
        },
    )
    .await;
    for i in 0..count {
        append(host, i).await;
    }
}

pub(super) async fn append(host: &FakeHost, i: usize) {
    execute(
        host,
        SessionOperation::Append {
            message: etas_host::SessionMessage {
                id: format!("message-{i:03}"),
                session: SessionRef {
                    id: "continue_or_new:pages".into(),
                },
                from: None,
                to: None,
                role: etas_host::SessionMessageRole::User,
                created_at: "2026-09-10T00:00:00Z".into(),
                payload: HostValue::String(format!("body-{i}")),
                provenance: None,
                dedup_key: None,
            },
        },
    )
    .await;
}

fn field(value: &InterpValue, name: &str) -> InterpValue {
    let InterpValue::Nominal { value, .. } = value else {
        panic!("checked nominal: {value:?}")
    };
    let InterpValue::Record(fields) = value.as_ref() else {
        panic!("record representation")
    };
    fields
        .borrow()
        .iter()
        .find(|(key, _)| key == name)
        .unwrap()
        .1
        .clone()
}

#[tokio::test(flavor = "current_thread")]
async fn session_history_source_pages_cover_101_messages_without_hidden_reads() {
    let checked = checked_project(
        r#"
module app.main;
import std.agent.session.history_page;
flow main() -> (SessionHistoryPage, SessionHistoryPage, SessionHistoryPage) {
    let config = SessionConfig.continue_or_new("pages");
    let first = history_page(config, None(), 50);
    let second = history_page(config, first.cursor, 50);
    let third = history_page(config, second.cursor, 50);
    return (first, second, third);
}
"#,
    );
    let summary = &checked.effects.item_effects[&checked.entry.unwrap()];
    assert_eq!(
        summary.requested_actions.effects,
        [etas_effects::Effect::AppliedAction(
            etas_effects::ActionInstanceRef {
                action: etas_effects::ActionRef {
                    tag: etas_effects::MEMORY_TAG,
                    action: etas_effects::MEMORY_READ_ACTION
                },
                args: vec![etas_types::EffectArgRef::Path(
                    ["std", "agent", "session", "SessionId"]
                        .map(str::to_owned)
                        .to_vec()
                )],
            }
        )]
        .into_iter()
        .collect()
    );
    assert_eq!(summary.escaping_effects.effects.iter().count(), 1);
    assert!(summary.escaping_effects.effects.iter().any(|effect| match effect {
        etas_effects::Effect::Error(ty) => matches!(checked.type_store.get(*ty), Some(etas_types::Type::Nominal(nominal)) if nominal.name == "std.memory.StorageError"),
        _ => false,
    }));
    for sqlite in [false, true] {
        let workspace = etas_host::TestWorkspace::create("source-history-pages").unwrap();
        let mut host = FakeHost::new(availability(&[HostRequirementKind::DurableMemory]));
        if sqlite {
            host.persistent_session =
                Some(etas_host::SqliteSessionClient::open(workspace.path().join("db")).unwrap());
        }
        seed(&host, 101).await;
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
        let Some(InterpValue::Tuple(pages)) = result.value() else {
            panic!("three pages: {:?}", result.value())
        };
        let mut ids = Vec::new();
        for (page, expected) in pages.iter().zip([50, 50, 1]) {
            let InterpValue::Array(messages) = field(page, "messages") else {
                panic!("message array")
            };
            assert_eq!(messages.borrow().len(), expected);
            for message in messages.borrow().iter() {
                let InterpValue::Message(message) = message else {
                    panic!("canonical Message ABI: {message:?}")
                };
                ids.push(message.id.clone());
            }
        }
        assert_eq!(
            ids,
            (0..101)
                .map(|i| format!("message-{i:03}"))
                .collect::<Vec<_>>()
        );
        assert_eq!(field(&pages[2], "cursor"), InterpValue::OptionNone);
        assert_eq!(
            host.session_call_count(),
            3,
            "exactly one host read per source call"
        );
        let actions = result
            .events
            .iter()
            .filter_map(|event| match event {
                WorkflowEvent::HostTrace(etas_host::TraceEvent::HostRequestStarted {
                    metadata,
                    ..
                }) => Some(metadata.qualified_action.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(actions, ["Session.load", "Session.load", "Session.load"]);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn session_history_source_cursor_survives_checkpoint_and_concurrent_append() {
    let checked = checked_project(
        r#"
module app.main;
import std.runtime.checkpoint;
import std.agent.session.history_page;
flow main(ticket: SessionId) -> SessionHistoryPage {
    let config = SessionConfig { id = ticket, context = LastTurns(1),
        retention = Days(90) };
    let first = history_page(config, None(), 1);
    checkpoint("first-page");
    return history_page(config, first.cursor, 1);
}
"#,
    );
    let workspace = etas_host::TestWorkspace::create("source-history-resume").unwrap();
    let mut host = FakeHost::new(availability(&[
        HostRequirementKind::DurableMemory,
        HostRequirementKind::Checkpoint,
    ]));
    host.persistent_session =
        Some(etas_host::SqliteSessionClient::open(workspace.path().join("db")).unwrap());
    seed(&host, 3).await;
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            vec![InterpValue::String("continue_or_new:pages".into())],
            &host,
            RunOptions::default(),
        )
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let artifact = crate::api::codec::checkpoint_artifact_json(
        &["history.es".into()],
        "main",
        &result.checkpoints[0],
    )
    .unwrap();
    let checkpoint = crate::api::codec::checkpoint_from_json(&artifact, &checked).unwrap();
    append(&host, 3).await;
    let resumed = Interpreter
        .resume_checkpoint(&checked, &checkpoint, &host, RunOptions::default())
        .await
        .unwrap();
    assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
    assert_eq!(
        resumed.value(),
        result.value(),
        "cursor retains original upper ordinal"
    );
    assert_eq!(
        field(resumed.value().unwrap(), "cursor"),
        InterpValue::OptionNone
    );
    assert_eq!(host.session_call_count(), 3);
}

#[tokio::test(flavor = "current_thread")]
async fn session_history_source_rejects_invalid_cursor_and_zero_limit_as_typed_errors() {
    for (cursor, limit, calls) in [
        ("Some(SessionCursor { opaque = \"forged\" })", 1, 1),
        ("None()", 0, 0),
    ] {
        let checked = checked_project(&format!(
            r#"
module app.main;
import std.agent.session.history_page;
flow main() -> bool {{
    let config = SessionConfig.continue_or_new("pages");
    let result: Result<SessionHistoryPage, StorageError> = history_page(config, {cursor}, {limit})?;
    return match result {{ Err(error) => error.code == "InvalidRequest", _ => false }};
}}
"#
        ));
        for sqlite in [false, true] {
            let workspace = etas_host::TestWorkspace::create("source-history-errors").unwrap();
            let mut host = FakeHost::new(availability(&[HostRequirementKind::DurableMemory]));
            if sqlite {
                host.persistent_session = Some(
                    etas_host::SqliteSessionClient::open(workspace.path().join("db")).unwrap(),
                );
            }
            seed(&host, 2).await;
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
            assert_eq!(
                result.value(),
                Some(&InterpValue::Bool(true)),
                "{sqlite}: {limit}"
            );
            assert_eq!(host.session_call_count(), calls);
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn session_history_source_preserves_caller_published_context() {
    use etas_host::session::{
        SessionContextContent, SessionContextPublication, SessionWriteOperation,
        SessionWriteRequest, SessionWriteResult,
    };
    let checked = checked_project(
        r#"
module app.main;
import std.agent.session.history_page;
flow main() -> string {
    let page = history_page(SessionConfig.continue_or_new("pages"), None(), 1);
    return match page.published_context {
        Some(context) => context.content.text,
        None => "missing",
    };
}

"#,
    );
    for sqlite in [false, true] {
        let workspace = etas_host::TestWorkspace::create("source-published-history").unwrap();
        let mut host = FakeHost::new(availability(&[HostRequirementKind::DurableMemory]));
        if sqlite {
            host.persistent_session =
                Some(etas_host::SqliteSessionClient::open(workspace.path().join("db")).unwrap());
        }
        seed(&host, 2).await;
        let session = SessionRef {
            id: "continue_or_new:pages".into(),
        };
        let SessionResult::History { fence, .. } = execute(
            &host,
            SessionOperation::Load {
                session: session.clone(),
                context: etas_host::ContextPolicy::All,
                cursor: None,
                limit: Some(1),
            },
        )
        .await
        else {
            panic!("history")
        };
        let publication = SessionContextPublication::prepare(
            session,
            fence,
            SessionContextContent {
                text: "context produced by the application".into(),
                provenance: [("producer".into(), "application".into())].into(),
            },
            &etas_host::StorageLimits::default(),
        )
        .unwrap();
        let request = SessionWriteRequest {
            id: HostRequestId(2000),
            operation: SessionWriteOperation::PublishContext(Box::new(publication)),
            authority: AuthorityContext::deny_all(),
            trace: TraceContext::root(TraceId(1)),
            budget: Default::default(),
        };
        let result = match &host.persistent_session {
            Some(client) => client.write(request).await,
            None => host.session.write(request).await,
        }
        .unwrap()
        .result
        .unwrap();
        assert!(matches!(
            result,
            SessionWriteResult::Context(etas_host::WriteOutcome::Committed(_))
        ));
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
        assert_eq!(
            result.value(),
            Some(&InterpValue::String(
                "context produced by the application".into()
            ))
        );
        assert_eq!(host.session_call_count(), 1);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn session_history_source_policy_denial_precedes_read() {
    let checked = checked_project(
        r#"
module app.main;
import std.agent.session.history_page;
flow main() -> bool {
    let config = SessionConfig.continue_or_new("pages");
    let result: Result<SessionHistoryPage, StorageError> = history_page(config, None(), 1)?;
    return match result { Err(error) => error.code == "AuthorityDenied", _ => false };
}
"#,
    );
    let host = FakeHost::new(availability(&[HostRequirementKind::DurableMemory]));
    host.seed_policy_decision(PolicyDecision::Deny {
        reason: "deny history".into(),
    });
    let mut options = RunOptions::default();
    options.host_context.authority.policy =
        boundary_policy_context(HostValue::String("history-policy".into()));
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
    assert_eq!(host.session_call_count(), 0);
    assert_eq!(host.policy_call_count(), 1);
}
