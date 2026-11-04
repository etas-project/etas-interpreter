use super::super::*;
use crate::api::codec::{checkpoint_artifact_json, checkpoint_from_json};

#[tokio::test(flavor = "current_thread")]
async fn returned_method_closure_keeps_owner_binders_across_wrapper_and_checkpoint_resume() {
    let checked = checked_project(include_str!(
        "../fixtures/method_resolution/captured_owner_binders.es"
    ));
    let services = availability(&[
        HostRequirementKind::Console,
        HostRequirementKind::Checkpoint,
    ]);
    let host = FakeHost::new(services);
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            Vec::new(),
            &host,
            RunOptions::default(),
        )
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&InterpValue::String("changed".into())));
    assert_eq!(host.stdout_text(), "copy\n");
    assert_eq!(host.console_call_count(), 1);
    assert_eq!(result.checkpoints.len(), 3);
    for (checkpoint, expected_output) in result.checkpoints.iter().zip(["copy\n", "copy\n", ""]) {
        let artifact = checkpoint_artifact_json(&["main.es".into()], "main", checkpoint).unwrap();
        let decoded = checkpoint_from_json(&artifact, &checked).unwrap();
        for checkpoint in [checkpoint, &decoded] {
            let host = FakeHost::new(services);
            let resumed = Interpreter
                .resume_checkpoint(&checked, checkpoint, &host, RunOptions::default())
                .await
                .unwrap();
            assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
            assert_eq!(resumed.value(), result.value());
            assert_eq!(host.stdout_text(), expected_output);
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn impl_owner_binders_survive_method_shadowing_and_checkpoint_resume() {
    let checked = checked_project(include_str!(
        "../fixtures/method_resolution/owner_binders.es"
    ));
    let services = availability(&[
        HostRequirementKind::Console,
        HostRequirementKind::Checkpoint,
    ]);
    let host = FakeHost::new(services);
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            Vec::new(),
            &host,
            RunOptions::default(),
        )
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&InterpValue::String("changed".into())));
    assert_eq!(host.stdout_text(), "copy\ncopy\n");
    assert_eq!(host.console_call_count(), 2);
    assert_eq!(result.checkpoints.len(), 3);
    for (checkpoint, expected_output) in result.checkpoints.iter().zip(["copy\n", "", ""]) {
        let artifact = checkpoint_artifact_json(&["main.es".into()], "main", checkpoint).unwrap();
        let decoded = checkpoint_from_json(&artifact, &checked).unwrap();
        for checkpoint in [checkpoint, &decoded] {
            let host = FakeHost::new(services);
            let resumed = Interpreter
                .resume_checkpoint(&checked, checkpoint, &host, RunOptions::default())
                .await
                .unwrap();
            assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
            assert_eq!(resumed.value(), result.value());
            assert_eq!(host.stdout_text(), expected_output);
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn heterogeneous_objects_keep_distinct_witnesses_in_a_restored_iteration() {
    let checked = checked_project(include_str!(
        "../fixtures/method_resolution/existential_sequence.es"
    ));
    let services = availability(&[
        HostRequirementKind::Console,
        HostRequirementKind::Checkpoint,
    ]);
    let host = FakeHost::new(services);
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            Vec::new(),
            &host,
            RunOptions::default(),
        )
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&InterpValue::usize(11)));
    assert_eq!(host.stdout_text(), "first\nsecond\n");
    assert_eq!(host.console_call_count(), 2);
    assert_eq!(result.checkpoints.len(), 2);
    for (checkpoint, expected_output) in result
        .checkpoints
        .iter()
        .zip(["first\nsecond\n", "second\n"])
    {
        let artifact = checkpoint_artifact_json(&["main.es".into()], "main", checkpoint).unwrap();
        let restored = checkpoint_from_json(&artifact, &checked).unwrap();
        let host = FakeHost::new(services);
        let result = Interpreter
            .resume_checkpoint(&checked, &restored, &host, RunOptions::default())
            .await
            .unwrap();
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        assert_eq!(result.value(), Some(&InterpValue::usize(11)));
        assert_eq!(host.stdout_text(), expected_output);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn existential_method_witnesses_survive_producer_and_argument_checkpoints() {
    let source = include_str!("../fixtures/method_resolution/existential.es");
    for source in [
        source.to_owned(),
        source.replace(
            "let value: ? ~ Named = label();",
            "let value = pack(label());",
        ),
    ] {
        let checked = checked_project(&source);
        let services = availability(&[
            HostRequirementKind::Console,
            HostRequirementKind::Checkpoint,
        ]);
        let host = FakeHost::new(services);
        let result = Interpreter
            .run_checked(
                &checked,
                EntryPoint {
                    item: checked.entry.unwrap(),
                },
                Vec::new(),
                &host,
                RunOptions::default(),
            )
            .await
            .unwrap();
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        assert_eq!(result.value(), Some(&InterpValue::usize(6)));
        assert_eq!(host.stdout_text(), "method\n");
        assert_eq!(result.checkpoints.len(), 4);
        for (checkpoint, expected_output) in result
            .checkpoints
            .iter()
            .zip(["method\n", "method\n", "method\n", ""])
        {
            let artifact =
                checkpoint_artifact_json(&["main.es".into()], "main", checkpoint).unwrap();
            let decoded = checkpoint_from_json(&artifact, &checked).unwrap();
            for checkpoint in [checkpoint, &decoded] {
                let host = FakeHost::new(services);
                let resumed = Interpreter
                    .resume_checkpoint(&checked, checkpoint, &host, RunOptions::default())
                    .await
                    .unwrap();
                assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
                assert_eq!(resumed.value(), result.value());
                assert_eq!(host.stdout_text(), expected_output);
            }
        }
        let mut invalid = result.checkpoints[2].clone();
        let mut changed = 0;
        for frame in &mut invalid.machine.frames {
            if let crate::orchestration::MachineFrameSnapshot::Continuation {
                continuation:
                    crate::orchestration::ContinuationSnapshot::CallArgs { evaluated_args, .. },
            } = frame
                && let Some(crate::orchestration::ValueSnapshot::Existential { header, .. }) =
                    evaluated_args.first_mut()
            {
                header.concrete_type = header.object_type;
                changed += 1;
            }
        }
        assert_eq!(changed, 1);
        let artifact = checkpoint_artifact_json(&["main.es".into()], "main", &invalid).unwrap();
        assert!(checkpoint_from_json(&artifact, &checked).is_err());
        let host = FakeHost::new(services);
        let rejected = Interpreter
            .resume_checkpoint(&checked, &invalid, &host, RunOptions::default())
            .await
            .unwrap();
        assert!(
            rejected.diagnostics.iter().any(|diagnostic| diagnostic
                .message
                .contains("checked producer or interface type")),
            "{:?}",
            rejected.diagnostics
        );
        assert_eq!(host.stdout_text(), "");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn method_chains_execute_inferred_callbacks_and_empty_literal_arguments() {
    let source = include_str!("../fixtures/method_resolution/contextual_arguments.es")
        .replace(
            "module app.main;",
            "module app.main; import std.io.println; import std.runtime.checkpoint;",
        )
        .replace(
            "return item.read_value();",
            "println(\"callback\")?; checkpoint(\"callback\"); return item.read_value();",
        );
    let checked = checked_project(&source);
    let services = availability(&[
        HostRequirementKind::Console,
        HostRequirementKind::Checkpoint,
    ]);
    let host = FakeHost::new(services);
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            Vec::new(),
            &host,
            RunOptions::default(),
        )
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&InterpValue::i32(8)));
    assert_eq!(host.stdout_text(), "callback\n");
    assert_eq!(result.checkpoints.len(), 1);
    let artifact =
        checkpoint_artifact_json(&["main.es".into()], "main", &result.checkpoints[0]).unwrap();
    let checkpoint = checkpoint_from_json(&artifact, &checked).unwrap();
    let host = FakeHost::new(services);
    let resumed = Interpreter
        .resume_checkpoint(&checked, &checkpoint, &host, RunOptions::default())
        .await
        .unwrap();
    assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
    assert_eq!(resumed.value(), result.value());
    assert_eq!(host.stdout_text(), "");
}

#[test]
fn checked_method_plan_preserves_the_callers_same_named_effect_row() {
    let checked = checked_project(include_str!("../fixtures/method_resolution/outer_row.es"));
    let plan = Interpreter.plan(&checked, crate::api::PlanOptions);
    assert!(plan.diagnostics.is_empty(), "{:?}", plan.diagnostics);
}

#[tokio::test(flavor = "current_thread")]
async fn spec_method_named_arguments_follow_the_declaration_through_checked_witnesses() {
    let source = include_str!("../fixtures/method_resolution/named_bound.es");
    for copies in [0, 256] {
        let source = source.replace(
            "value.add(amount = 5)",
            &format!("value{}.add(amount = 5)", ".copy()".repeat(copies)),
        );
        let checked = checked_project(&source);
        let host = FakeHost::new(availability(&[]));
        let result = Interpreter
            .run_checked(
                &checked,
                EntryPoint {
                    item: checked.entry.unwrap(),
                },
                Vec::new(),
                &host,
                RunOptions::default(),
            )
            .await
            .unwrap();
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        assert_eq!(result.value(), Some(&InterpValue::i32(22)));
    }
}

#[tokio::test(flavor = "current_thread")]
async fn checked_method_chains_evaluate_receivers_and_named_arguments_once_in_source_order() {
    let source = include_str!("../fixtures/method_resolution/ordered_chain.es");
    for source in [
        source.to_owned(),
        source.replace(".number()", "::Number.number()"),
    ] {
        let checked = checked_project(&source);
        let host = FakeHost::new(availability(&[HostRequirementKind::Console]));
        let result = Interpreter
            .run_checked(
                &checked,
                EntryPoint {
                    item: checked.entry.unwrap(),
                },
                Vec::new(),
                &host,
                RunOptions::default(),
            )
            .await
            .unwrap();
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        assert_eq!(result.value(), Some(&value::InterpValue::i32(30)));
        assert_eq!(
            host.stdout_text(),
            "receiver\nright\nleft\nadvance\nnumber\n"
        );
        assert_eq!(host.console_call_count(), 5);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn checked_method_chains_restore_receiver_and_source_argument_progress() {
    let source = include_str!("../fixtures/method_resolution/suspended_chain.es");
    let checked = checked_project(source);
    let services = availability(&[
        HostRequirementKind::Console,
        HostRequirementKind::Checkpoint,
    ]);
    let host = FakeHost::new(services);
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            Vec::new(),
            &host,
            RunOptions::default(),
        )
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&InterpValue::i32(30)));
    assert_eq!(
        host.stdout_text(),
        "receiver\nright\nleft\nadvance\nnumber\n"
    );
    assert_eq!(result.checkpoints.len(), 3);
    for (checkpoint, output) in result.checkpoints.iter().zip([
        "right\nleft\nadvance\nnumber\n",
        "left\nadvance\nnumber\n",
        "advance\nnumber\n",
    ]) {
        let artifact = checkpoint_artifact_json(&["main.es".into()], "main", checkpoint).unwrap();
        let restored = checkpoint_from_json(&artifact, &checked).unwrap();
        for checkpoint in [checkpoint, &restored] {
            let host = FakeHost::new(services);
            let resumed = Interpreter
                .resume_checkpoint(&checked, checkpoint, &host, RunOptions::default())
                .await
                .unwrap();
            assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
            assert_eq!(resumed.value(), result.value());
            assert_eq!(host.stdout_text(), output);
        }
    }
    let mut invalid = result.checkpoints[1].clone();
    let mut changed = 0;
    for frame in &mut invalid.machine.frames {
        if let crate::orchestration::MachineFrameSnapshot::Continuation {
            continuation:
                crate::orchestration::ContinuationSnapshot::CallArgs {
                    target: crate::orchestration::CallTargetSnapshot::FlowItem(target),
                    ..
                },
        } = frame
        {
            *target = checked.entry.unwrap();
            changed += 1;
        }
    }
    assert_eq!(changed, 1);
    let host = FakeHost::new(services);
    let rejected = Interpreter
        .resume_checkpoint(&checked, &invalid, &host, RunOptions::default())
        .await
        .unwrap();
    assert!(
        rejected.diagnostics.iter().any(|diagnostic| diagnostic
            .message
            .contains("method target differs from its prepared checked selection")),
        "{:?}",
        rejected.diagnostics
    );
    assert_eq!(host.stdout_text(), "");
}
