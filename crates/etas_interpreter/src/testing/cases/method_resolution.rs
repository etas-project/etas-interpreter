use super::super::*;
use crate::api::codec::{checkpoint_artifact_json, checkpoint_from_json};

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
