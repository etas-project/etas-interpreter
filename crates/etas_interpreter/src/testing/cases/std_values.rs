use super::super::*;
use crate::api::codec::{checkpoint_artifact_json, checkpoint_from_json};

#[tokio::test(flavor = "current_thread")]
async fn standard_constants_preserve_checked_identity_through_checkpoint_and_resume() {
    let checked = checked_project(
        r#"
module app.main;
import std.host.sandbox.{SandboxProfile, DefaultCommandSandbox as Profile};
import std.runtime.trace.{TraceLabel, Logical};
import std.runtime.checkpoint;
flow main() -> (SandboxProfile, TraceLabel) {
    let profile = Profile;
    let label = Logical;
    checkpoint("standard-values");
    return (profile, label);
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
        panic!("{:?}", result.value());
    };
    for (value, role) in values.iter().zip([
        etas_std::StdValueRole::DefaultCommandSandbox,
        etas_std::StdValueRole::LogicalTraceLabel,
    ]) {
        let InterpValue::StdConstant(value) = value else {
            panic!("native standard constant");
        };
        assert_eq!(value.role(), role);
        assert_eq!(
            Some(value.ty()),
            checked.types.known_std_types.runtime_value(role)
        );
    }
    assert_eq!(result.checkpoints.len(), 1);
    let artifact =
        checkpoint_artifact_json(&["main.es".into()], "main", &result.checkpoints[0]).unwrap();
    let decoded = checkpoint_from_json(&artifact, &checked).unwrap();
    let resumed = Interpreter
        .resume_checkpoint(&checked, &decoded, &host, RunOptions::default())
        .await
        .unwrap();
    assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
    assert_eq!(resumed.value(), result.value());

    fn tamper(value: &mut serde_json::Value, mode: u8) -> usize {
        if value.get("kind").and_then(|value| value.as_str()) == Some("std_constant") {
            match mode {
                0 => value["ty"] = serde_json::json!(u32::MAX),
                1 => value["role"] = serde_json::json!("unknown-role"),
                2 => value["role"] = serde_json::json!("logical-trace-label"),
                _ => {
                    *value = serde_json::json!({"kind":"variant", "name":"DefaultCommandSandbox", "fields":[]})
                }
            }
            return 1;
        }
        match value {
            serde_json::Value::Array(values) => {
                values.iter_mut().map(|value| tamper(value, mode)).sum()
            }
            serde_json::Value::Object(fields) => {
                fields.values_mut().map(|value| tamper(value, mode)).sum()
            }
            _ => 0,
        }
    }
    for mode in 0..4 {
        let mut malformed = artifact.clone();
        assert!(tamper(&mut malformed, mode) >= 2);
        assert!(
            checkpoint_from_json(&malformed, &checked).is_err(),
            "accepted altered constant mode {mode}"
        );
    }
}
