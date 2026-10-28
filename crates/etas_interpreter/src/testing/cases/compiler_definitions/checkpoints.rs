use super::*;

#[tokio::test(flavor = "current_thread")]
async fn pending_compile_handler_cannot_checkpoint_a_live_definition_as_a_replay_permit() {
    let (checked, definitions) = prepared(
        r#"
module app.main;
import std.compiler.compile;
import std.runtime.checkpoint;
agent Worker() -> i32 { return 7; }
flow main() -> i32 {
    let candidate = handle {
        compile(^Worker)
    } with {
        Code.compile(definition) => {
            checkpoint("pending compile");
            let identity = definition.identity;
            resume Worker;
        }
    };
    return candidate();
}
"#,
        &[],
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
            RunOptions {
                code_definitions: Some(definitions),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(result.value().is_none());
    assert!(result.checkpoints.is_empty());
    assert!(
        result.diagnostics.iter().any(|diagnostic| {
            diagnostic.code
                == etas_core::DiagnosticCode::Analysis(
                    etas_core::AnalysisDiagnosticCode::UnhandledRuntimeError,
                )
                && diagnostic.message.contains("locked definition environment")
        }),
        "{:?}",
        result.diagnostics
    );
    assert!(!result.events.iter().any(|event| matches!(
        event,
        WorkflowEvent::HostTrace(etas_host::TraceEvent::HostRequestStarted {
            kind: etas_host::HostRequestKind::Compiler,
            ..
        })
    )));
}
