use super::super::*;

fn output_type(checked: &etas_frontend::CheckedProject) -> etas_types::TypeId {
    let etas_types::ItemSignature::Flow(signature) =
        &checked.types.item_signatures[&checked.entry.unwrap()]
    else {
        panic!("flow signature");
    };
    signature.output
}

#[tokio::test(flavor = "current_thread")]
async fn a_standard_payload_constructor_is_an_ordinary_callable_value() {
    let checked = checked_project(
        r#"
module app.main;
import std.stream.StreamRead;
flow main(input: bytes) -> StreamRead {
    let constructor = StreamRead.Data;
    return constructor(input);
}
"#,
    );
    let host = FakeHost::new(HostServiceAvailability::default());
    let payload = value::InterpValue::Bytes(b"constructed".to_vec().into());
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            vec![payload.clone()],
            &host,
            RunOptions::default(),
        )
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.value(),
        Some(&value::InterpValue::Nominal {
            ty: output_type(&checked),
            value: value::InterpValue::Variant {
                name: "Data".to_owned().into(),
                fields: vec![payload].into()
            }
            .into()
        })
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_stream_handler_can_resume_with_the_declared_data_or_eof_constructor() {
    for (expression, name, fields) in [
        (
            "StreamRead.Data(input)",
            "Data",
            vec![value::InterpValue::Bytes(b"handled".to_vec().into())],
        ),
        ("StreamRead.Eof", "Eof", vec![]),
    ] {
        let checked = checked_project(&format!(
            r#"
module app.main;
import std.stream.{{StreamRead, ByteStream, read, Timeout}};
import std.effects.Stream;
import std.option.None;
type Buffer = string;
impl Buffer ~ ByteStream;
flow main(input: bytes) -> StreamRead ![] {{
    return handle {{ read(Buffer("source stream"), 16, None<Timeout>()) }} with {{
        Stream.read<Buffer>(_) => {{ resume {expression}; }}
    }};
}}
"#
        ));
        let host = FakeHost::new(HostServiceAvailability::default());
        let result = Interpreter
            .run_checked(
                &checked,
                EntryPoint {
                    item: checked.entry.unwrap(),
                },
                vec![value::InterpValue::Bytes(b"handled".to_vec().into())],
                &host,
                RunOptions::default(),
            )
            .await
            .unwrap();
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        assert_eq!(
            result.value(),
            Some(&value::InterpValue::Nominal {
                ty: output_type(&checked),
                value: value::InterpValue::Variant {
                    name: name.to_owned().into(),
                    fields: fields.into()
                }
                .into()
            })
        );
        assert!(host.stream_requests().is_empty());
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_stream_result_retains_its_checked_enum_layout_across_checkpoint_resume() {
    let checked = checked_project(
        r#"
module app.main;
import std.stream.StreamRead;
import std.runtime.checkpoint;
flow main(input: bytes) -> StreamRead {
    let value = StreamRead.Data(input);
    checkpoint("stream value");
    return value;
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
            vec![value::InterpValue::Bytes(b"saved".to_vec().into())],
            &host,
            RunOptions::default(),
        )
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let checkpoint = result.checkpoints.first().unwrap();
    let restored = Interpreter
        .resume_checkpoint(&checked, checkpoint, &host, RunOptions::default())
        .await
        .unwrap();
    assert!(
        restored.diagnostics.is_empty(),
        "{:?}",
        restored.diagnostics
    );
    assert_eq!(restored.value(), result.value());
}
