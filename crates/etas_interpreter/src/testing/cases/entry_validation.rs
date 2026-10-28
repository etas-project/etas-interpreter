use super::super::*;

#[tokio::test(flavor = "current_thread")]
async fn checked_entry_enum_results_preserve_the_constructor_owner_identity() {
    let checked = checked_project(
        "module app.main; enum First { Same(i32), } enum Second { Same(i32), } flow main() -> First { return First.Same(7); }",
    );
    let entry = checked.entry.unwrap();
    let Some(etas_types::ItemSignature::Flow(signature)) =
        checked.types.item_signatures.get(&entry)
    else {
        panic!("checked flow")
    };
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint { item: entry },
            vec![],
            &FakeHost::new(availability(&[])),
            RunOptions::default(),
        )
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert!(
        matches!(result.value(), Some(InterpValue::Nominal { ty, .. }) if *ty == signature.output),
        "source enum lost its checked owner identity: {:?}",
        result.value()
    );
}

#[tokio::test(flavor = "current_thread")]
async fn checked_entry_checkpoint_arguments_use_the_same_value_type_contract() {
    let checked = checked_project(
        "module app.main; import std.runtime.checkpoint; flow main(value: i32) -> i32 { checkpoint(\"typed-entry\"); return value; }",
    );
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            vec![InterpValue::i32(7)],
            &FakeHost::new(availability(&[])),
            RunOptions::default(),
        )
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let checkpoint = result.checkpoints.first().expect("real checkpoint");
    let mut artifact =
        crate::api::codec::checkpoint_artifact_json(&[], "main", checkpoint).unwrap();
    artifact["checkpoint"]["args"][0] = serde_json::json!({"kind":"bool","value":true});
    assert!(
        crate::api::codec::checkpoint_from_json(&artifact, &checked).is_err(),
        "checkpoint argument changed from i32 to bool without rejection"
    );
}

async fn rejected_argument(
    declarations: &str,
    ty: &str,
    argument: InterpValue,
) -> Result<(), String> {
    let checked = checked_project(&format!(
        "module app.main; {declarations} flow main(value: {ty}) -> unit {{ return; }}"
    ));
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            vec![argument],
            &FakeHost::new(availability(&[])),
            RunOptions::default(),
        )
        .await
        .unwrap();
    if result.value().is_some() {
        return Err(format!(
            "invalid {ty} argument was accepted: {:?}",
            result.value()
        ));
    }
    if !result.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == DiagnosticCode::Analysis(AnalysisDiagnosticCode::InvalidArguments)
    }) {
        return Err(format!("{ty}: {:?}", result.diagnostics));
    }
    if !result.events.is_empty() {
        return Err(format!("invalid {ty} argument executed HIR"));
    }
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn checked_entry_rejects_numeric_width_and_aggregate_payload_mismatches() {
    let mut failures = Vec::new();
    for (ty, argument) in [
        ("i32", InterpValue::u8(7)),
        ("u8", InterpValue::i32(7)),
        ("char", InterpValue::String("two".into())),
        (
            "Array<i32>",
            InterpValue::Array(vec![InterpValue::Bool(true)].into()),
        ),
        (
            "Array<i32>",
            InterpValue::List(vec![InterpValue::i32(7)].into()),
        ),
        (
            "List<i32>",
            InterpValue::List(vec![InterpValue::String("bad".into())].into()),
        ),
        (
            "Map<string, i32>",
            InterpValue::Map(
                vec![(InterpValue::String("key".into()), InterpValue::Bool(true))].into(),
            ),
        ),
        (
            "(i32, bool)",
            InterpValue::Tuple(vec![InterpValue::Bool(true), InterpValue::i32(7)].into()),
        ),
        (
            "Option<i32>",
            InterpValue::OptionSome(crate::value::SharedValue::new(InterpValue::Bool(true))),
        ),
        (
            "Result<i32, string>",
            InterpValue::Variant {
                name: "Ok".into(),
                fields: vec![InterpValue::Bool(true)].into(),
            },
        ),
        (
            "Result<i32, string>",
            InterpValue::Variant {
                name: "Unknown".into(),
                fields: vec![InterpValue::i32(7)].into(),
            },
        ),
        (
            "{ x: i32, y: bool }",
            InterpValue::Record(
                vec![
                    ("x".into(), InterpValue::i32(1)),
                    ("x".into(), InterpValue::Bool(true)),
                ]
                .into(),
            ),
        ),
    ] {
        if let Err(failure) = rejected_argument("", ty, argument).await {
            failures.push(failure);
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[tokio::test(flavor = "current_thread")]
async fn checked_entry_preserves_nominal_and_trust_contracts() {
    let probes = [
        rejected_argument(
            "type UserId = string;",
            "UserId",
            InterpValue::String("raw".into()),
        )
        .await,
        rejected_argument(
            "",
            "Trusted<i32>",
            InterpValue::Trust {
                wrapper: etas_types::TrustWrapper::Untrusted,
                value: crate::value::SharedValue::new(InterpValue::i32(7)),
            },
        )
        .await,
        rejected_argument(
            "",
            "Trusted<i32>",
            InterpValue::Trust {
                wrapper: etas_types::TrustWrapper::Trusted,
                value: crate::value::SharedValue::new(InterpValue::Bool(true)),
            },
        )
        .await,
    ];
    let failures = probes
        .into_iter()
        .filter_map(Result::err)
        .collect::<Vec<_>>();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[tokio::test(flavor = "current_thread")]
async fn checked_entry_rejects_arguments_before_host_dispatch() {
    let checked = checked_project(
        "module app.main; import std.io.println; flow main(value: i32) -> unit { println(\"must-not-run\"); return; }",
    );
    let host = FakeHost::new(availability(&[HostRequirementKind::Console]));
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            vec![InterpValue::Bool(true)],
            &host,
            RunOptions::default(),
        )
        .await
        .unwrap();
    assert!(
        result.value().is_none(),
        "invalid argument executed the effectful entry: Console calls={}, stdout={:?}",
        host.console_call_count(),
        host.stdout_text()
    );
    assert!(
        result.diagnostics.iter().any(|diagnostic| diagnostic.code
            == DiagnosticCode::Analysis(AnalysisDiagnosticCode::InvalidArguments)),
        "{:?}",
        result.diagnostics
    );
    assert_eq!(host.console_call_count(), 0);
    assert_eq!(host.stdout_text(), "");
    assert!(result.events.is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn checked_entry_valid_primitive_and_container_controls_remain_supported() {
    for (ty, argument) in [
        ("i32", InterpValue::i32(7)),
        ("u8", InterpValue::u8(7)),
        ("char", InterpValue::String("x".into())),
        (
            "Array<i32>",
            InterpValue::Array(vec![InterpValue::i32(7)].into()),
        ),
        (
            "List<i32>",
            InterpValue::List(vec![InterpValue::i32(7)].into()),
        ),
        (
            "Map<string, i32>",
            InterpValue::Map(vec![(InterpValue::String("key".into()), InterpValue::i32(7))].into()),
        ),
        (
            "(i32, bool)",
            InterpValue::Tuple(vec![InterpValue::i32(7), InterpValue::Bool(true)].into()),
        ),
        ("Option<i32>", InterpValue::OptionNone),
        (
            "Result<i32, string>",
            InterpValue::Variant {
                name: "Ok".into(),
                fields: vec![InterpValue::i32(7)].into(),
            },
        ),
    ] {
        let checked = checked_project(&format!(
            "module app.main; flow main(value: {ty}) -> unit {{ return; }}"
        ));
        let result = Interpreter
            .run_checked(
                &checked,
                EntryPoint {
                    item: checked.entry.unwrap(),
                },
                vec![argument],
                &FakeHost::new(availability(&[])),
                RunOptions::default(),
            )
            .await
            .unwrap();
        assert!(
            result.diagnostics.is_empty(),
            "{ty}: {:?}",
            result.diagnostics
        );
        assert_eq!(result.value(), Some(&InterpValue::Unit), "{ty}");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn checked_entry_rejects_wrong_primitive_values_before_executing_hir() {
    let checked =
        checked_project("module app.main; flow main(value: i32) -> i32 { return value; }");
    let host = FakeHost::new(availability(&[]));
    for value in [
        InterpValue::Bool(true),
        InterpValue::String("not an integer".into()),
        InterpValue::Unit,
    ] {
        let result = Interpreter
            .run_checked(
                &checked,
                EntryPoint {
                    item: checked.entry.unwrap(),
                },
                vec![value],
                &host,
                RunOptions::default(),
            )
            .await
            .unwrap();
        assert!(
            result.value().is_none(),
            "wrong entry value was accepted: {:?}",
            result.value()
        );
        assert!(
            result.diagnostics.iter().any(|diagnostic| diagnostic.code
                == DiagnosticCode::Analysis(AnalysisDiagnosticCode::InvalidArguments)),
            "{:?}",
            result.diagnostics
        );
        assert!(
            result.events.is_empty(),
            "rejected arguments must not execute any HIR"
        );
    }
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            vec![InterpValue::i32(7)],
            &host,
            RunOptions::default(),
        )
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&InterpValue::i32(7)));
}
