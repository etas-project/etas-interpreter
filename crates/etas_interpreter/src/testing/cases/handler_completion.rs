use super::super::*;

async fn execute(body: &str, flag: bool) -> RunResult {
    let checked = checked_project(&format!(
        r#"
module app.main;
effect Probe {{ action request() -> i32; }}
flow main(flag: bool) -> i32 ![] {{
    return handle {{ perform Probe.request() }} with {{
        Probe.request() => {{ {body} }}
    }};
}}
"#
    ));
    Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            vec![InterpValue::Bool(flag)],
            &FakeHost::new(availability(&[])),
            RunOptions::default(),
        )
        .await
        .unwrap()
}

#[tokio::test(flavor = "current_thread")]
async fn mixed_finish_and_tail_resume_execute_each_legal_alternative() {
    for (flag, expected) in [(true, 7), (false, 11)] {
        let result = execute("if flag { finish 7; } resume 11;", flag).await;
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        assert_eq!(result.value(), Some(&InterpValue::i32(expected)));
    }
}

#[tokio::test(flavor = "current_thread")]
async fn mixed_abort_and_tail_resume_preserve_abort_and_resumption() {
    for flag in [true, false] {
        let result = execute("if flag { abort(\"stopped\"); } resume 11;", flag).await;
        if flag {
            assert!(result.value().is_none());
            assert!(
                result
                    .diagnostics
                    .iter()
                    .any(|diagnostic| diagnostic.message.contains("stopped")),
                "{:?}",
                result.diagnostics
            );
        } else {
            assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
            assert_eq!(result.value(), Some(&InterpValue::i32(11)));
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn ordinary_resume_and_finish_handler_controls_still_execute() {
    for (body, expected) in [("resume 11;", 11), ("finish 7;", 7)] {
        let result = execute(body, false).await;
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        assert_eq!(result.value(), Some(&InterpValue::i32(expected)));
    }
}
