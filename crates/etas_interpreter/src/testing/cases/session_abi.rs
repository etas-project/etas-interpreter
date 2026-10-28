use super::super::*;

#[tokio::test(flavor = "current_thread")]
async fn checked_session_constructors_preserve_canonical_identity_across_checkpoint() {
    let checked = checked_project(
        r#"
module app.main;
import std.runtime.checkpoint;
flow main() -> (SessionId, SessionConfig, ContextPolicy, RetentionPolicy, Limit) {
    let id = SessionId("ticket");
    let configured = SessionConfig { id, context = LastTurns(8), retention = Days(90) };
    let continued = SessionConfig.continue_or_new("key");
    let context = SummaryPlusRecent(4);
    let retention = Days(30);
    let limit = Tokens(200);
    checkpoint("session ABI");
    if continued.id != SessionId("continue_or_new:key") { abort("session identity changed"); }
    return (id, configured, context, retention, limit);
}
"#,
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
    let Some(InterpValue::Tuple(values)) = result.value() else {
        panic!("tuple");
    };
    for (value, role) in values.iter().zip([
        etas_std::RuntimeTypeRole::SessionId,
        etas_std::RuntimeTypeRole::SessionConfig,
        etas_std::RuntimeTypeRole::ContextPolicy,
        etas_std::RuntimeTypeRole::RetentionPolicy,
        etas_std::RuntimeTypeRole::Limit,
    ]) {
        let expected = checked.types.known_std_types.runtime_type(role).unwrap();
        assert!(
            matches!(value, InterpValue::Nominal {ty, ..} if *ty == expected),
            "{role:?}: {value:?}"
        );
    }
    let artifact =
        crate::api::codec::checkpoint_artifact_json(&[], "main", &result.checkpoints[0]).unwrap();
    let checkpoint = crate::api::codec::checkpoint_from_json(&artifact, &checked).unwrap();
    let resumed = Interpreter
        .resume_checkpoint(&checked, &checkpoint, &host, RunOptions::default())
        .await
        .unwrap();
    assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
    assert_eq!(resumed.value(), result.value());
}
