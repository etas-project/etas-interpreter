use super::*;

#[tokio::test(flavor = "current_thread")]
async fn prepared_revision_resume_uses_current_policy_and_not_prior_authority() {
    let base = frozen(
        r#"
module app;
import std.runtime.checkpoint;
import std.io.println;
agent Worker() -> i32 { checkpoint("base"); println("original")?; return 1; }
flow inspect() -> unit { let definition = ^Worker; return; }
"#,
    );
    let revision = InterpreterCandidatePublisher
        .publish(compile(
            base,
            "checkpoint(\"candidate\"); println(\"revised\")?; return 2;",
        ))
        .unwrap();
    let first_host = crate::testing::FakeHost::new(crate::host::HostServiceAvailability {
        checkpoint: true,
        console: true,
        ..Default::default()
    });
    let mut options = crate::api::RunOptions::default();
    options
        .host_context
        .authority
        .grants
        .push(etas_host::HostActionGrant::allow("Console", "stdout_write"));
    let first = revision
        .create_run(vec![], &first_host, options)
        .execute()
        .await
        .unwrap();
    assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);
    assert_eq!(first.value(), Some(&crate::value::InterpValue::i32(2)));
    assert_eq!(first_host.console_call_count(), 1);
    assert_eq!(first.checkpoints.len(), 1);

    let current_host = crate::testing::FakeHost::new(crate::host::HostServiceAvailability {
        checkpoint: true,
        console: true,
        host_authority: true,
        ..Default::default()
    });
    current_host.deny_policy("revoked after restart");
    let mut options = crate::api::RunOptions::default();
    options.host_context.authority.policy.boundary_policy_ref =
        Some(etas_host::HostValue::String("current-policy".into()));
    let result = revision
        .create_resume(&first.checkpoints[0], &current_host, options)
        .execute()
        .await
        .unwrap();
    assert!(result.value().is_none());
    assert!(
        result
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("revoked after restart")),
        "{:?}",
        result.diagnostics
    );
    assert_eq!(current_host.console_call_count(), 0);
    let requests = current_host.policy_requests();
    assert_eq!(requests.len(), 1);
    assert!(
        requests[0].authority.grants.is_empty(),
        "previous Console grant must not survive restore"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn checkpoint_arguments_rebind_to_the_current_checked_revisions() {
    use crate::{
        api::{RunOptions, codec},
        control::CallTarget,
        value::{AgentRevisionValue, InterpValue},
    };
    let source = r#"
module app;
import std.runtime.checkpoint;
agent Worker() -> i32 { return 1; }
flow main(value: Worker) -> i32 {
    checkpoint("before");
    let output = value();
    checkpoint("after");
    return output;
}
flow inspect() -> unit { let definition = ^Worker; return; }
"#;
    let prepare = || {
        let base = frozen(source);
        let checked = base.checked().clone();
        let revision = InterpreterCandidatePublisher
            .publish(compile(base, "return 9;"))
            .unwrap();
        (checked, revision)
    };
    let (artifact, memory) = {
        let (checked, revision) = prepare();
        let value = AgentRevisionValue::new_in_program(
            revision.clone(),
            None,
            &checked,
            revision.targets.component,
        )
        .unwrap();
        let host = crate::testing::FakeHost::new(crate::host::HostServiceAvailability {
            checkpoint: true,
            ..Default::default()
        });
        let result = Interpreter
            .run_checked(
                &checked,
                EntryPoint {
                    item: checked.entry.unwrap(),
                },
                vec![InterpValue::Callable(CallTarget::AgentRevision(value))],
                &host,
                RunOptions::default(),
            )
            .await
            .unwrap();
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        assert_eq!(result.value(), Some(&InterpValue::i32(9)));
        assert_eq!(result.checkpoints.len(), 2);
        let memory = result.checkpoints[0].clone();
        let artifact = codec::checkpoint_artifact_json(&[], "main", &memory).unwrap();
        (
            codec::checkpoint_file_to_bytes(artifact, Default::default()).unwrap(),
            memory,
        )
    };
    let (checked, revision) = prepare();
    let mut revisions = crate::api::code::PreparedRevisionSet::new(checked.clone());
    revisions.insert(revision.clone()).unwrap();
    let revisions = Arc::new(revisions);
    let document = codec::checkpoint_file_from_bytes(&artifact, Default::default()).unwrap();
    assert!(
        codec::checkpoint_from_json(&document, &checked)
            .unwrap_err()
            .message()
            .contains("requires freshly prepared revisions")
    );
    let decoded = codec::checkpoint_from_json_with_revisions(
        &Default::default(),
        &document,
        &checked,
        Some(&revisions),
    )
    .unwrap();
    for checkpoint in [&memory, &decoded] {
        let host = crate::testing::FakeHost::new(crate::host::HostServiceAvailability {
            checkpoint: true,
            ..Default::default()
        });
        let result = Interpreter
            .resume_checkpoint(
                &checked,
                checkpoint,
                &host,
                RunOptions {
                    checkpoint_revisions: Some(revisions.clone()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        assert_eq!(result.value(), Some(&InterpValue::i32(9)));
        assert_eq!(result.checkpoints.len(), 1);
        assert_eq!(result.checkpoints[0].label.as_deref(), Some("after"));
        let mut restore = crate::eval::machine::snapshot::RestoreContext::for_program(
            checked.clone(),
            Some(revisions.clone()),
        );
        let argument = result.checkpoints[0].args[0]
            .clone()
            .restore_with(&mut restore)
            .unwrap();
        let InterpValue::Callable(CallTarget::AgentRevision(argument)) = &argument else {
            panic!("restored Agent argument")
        };
        assert!(Arc::ptr_eq(argument.prepared(), &revision));
        assert!(Arc::ptr_eq(argument.source_program(), &checked));
    }
}
