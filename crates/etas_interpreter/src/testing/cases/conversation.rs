use super::super::*;
use etas_host::{SessionClient, SessionOperation, SessionRef, SessionRequest};

async fn seed_message(host: &FakeHost, payload: &str) {
    super::session_history::seed(host, 0).await;
    let request = SessionRequest {
        id: HostRequestId(2000),
        operation: SessionOperation::Append {
            message: etas_host::SessionMessage {
                id: "large".into(),
                session: SessionRef {
                    id: "continue_or_new:pages".into(),
                },
                from: None,
                to: None,
                role: etas_host::SessionMessageRole::User,
                created_at: "2026-09-10T00:00:00Z".into(),
                payload: HostValue::String(payload.into()),
                provenance: None,
                dedup_key: None,
            },
        },
        authority: AuthorityContext::deny_all(),
        trace: TraceContext::root(TraceId(1)),
        budget: Default::default(),
    };
    match &host.persistent_session {
        Some(client) => client.execute(request).await,
        None => host.session.execute(request).await,
    }
    .unwrap()
    .result
    .unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn conversation_load_decode_and_resume_reject_oversized_history() {
    let checked = checked_project(
        r#"
module app.main;
import std.runtime.checkpoint;
flow main() -> usize {
    let conversation = Conversation.load(SessionConfig.continue_or_new("pages"));
    checkpoint("loaded");
    return conversation.messages.len();
}
"#,
    );
    for sqlite in [false, true] {
        let workspace = etas_host::TestWorkspace::create("conversation-size").unwrap();
        let mut host = FakeHost::new(availability(&[
            HostRequirementKind::DurableMemory,
            HostRequirementKind::Checkpoint,
        ]));
        if sqlite {
            host.persistent_session =
                Some(etas_host::SqliteSessionClient::open(workspace.path().join("db")).unwrap());
        }
        seed_message(&host, &"x".repeat(64 * 1024)).await;
        let entry = EntryPoint {
            item: checked.entry.unwrap(),
        };
        let loaded = Interpreter
            .run_checked(&checked, entry, vec![], &host, RunOptions::default())
            .await
            .unwrap();
        assert!(loaded.diagnostics.is_empty(), "{:?}", loaded.diagnostics);
        assert_eq!(loaded.value(), Some(&InterpValue::usize(1)));
        let limits = etas_host::StorageLimits {
            max_value_bytes: 8 * 1024,
            ..Default::default()
        };
        let options = RunOptions {
            storage_limits: limits.clone(),
            ..Default::default()
        };
        let direct = Interpreter
            .run_checked(&checked, entry, vec![], &host, options.clone())
            .await
            .unwrap();
        assert!(!direct.diagnostics.is_empty(), "load must reject: {sqlite}");
        let checkpoint = &loaded.checkpoints[0];
        let artifact = crate::api::codec::checkpoint_artifact_json(
            &["conversation.es".into()],
            "main",
            checkpoint,
        )
        .unwrap();
        let error =
            crate::api::codec::checkpoint_from_json_with_limits(&limits, &artifact, &checked)
                .unwrap_err();
        assert!(
            error.message().contains("storage resource limit"),
            "{error}"
        );
        let restored = Interpreter
            .resume_checkpoint(&checked, checkpoint, &host, options)
            .await
            .unwrap();
        assert!(
            !restored.diagnostics.is_empty(),
            "in-memory restore must reject: {sqlite}"
        );
        assert!(
            restored
                .diagnostics
                .iter()
                .any(|d| d.message.contains("storage resource limit"))
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn conversation_selects_published_context_only_for_summary_policy() {
    for (policy, count, summary) in [
        ("LastTurns(0)", 0, false),
        ("LastTurns(1)", 2, false),
        ("SummaryPlusRecent(1)", 2, true),
    ] {
        let checked = checked_project(&format!(
            r#"
module app.main;
import std.agent.session.{{history_page, prepare_context, publish_context}};
import std.runtime.checkpoint;
flow main(ticket: SessionId) -> bool {{
    let config = SessionConfig {{ id = ticket, context = {policy}, retention = Days(90) }};
    let before = history_page(config, None(), 10);
    let content = SessionContextContent {{ text = "published", provenance = {{"producer" => "app"}} }};
    let operation = prepare_context(config, before.fence, content);
    let outcome = publish_context(config, before.fence, content, operation);
    let page = history_page(config, None(), 10);
    let conversation = Conversation.load(config);
    checkpoint("selected");
    let committed = match outcome {{ WriteOutcome.Committed(_) => true, _ => false }};
    let stored = match page.published_context {{ Some(_) => true, _ => false }};
    let selected = match conversation.summary {{ Some(context) => context.content.text == "published", _ => false }};
    return committed && stored && selected == {summary} && conversation.messages.len() == {count};
}}
"#
        ));
        for sqlite in [false, true] {
            let workspace = etas_host::TestWorkspace::create("conversation-selection").unwrap();
            let mut host = FakeHost::new(availability(&[
                HostRequirementKind::DurableMemory,
                HostRequirementKind::Checkpoint,
            ]));
            if sqlite {
                host.persistent_session = Some(
                    etas_host::SqliteSessionClient::open(workspace.path().join("db")).unwrap(),
                );
            }
            let context = match policy {
                "LastTurns(0)" => etas_host::ContextPolicy::LastTurns(0),
                "LastTurns(1)" => etas_host::ContextPolicy::LastTurns(1),
                _ => etas_host::ContextPolicy::SummaryPlusRecent { recent: 1 },
            };
            let request = SessionRequest {
                id: HostRequestId(1000),
                operation: SessionOperation::Resolve {
                    config: etas_host::SessionConfig {
                        id: "continue_or_new:pages".into(),
                        context,
                        retention: etas_host::RetentionPolicy::Days(90),
                    },
                },
                authority: AuthorityContext::deny_all(),
                trace: TraceContext::root(TraceId(1)),
                budget: Default::default(),
            };
            match &host.persistent_session {
                Some(client) => client.execute(request).await,
                None => host.session.execute(request).await,
            }
            .unwrap()
            .result
            .unwrap();
            for i in 0..4 {
                super::session_history::append(&host, i).await;
            }
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
            assert!(
                result.diagnostics.is_empty(),
                "{policy}/{sqlite}: {:?}",
                result.diagnostics
            );
            assert_eq!(
                result.value(),
                Some(&InterpValue::Bool(true)),
                "{policy}/{sqlite}"
            );
            let selections = result
                .events
                .iter()
                .filter_map(|event| match event {
                    WorkflowEvent::SessionHistoryLoaded { has_summary, .. } => Some(*has_summary),
                    _ => None,
                })
                .collect::<Vec<_>>();
            assert_eq!(
                selections,
                vec![summary],
                "trace must describe the selected view"
            );
            let artifact = crate::api::codec::checkpoint_artifact_json(
                &["conversation.es".into()],
                "main",
                &result.checkpoints[0],
            )
            .unwrap();
            let checkpoint = crate::api::codec::checkpoint_from_json(&artifact, &checked).unwrap();
            let restored = Interpreter
                .resume_checkpoint(&checked, &checkpoint, &host, RunOptions::default())
                .await
                .unwrap();
            assert!(
                restored.diagnostics.is_empty(),
                "{:?}",
                restored.diagnostics
            );
            assert_eq!(
                restored.value(),
                Some(&InterpValue::Bool(true)),
                "{policy}/{sqlite}"
            );
        }
    }
}
