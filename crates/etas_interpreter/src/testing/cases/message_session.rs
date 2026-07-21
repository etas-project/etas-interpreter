use super::super::*;

#[tokio::test(flavor = "current_thread")]
async fn run_checked_executes_message_new_and_checked_cast_without_host() {
    let checked = checked_project(
        r#"
module app.main;
import std.agent.message.Message;

flow main(input: string) -> Option<Message<string>> {
  let message = Message.new(input);
  return message.cast<string>();
}
"#,
    );

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            vec![value::InterpValue::String("hello".to_owned())],
            &FakeHost::new(HostServiceAvailability::default()),
            RunOptions::default(),
        )
        .await;

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let Some(value::InterpValue::OptionSome(message)) = result.value else {
        panic!("expected cast message, got {:?}", result.value);
    };
    let value::InterpValue::Message(message) = *message else {
        panic!("expected message value, got {message:?}");
    };
    assert_eq!(message.id, "msg-0");
    assert_eq!(message.from, None);
    assert_eq!(message.to, None);
    assert_eq!(message.role, value::MessageRoleValue::User);
    assert_eq!(message.session, None);
    assert!(is_unix_timestamp(&message.created_at), "{message:?}");
    assert_eq!(
        *message.payload,
        value::InterpValue::String("hello".to_owned())
    );
    assert_eq!(
        message.provenance,
        Some(value::ProvenanceValue {
            trace_id: Some("TraceId(0)".to_owned()),
            source: Some("Message.new".to_owned()),
        })
    );
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_message_cast_returns_none_on_payload_type_mismatch() {
    let checked = checked_project(
        r#"
module app.main;
import std.agent.message.Message;

flow main(input: Message<string>) -> Option<Message<string>> {
  return input.cast<string>();
}
"#,
    );

    let corrupted_message = value::InterpValue::Message(value::MessageValue {
        id: "external-message".to_owned(),
        from: None,
        to: None,
        role: value::MessageRoleValue::User,
        session: None,
        created_at: "external-boundary".to_owned(),
        payload: Box::new(value::InterpValue::i32(42)),
        provenance: None,
    });

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            vec![corrupted_message],
            &FakeHost::new(HostServiceAvailability::default()),
            RunOptions::default(),
        )
        .await;

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value, Some(value::InterpValue::OptionNone));
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_message_new_links_current_runtime_session() {
    let checked = checked_project(
        r#"
module app.main;
import std.agent.message.Message;

flow main(input: string) -> Message<string> {
  return Message.new(input);
}
"#,
    );

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            vec![value::InterpValue::String("hello".to_owned())],
            &FakeHost::new(HostServiceAvailability::default()),
            RunOptions {
                current_session: Some("session-42".to_owned()),
                ..RunOptions::default()
            },
        )
        .await;

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let Some(value::InterpValue::Message(message)) = result.value else {
        panic!("expected message value, got {:?}", result.value);
    };
    assert_eq!(message.session.as_deref(), Some("session-42"));
    assert_eq!(
        message.provenance,
        Some(value::ProvenanceValue {
            trace_id: Some("TraceId(0)".to_owned()),
            source: Some("Message.new".to_owned()),
        })
    );
    assert!(result.events.iter().any(|event| {
        matches!(
            event,
            WorkflowEvent::MessageCreated {
                id,
                session: Some(session),
                role,
                created_at,
                payload,
                provenance: Some(provenance),
                ..
            } if id == "msg-0"
                && session == "session-42"
                && role == "user"
                && is_unix_timestamp(created_at)
                && payload.as_ref() == &value::InterpValue::String("hello".to_owned())
                && provenance.trace_id.as_deref() == Some("TraceId(0)")
                && provenance.source.as_deref() == Some("Message.new")
        )
    }));
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_reads_message_body_payload() {
    let checked = checked_project(
        r#"
module app.main;
import std.agent.message.Message;

flow main(input: string) -> string {
  let message = Message.new(input);
  return message.body;
}
"#,
    );

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            vec![value::InterpValue::String("hello".to_owned())],
            &FakeHost::new(HostServiceAvailability::default()),
            RunOptions::default(),
        )
        .await;

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.value,
        Some(value::InterpValue::String("hello".to_owned()))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_reads_message_content_and_metadata_fields() {
    let checked = checked_project(
        r#"
module app.main;
import std.agent.message.{Message, MessageId, Provenance, Role};
import std.agent.session.SessionId;
import std.runtime.time.Time;

flow main(input: string) -> (string, MessageId, Option<SessionId>, Role, Time, Option<Provenance>) {
  let message = Message.new(input);
  return (
    message.content,
    message.id,
    message.session,
    message.role,
    message.created_at,
    message.provenance
  );
}
"#,
    );

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            vec![value::InterpValue::String("hello".to_owned())],
            &FakeHost::new(HostServiceAvailability::default()),
            RunOptions {
                current_session: Some("session-42".to_owned()),
                ..RunOptions::default()
            },
        )
        .await;

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let Some(value::InterpValue::Tuple(values)) = result.value else {
        panic!("expected metadata tuple, got {:?}", result.value);
    };
    assert_eq!(values.len(), 6);
    assert_eq!(values[0], value::InterpValue::String("hello".to_owned()));
    assert_eq!(values[1], value::InterpValue::String("msg-0".to_owned()));
    assert_eq!(
        values[2],
        value::InterpValue::OptionSome(Box::new(value::InterpValue::String(
            "session-42".to_owned()
        )))
    );
    assert_eq!(values[3], value::InterpValue::String("user".to_owned()));
    let value::InterpValue::String(created_at) = &values[4] else {
        panic!("expected timestamp string, got {:?}", values[4]);
    };
    assert!(is_unix_timestamp(created_at));
    assert_eq!(
        values[5],
        value::InterpValue::OptionSome(Box::new(value::InterpValue::Provenance(
            value::ProvenanceValue {
                trace_id: Some("TraceId(0)".to_owned()),
                source: Some("Message.new".to_owned()),
            }
        )))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_executes_session_policy_constructors_without_host() {
    let checked = checked_project(
        r#"
module app.main;

flow main(limit: Limit) -> (ContextPolicy, ContextPolicy, RetentionPolicy, CompactionPolicy) {
  return (LastTurns(8), SummaryPlusRecent(4), Days(90), SummarizeWhen(limit));
}
"#,
    );
    let limit = value::InterpValue::Variant {
        name: "ContextTokens".to_owned(),
        fields: vec![value::InterpValue::i32(24_000)],
    };

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            vec![limit.clone()],
            &FakeHost::new(HostServiceAvailability::default()),
            RunOptions::default(),
        )
        .await;

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.value,
        Some(value::InterpValue::Tuple(vec![
            value::InterpValue::Variant {
                name: "LastTurns".to_owned(),
                fields: vec![value::InterpValue::usize(8)],
            },
            value::InterpValue::Variant {
                name: "SummaryPlusRecent".to_owned(),
                fields: vec![value::InterpValue::usize(4)],
            },
            value::InterpValue::Variant {
                name: "Days".to_owned(),
                fields: vec![value::InterpValue::usize(90)],
            },
            value::InterpValue::Variant {
                name: "SummarizeWhen".to_owned(),
                fields: vec![limit],
            },
        ]))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_executes_session_config_continue_or_new_without_host() {
    let checked = checked_project(
        r#"
module app.main;

flow main(ticket: string) -> SessionConfig {
  return SessionConfig.continue_or_new(ticket);
}
"#,
    );

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            vec![value::InterpValue::String("ticket-42".to_owned())],
            &FakeHost::new(HostServiceAvailability::default()),
            RunOptions::default(),
        )
        .await;

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.value,
        Some(value::InterpValue::Variant {
            name: "SessionConfig.continue_or_new".to_owned(),
            fields: vec![value::InterpValue::String("ticket-42".to_owned())],
        })
    );
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_reads_session_config_continue_or_new_id() {
    let checked = checked_project(
        r#"
module app.main;

flow main(ticket: string) -> SessionId {
  let session = SessionConfig.continue_or_new(ticket);
  return session.id;
}
"#,
    );

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            vec![value::InterpValue::String("ticket-42".to_owned())],
            &FakeHost::new(HostServiceAvailability::default()),
            RunOptions::default(),
        )
        .await;

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.value,
        Some(value::InterpValue::String(
            "continue_or_new:ticket-42".to_owned()
        ))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_aborts_unmaterialized_session_config_policy_field() {
    let checked = checked_project(
        r#"
module app.main;

flow main(ticket: string) -> ContextPolicy {
  let session = SessionConfig.continue_or_new(ticket);
  return session.context;
}
"#,
    );

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            vec![value::InterpValue::String("ticket-42".to_owned())],
            &FakeHost::new(HostServiceAvailability::default()),
            RunOptions::default(),
        )
        .await;

    assert_eq!(result.value, None);
    assert_eq!(result.diagnostics.len(), 1, "{:?}", result.diagnostics);
    assert_eq!(
        result.diagnostics[0].code,
        DiagnosticCode::Analysis(AnalysisDiagnosticCode::InvalidArguments)
    );
    assert!(
        result.diagnostics[0]
            .message
            .contains("session config field `context` is not materialized"),
        "{:?}",
        result.diagnostics
    );
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_reads_current_runtime_session() {
    let checked = checked_project(
        r#"
module app.main;
import std.agent.session.current_session;

flow main() -> SessionId ![Memory.read<SessionId>] {
  return current_session();
}
"#,
    );

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &FakeHost::new(availability(&[HostRequirementKind::DurableMemory])),
            RunOptions {
                current_session: Some("session-42".to_owned()),
                ..RunOptions::default()
            },
        )
        .await;

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.value,
        Some(value::InterpValue::String("session-42".to_owned()))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_rejects_current_session_without_runtime_session() {
    let checked = checked_project(
        r#"
module app.main;
import std.agent.session.current_session;

flow main() -> SessionId ![Memory.read<SessionId>] {
  return current_session();
}
"#,
    );

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &FakeHost::new(availability(&[HostRequirementKind::DurableMemory])),
            RunOptions::default(),
        )
        .await;

    assert_eq!(result.value, None);
    assert!(result.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("current_session requires an active runtime session")
    }));
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_attaches_session_config_to_runtime_session_state() {
    let checked = checked_project(
        r#"
module app.main;
import std.agent.message.Message;

flow main(ticket: string, input: string) -> Message<string> {
  let message = Message.new(input);
  let session = SessionConfig.continue_or_new(ticket);
  return Message.with_session(message, session);
}
"#,
    );

    let host = FakeHost::new(HostServiceAvailability::default());
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            vec![
                value::InterpValue::String("ticket-42".to_owned()),
                value::InterpValue::String("hello".to_owned()),
            ],
            &host,
            RunOptions::default(),
        )
        .await;

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(host.session_call_count(), 2);
    let Some(value::InterpValue::Message(message)) = result.value else {
        panic!("expected message value, got {:?}", result.value);
    };
    assert_eq!(message.id, "msg-0");
    assert_eq!(message.role, value::MessageRoleValue::User);
    assert_eq!(
        message.session.as_deref(),
        Some("continue_or_new:ticket-42")
    );
    assert_eq!(
        *message.payload,
        value::InterpValue::String("hello".to_owned())
    );
    assert!(result.events.iter().any(|event| {
        matches!(
            event,
            WorkflowEvent::MessageCreated {
                id,
                session: None,
                role,
                payload,
                ..
            } if id == "msg-0"
                && role == "user"
                && payload.as_ref() == &value::InterpValue::String("hello".to_owned())
        )
    }));
    assert!(result.events.iter().any(|event| {
        matches!(
            event,
            WorkflowEvent::MessageSessionAttached {
                id,
                session,
                session_config,
            } if id == "msg-0"
                && session == "continue_or_new:ticket-42"
                && session_config.id == "continue_or_new:ticket-42"
                && session_config.context.is_none()
                && session_config.retention.is_none()
                && session_config.compaction.is_none()
        )
    }));
}

#[tokio::test(flavor = "current_thread")]
async fn resume_checkpoint_replays_completed_session_append_without_rewriting_history() {
    let checked = checked_project(
        r#"
module app.main;
import std.agent.message.Message;
import std.runtime.{checkpoint};

flow main(ticket: string, input: string) -> Message<string> {
  checkpoint("before-session");
  let message = Message.new(input);
  let session = SessionConfig.continue_or_new(ticket);
  let scoped = Message.with_session(message, session);
  checkpoint("after-session");
  return scoped;
}
"#,
    );

    let first_host = FakeHost::new(availability(&[
        HostRequirementKind::Checkpoint,
        HostRequirementKind::DurableMemory,
    ]));
    let first = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            vec![
                value::InterpValue::String("ticket-42".to_owned()),
                value::InterpValue::String("hello".to_owned()),
            ],
            &first_host,
            RunOptions::default(),
        )
        .await;

    assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);
    assert_eq!(first_host.session_call_count(), 2);
    assert_eq!(first.checkpoints.len(), 2);
    assert!(
        first.checkpoints[1]
            .completed_host_boundaries
            .completed
            .iter()
            .any(|boundary| boundary.kind == "session"),
        "{:?}",
        first.checkpoints[1].completed_host_boundaries.completed
    );

    let mut replay_checkpoint = first.checkpoints[0].clone();
    replay_checkpoint.completed_host_boundaries =
        first.checkpoints[1].completed_host_boundaries.clone();

    let replay_host = FakeHost::new(availability(&[
        HostRequirementKind::Checkpoint,
        HostRequirementKind::DurableMemory,
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
    assert_eq!(replay_host.session_call_count(), 1);
    let Some(value::InterpValue::Message(message)) = resumed.value else {
        panic!("expected message value, got {:?}", resumed.value);
    };
    assert_eq!(
        message.session.as_deref(),
        Some("continue_or_new:ticket-42")
    );
    assert_eq!(
        *message.payload,
        value::InterpValue::String("hello".to_owned())
    );
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_attaches_session_config_record_to_runtime_session_state() {
    let checked = checked_project(
        r#"
module app.main;
import std.agent.message.Message;

flow main(ticket: SessionId, input: string, limit: Limit) -> Message<string> {
  let message = Message.new(input);
  let session = SessionConfig {
    id = ticket,
    context = SummaryPlusRecent(8),
    retention = Days(90),
    compaction = SummarizeWhen(limit),
  };
  return Message.with_session(message, session);
}
"#,
    );

    let host = FakeHost::new(HostServiceAvailability::default());
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            vec![
                value::InterpValue::String("session-record-42".to_owned()),
                value::InterpValue::String("hello".to_owned()),
                value::InterpValue::Variant {
                    name: "ContextTokens".to_owned(),
                    fields: vec![value::InterpValue::i32(24_000)],
                },
            ],
            &host,
            RunOptions::default(),
        )
        .await;

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(host.session_call_count(), 2);
    let Some(value::InterpValue::Message(message)) = result.value else {
        panic!("expected message value, got {:?}", result.value);
    };
    assert_eq!(message.session.as_deref(), Some("session-record-42"));
    assert_eq!(
        *message.payload,
        value::InterpValue::String("hello".to_owned())
    );
    assert!(result.events.iter().any(|event| {
        matches!(
            event,
            WorkflowEvent::MessageSessionAttached {
                id,
                session,
                session_config,
            } if id == "msg-0"
                && session == "session-record-42"
                && session_config.id == "session-record-42"
                && session_config.context.is_some()
                && session_config.retention.is_some()
                && session_config.compaction.is_some()
        )
    }));
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_loads_runtime_conversation_history() {
    let checked = checked_project(
        r#"
module app.main;
import std.agent.message.Message;
import std.agent.session.Conversation;

flow main(ticket: string) -> usize ![Memory.read<SessionId>, Memory.write<SessionId>] {
  let session = SessionConfig.continue_or_new(ticket);
  let _first = Message.with_session(Message.new("hello"), session);
  let _second = Message.with_session(Message.new("again"), session);
  let conversation = Conversation.load(session);
  return conversation.messages.len();
}
"#,
    );

    let host = FakeHost::new(availability(&[HostRequirementKind::DurableMemory]));
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            vec![value::InterpValue::String("ticket-load".to_owned())],
            &host,
            RunOptions::default(),
        )
        .await;

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(host.session_call_count(), 6);
    assert_eq!(result.value, Some(value::InterpValue::usize(2)));
    assert!(result.events.iter().any(|event| {
        matches!(
            event,
            WorkflowEvent::SessionMessageAppended {
                session,
                message,
                deduplicated: false,
            } if session == "continue_or_new:ticket-load" && message == "msg-0"
        )
    }));
    assert!(result.events.iter().any(|event| {
        matches!(
            event,
            WorkflowEvent::SessionHistoryLoaded {
                session,
                message_count: 2,
                ..
            } if session == "continue_or_new:ticket-load"
        )
    }));
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_loads_retained_runtime_conversation_history() {
    let checked = checked_project(
        r#"
module app.main;
import std.agent.message.Message;
import std.agent.session.Conversation;

flow main(ticket: SessionId, limit: Limit) -> usize ![Memory.read<SessionId>, Memory.write<SessionId>] {
  let session = SessionConfig {
    id = ticket,
    context = SummaryPlusRecent(4),
    retention = Days(1),
    compaction = SummarizeWhen(limit),
  };
  let _message = Message.with_session(Message.new("hello"), session);
  let conversation = Conversation.load(session);
  return conversation.messages.len();
}
"#,
    );

    let host = FakeHost::new(availability(&[HostRequirementKind::DurableMemory]));
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            vec![
                value::InterpValue::String("ticket-retention".to_owned()),
                value::InterpValue::Variant {
                    name: "ContextTokens".to_owned(),
                    fields: vec![value::InterpValue::i32(100)],
                },
            ],
            &host,
            RunOptions::default(),
        )
        .await;

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(host.session_call_count(), 4);
    assert_eq!(result.value, Some(value::InterpValue::usize(1)));
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_compacts_runtime_conversation_history() {
    let checked = checked_project(
        r#"
module app.main;
import std.agent.message.Message;
import std.agent.session.Conversation;

flow main(ticket: SessionId, limit: Limit) -> Conversation ![Memory.write<SessionId>] {
  let session = SessionConfig {
    id = ticket,
    context = SummaryPlusRecent(1),
    retention = Days(90),
    compaction = SummarizeWhen(limit),
  };
  let _message = Message.with_session(Message.new("hello"), session);
  let conversation = Conversation.compact(session);
  return conversation;
}
"#,
    );

    let host = FakeHost::new(availability(&[HostRequirementKind::DurableMemory]));
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            vec![
                value::InterpValue::String("ticket-compact".to_owned()),
                value::InterpValue::Variant {
                    name: "ContextTokens".to_owned(),
                    fields: vec![value::InterpValue::i32(100)],
                },
            ],
            &host,
            RunOptions::default(),
        )
        .await;

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(host.session_call_count(), 4);
    let Some(value::InterpValue::Conversation(conversation)) = result.value else {
        panic!("expected conversation, got {:?}", result.value);
    };
    let Some(summary) = conversation.summary else {
        panic!("expected compacted conversation summary");
    };
    assert_eq!(conversation.session, "ticket-compact");
    assert_eq!(summary.message_count, 1);
    assert!(!summary.text.is_empty());
    assert!(result.events.iter().any(|event| {
        matches!(
            event,
            WorkflowEvent::SessionCompacted {
                session,
                summary_message_count: 1,
            } if session == "ticket-compact"
        )
    }));
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_records_message_handoff_to_agent_stage() {
    let checked = checked_project(
        r#"
module app.main;
import std.agent.message.Message;
import std.agent.prompt.Prompt;

agent Writer(input: Message<string>) -> string {
  return Prompt.new().user(Public(input.content));
}

flow main(ticket: string, input: string) -> string {
  let message = Message.with_session(
    Message.new(input),
    SessionConfig.continue_or_new(ticket)
  );
  return message ~> Writer;
}
"#,
    );

    let host = FakeHost::new(availability(&[HostRequirementKind::Agentic]));
    host.seed_model_response_text("handoff-ok");
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            vec![
                value::InterpValue::String("ticket-42".to_owned()),
                value::InterpValue::String("hello".to_owned()),
            ],
            &host,
            RunOptions::default(),
        )
        .await;

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(host.session_call_count(), 2);
    assert_eq!(
        result.value,
        Some(value::InterpValue::String("handoff-ok".to_owned()))
    );
    assert!(result.events.iter().any(|event| {
        matches!(
            event,
            WorkflowEvent::MessageHandoff {
                id,
                session: Some(session),
                ..
            } if id == "msg-0"
                && session == "continue_or_new:ticket-42"
        )
    }));
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_rejects_raw_string_as_session_config_value() {
    let checked = checked_project(
        r#"
module app.main;
import std.agent.message.Message;

flow main(input: string, session: SessionConfig) -> Message<string> {
  let message = Message.new(input);
  return Message.with_session(message, session);
}
"#,
    );

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            vec![
                value::InterpValue::String("hello".to_owned()),
                value::InterpValue::String("session-should-not-be-accepted".to_owned()),
            ],
            &FakeHost::new(HostServiceAvailability::default()),
            RunOptions::default(),
        )
        .await;

    assert_eq!(result.value, None);
    assert!(result.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("Message.with_session expects a SessionConfig value")
    }));
}

fn is_unix_timestamp(value: &str) -> bool {
    value.parse::<u64>().is_ok_and(|timestamp| timestamp > 0)
}
