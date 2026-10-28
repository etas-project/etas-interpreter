use super::super::*;
use crate::api::codec::{checkpoint_artifact_json, checkpoint_from_json};

#[tokio::test(flavor = "current_thread")]
async fn command_result_fields_share_output_storage_and_reject_inconsistent_checked_abi() {
    let checked = checked_project(
        r#"
module app.main;
import std.host.command.CommandResult;
alias ProcessResult = CommandResult;
flow main(value: ProcessResult) -> (bytes, bytes, i32) {
    let alias = value;
    return ((alias).stdout, value.stderr, value.exit_code);
}
"#,
    );
    for size in [1024, 1024 * 1024, 4 * 1024 * 1024] {
        let stdout = crate::value::BytesValue::from(vec![255; size]);
        let stderr = crate::value::BytesValue::from(vec![0, 128, 255]);
        let input = InterpValue::CommandResult {
            exit_code: -1,
            stdout: stdout.clone(),
            stderr: stderr.clone(),
        };
        let (alias, cost) = crate::testing::allocation::measure(|| input.clone());
        assert_eq!(
            cost.count, 0,
            "native command results must not copy output bytes"
        );
        let snapshot = crate::orchestration::ValueSnapshot::capture(&input).unwrap();
        let crate::orchestration::ValueSnapshot::CommandResult { stdout: saved, .. } = &snapshot
        else {
            panic!("native result")
        };
        assert_eq!(saved.as_ptr(), stdout.as_ptr());
        let result = Interpreter
            .run_checked(
                &checked,
                EntryPoint {
                    item: checked.entry.unwrap(),
                },
                vec![alias],
                &FakeHost::new(availability(&[])),
                RunOptions::default(),
            )
            .await
            .unwrap();
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        let Some(InterpValue::Tuple(fields)) = result.value() else {
            panic!("{:?}", result.value())
        };
        let InterpValue::Bytes(out) = &fields[0] else {
            panic!("stdout bytes")
        };
        let InterpValue::Bytes(err) = &fields[1] else {
            panic!("stderr bytes")
        };
        assert_eq!(out.as_ptr(), stdout.as_ptr());
        assert_eq!(err.as_ptr(), stderr.as_ptr());
        assert_eq!(fields[2], InterpValue::i32(-1));
    }
    let mut incomplete = checked.clone();
    let program = std::sync::Arc::make_mut(&mut incomplete);
    let facts = std::sync::Arc::make_mut(&mut program.types);
    let integer = program
        .type_store
        .iter()
        .find_map(|(ty, data)| {
            (*data == etas_types::Type::Primitive(etas_types::PrimitiveType::I32)).then_some(ty)
        })
        .unwrap();
    let projection = facts
        .field_projections
        .values_mut()
        .flat_map(|projections| projections.iter_mut())
        .find(|projection| projection.field == "stdout")
        .unwrap();
    projection.output = integer;
    let rejected = Interpreter.plan(&incomplete, PlanOptions);
    assert!(rejected.plan.is_none());
    assert!(
        rejected
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("native field")),
        "{:?}",
        rejected.diagnostics
    );
}

#[tokio::test(flavor = "current_thread")]
async fn command_result_fields_preserve_host_exit_status_and_exact_bytes_across_resume() {
    let checked = checked_project(
        r#"
module app.main;
import std.host.command.{CommandResult, command, run};
import std.runtime.checkpoint;
flow exit_value(result: CommandResult) -> i32 { return result.exit_code; }
flow main() -> (i32, bytes, bytes) {
    let result = run(command("test-command", []), DefaultCommandSandbox);
    checkpoint("command-result");
    return (exit_value(result), result.stdout, result.stderr);
}
"#,
    );
    let services = availability(&[
        HostRequirementKind::Command,
        HostRequirementKind::Checkpoint,
    ]);
    let host = FakeHost::new(services);
    let stdout = [0, 255, 128, b'\n'];
    let stderr = [0xF0, 0x28, 0x8C, 0x28, 0];
    host.seed_command_output(7, &stdout, &stderr);
    let options = RunOptions {
        host_context: api::HostExecutionContext {
            authority: AuthorityContext {
                grants: vec![HostActionGrant::allow("Command", "run")],
                approvals: Vec::new(),
                sandbox: SandboxPolicy::deny_all(),
                policy: Default::default(),
            },
            ..Default::default()
        },
        ..RunOptions::default()
    };
    let result = Interpreter
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
    let expected = InterpValue::Tuple(
        vec![
            InterpValue::i32(7),
            InterpValue::Bytes(stdout.to_vec().into()),
            InterpValue::Bytes(stderr.to_vec().into()),
        ]
        .into(),
    );
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&expected));
    assert_eq!(host.command_call_count(), 1);
    assert_eq!(result.checkpoints.len(), 1);
    let checkpoint = &result.checkpoints[0];
    let artifact = checkpoint_artifact_json(&["main.es".into()], "main", checkpoint).unwrap();
    let decoded = checkpoint_from_json(&artifact, &checked).unwrap();
    for checkpoint in [checkpoint, &decoded] {
        let fresh = FakeHost::new(services);
        let resumed = Interpreter
            .resume_checkpoint(&checked, checkpoint, &fresh, options.clone())
            .await
            .unwrap();
        assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
        assert_eq!(resumed.value(), Some(&expected));
        assert_eq!(
            fresh.command_call_count(),
            0,
            "resume must not replay the command"
        );
    }
}
