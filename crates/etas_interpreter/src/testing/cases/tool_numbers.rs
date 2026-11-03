use super::super::*;

fn options() -> RunOptions {
    let mut options = RunOptions::default();
    options
        .host_context
        .authority
        .grants
        .push(HostActionGrant::allow("Agentic", "infer"));
    options.model_policy.provider_capabilities = Some(full_model_capabilities());
    options
}

#[tokio::test(flavor = "current_thread")]
async fn source_tools_accept_integral_json_numbers_as_checked_floats() {
    for (ty, values) in [
        ("f64", vec![90.0, 90.5, -0.0, 1e300]),
        ("f32", vec![90.0, 90.5, -0.0]),
    ] {
        let checked = checked_project(&format!(
            r#"
module app.main;
tool echo(value: {ty}) -> {ty} ![] {{ return value; }}
@tools([echo])
agent Reader() -> string ![] {{ return perform infer(Prompt.new()); }}
flow main() -> string ![Agentic] {{ return Reader.run(); }}
"#
        ));
        for value in values {
            let host = FakeHost::new(availability(&[HostRequirementKind::Agentic]));
            host.seed_model_response_tool_call_with_args(
                "echo",
                HostValue::Json(etas_host::HostJsonValue::Object(vec![(
                    "value".into(),
                    etas_host::HostJsonValue::Number(value),
                )])),
            );
            host.seed_model_response_text("finished");
            let result = Interpreter
                .run_checked(
                    &checked,
                    EntryPoint {
                        item: checked.entry.unwrap(),
                    },
                    vec![],
                    &host,
                    options(),
                )
                .await
                .unwrap();
            assert!(
                result.diagnostics.is_empty(),
                "{ty}/{value}: {:?}",
                result.diagnostics
            );
            assert_eq!(
                result.value(),
                Some(&InterpValue::String("finished".into()))
            );
            let requests = host.model_requests();
            assert_eq!(requests.len(), 2);
            let tools: Vec<_> = requests[1]
                .messages
                .iter()
                .filter(|message| message.role == ModelRole::Tool)
                .collect();
            assert_eq!(tools.len(), 1);
            assert_eq!(tools[0].tool_call_id.as_deref(), Some("call-1"));
            let [ModelContent::Value(HostValue::Float(actual))] = tools[0].content.as_slice()
            else {
                panic!("checked float tool result: {:?}", tools[0])
            };
            assert_eq!(actual.to_bits(), value.to_bits());
            assert!(
                host.tool_requests().is_empty(),
                "source tool must not use external tool dispatch"
            );
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn source_tools_reject_wrong_json_numeric_types_before_entering_the_body() {
    for (ty, number) in [("i32", 1e30), ("u8", 256.0), ("f32", 1e300)] {
        let checked = checked_project(&format!(
            r#"
module app.main;
import std.io.println;
tool echo(value: {ty}) -> {ty} ![Error<IOError>] {{ println("entered"); return value; }}
@tools([echo])
agent Reader() -> string ![Error<IOError>] {{ return perform infer(Prompt.new()); }}
flow main() -> string ![Agentic, Error<IOError>] {{ return Reader.run(); }}
"#
        ));
        let host = FakeHost::new(availability(&[
            HostRequirementKind::Agentic,
            HostRequirementKind::Console,
        ]));
        host.seed_model_response_tool_call_with_args(
            "echo",
            HostValue::Json(etas_host::HostJsonValue::Object(vec![(
                "value".into(),
                etas_host::HostJsonValue::Number(number),
            )])),
        );
        host.seed_model_response_text("must not run");
        let result = Interpreter
            .run_checked(
                &checked,
                EntryPoint {
                    item: checked.entry.unwrap(),
                },
                vec![],
                &host,
                options(),
            )
            .await
            .unwrap();
        assert!(!result.diagnostics.is_empty(), "{ty}/{number}");
        assert_eq!(host.model_call_count(), 1);
        assert!(host.stdout_text().is_empty());
    }
}
