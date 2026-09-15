use crate::{
    api::codec::{self, CheckpointDocument},
    testing::allocation::measure,
    value::InterpValue as V,
};
use serde_json::json;

#[test]
fn aggregate_wire_tags_and_child_order_are_preserved() {
    let children = vec![V::Bool(true), V::String("second".into())];
    let wire_children = json!([{"kind":"bool","value":true},{"kind":"string","value":"second"}]);
    let sequences = [
        ("tuple", V::Tuple(children.clone().into())),
        ("array", V::Array(children.clone().into())),
        ("list", V::List(children.clone().into())),
        ("slice", V::Slice(children.clone().into())),
        ("set", V::Set(children.clone().into())),
        ("deque", V::Deque(children.clone().into())),
        ("queue", V::Queue(children.clone().into())),
        ("stack", V::Stack(children.clone().into())),
        ("ordered_set", V::OrderedSet(children.into())),
    ];
    for (kind, value) in sequences {
        let wire = codec::value_json(&value);
        assert_eq!(wire, json!({"kind":kind,"values":wire_children}));
        assert_eq!(codec::value_from_json(&wire).unwrap(), value);
    }
    let entries = vec![(V::String("key".into()), V::OptionSome(V::Bool(true).into()))];
    for (kind, key, value) in [
        ("map", "key", V::Map(entries.clone().into())),
        ("ordered_map", "key", V::OrderedMap(entries.clone().into())),
        (
            "priority_queue",
            "priority",
            V::PriorityQueue(entries.into()),
        ),
    ] {
        let wire = codec::value_json(&value);
        assert_eq!(
            wire,
            json!({"kind":kind,"entries":[{key:{"kind":"string","value":"key"},"value":{"kind":"option_some","value":{"kind":"bool","value":true}}}]})
        );
        assert_eq!(codec::value_from_json(&wire).unwrap(), value);
    }
    let value = V::Nominal {
        ty: etas_types::TypeId(17),
        value: V::Variant {
            name: "Named".into(),
            fields: vec![V::Record(
                vec![("z".into(), V::Bool(true)), ("a".into(), V::OptionNone)].into(),
            )]
            .into(),
        }
        .into(),
    };
    assert_eq!(
        codec::value_json(&value),
        json!({"kind":"nominal","ty":17,"value":{"kind":"variant","name":"Named","fields":[{"kind":"record","fields":[{"name":"z","value":{"kind":"bool","value":true}},{"name":"a","value":{"kind":"option_none"}}]}]}})
    );
}

#[test]
fn conversation_encodes_borrowed_message_contract_without_changing_the_wire() {
    use crate::value::{ConversationValue, MessageRoleValue, MessageValue, ProvenanceValue};
    let message = MessageValue {
        id: "m1".into(),
        from: Some("a".into()),
        to: Some("b".into()),
        role: MessageRoleValue::User,
        session: Some("s".into()),
        created_at: "step-1".into(),
        payload: Box::new(V::Array(vec![V::Bool(true)].into())),
        provenance: Some(ProvenanceValue {
            trace_id: Some("trace".into()),
            source: Some("source".into()),
        }),
    };
    let expected = json!({"kind":"message","id":"m1","from":"a","to":"b","role":"user","session":"s","created_at":"step-1","payload":{"kind":"array","values":[{"kind":"bool","value":true}]},"provenance":{"trace_id":"trace","source":"source"}});
    assert_eq!(codec::value_json(&V::Message(message.clone())), expected);
    let value = V::Conversation(ConversationValue {
        selected_context: None,
        history_fence: None,
        session: "s".into(),
        messages: vec![message],
        cursor: Some("next".into()),
    });
    let wire = codec::value_json(&value);
    assert_eq!(
        wire,
        json!({"kind":"conversation","selected_context":null,"history_fence":null,"session":"s","messages":[expected],"cursor":"next"})
    );
    assert_eq!(codec::value_from_json(&wire).unwrap(), value);
}

#[tokio::test(flavor = "current_thread")]
async fn checked_recursive_adt_report_and_file_resume_preserve_the_value() {
    use crate::api::{EntryPoint, RunOptions};
    use crate::testing::{FakeHost, project::checked_project};
    let checked = checked_project(
        r#"
module app.main;
import std.runtime.checkpoint;
enum Link { End, Next { child: Link } }
flow main() -> Link {
    var chain = Link.End;
    var count: i32 = 0;
    while count < 1000 limit Iterations(2000) {
        chain = Link.Next { child = chain };
        count = count + 1;
    }
    checkpoint("encoded-adt");
    return chain;
}
"#,
    );
    let host = FakeHost::new(crate::host::HostServiceAvailability::with_host(
        etas_effects::HostRequirementKind::Checkpoint,
    ));
    let result = crate::Interpreter
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
    let value = result.value().unwrap();
    let report = CheckpointDocument::from_value(
        codec::run_report_json("run", &[], "main", &result).unwrap(),
    );
    assert_eq!(&codec::value_from_json(&report["value"]).unwrap(), value);
    let bytes = codec::checkpoint_file_to_bytes(
        codec::checkpoint_artifact_json(&[], "main", &result.checkpoints[0]).unwrap(),
        codec::CheckpointFileLimits::default(),
    )
    .unwrap();
    let document =
        codec::checkpoint_file_from_bytes(&bytes, codec::CheckpointFileLimits::default()).unwrap();
    let checkpoint = codec::checkpoint_from_json(&document, &checked).unwrap();
    let resumed = crate::Interpreter
        .resume_checkpoint(&checked, &checkpoint, &host, RunOptions::default())
        .await
        .unwrap();
    assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
    assert_eq!(resumed.value(), Some(value));
}

fn chain(depth: usize) -> V {
    (0..depth).fold(V::String("x".repeat(1024).into()), |child, _| V::Nominal {
        ty: etas_types::TypeId(17),
        value: V::Variant {
            name: "Link".into(),
            fields: vec![V::Array(
                vec![V::Record(vec![("child".into(), child)].into())].into(),
            )]
            .into(),
        }
        .into(),
    })
}

#[test]
fn runtime_aggregate_encoding_does_not_reencode_descendants() {
    for depth in [16, 32, 64] {
        let value = chain(depth);
        let (wire, cost) = measure(|| CheckpointDocument::from_value(codec::value_json(&value)));
        eprintln!("runtime aggregate depth={depth}: {cost:?}");
        assert!(cost.count <= depth * 50 + 64, "{cost:?}");
        assert!(cost.bytes <= depth * 6000 + 8192, "{cost:?}");
        assert_eq!(codec::value_from_json(&wire).unwrap(), value);
    }
}

#[test]
fn runtime_aggregate_encoding_is_stack_safe() {
    const WORKER: &str = "ETAS_RUNTIME_AGGREGATE_CODEC_WORKER";
    if std::env::var_os(WORKER).is_none() {
        let thread = std::thread::current();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", thread.name().unwrap(), "--nocapture"])
            .env(WORKER, "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let value = chain(30_000);
    let wire = CheckpointDocument::from_value(codec::value_json(&value));
    assert_eq!(codec::value_from_json(&wire).unwrap(), value);
}
