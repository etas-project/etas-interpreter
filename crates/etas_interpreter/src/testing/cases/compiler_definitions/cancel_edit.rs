use super::*;
use etas_code::syntax::CodeSyntaxOperations;
use std::sync::atomic::{AtomicU64, Ordering};

struct EditClock {
    now: std::time::Instant,
    reads: AtomicU64,
    stop_at: Option<u64>,
    expire_at: Option<u64>,
    control: std::sync::Mutex<Option<api::RunControl>>,
}

impl etas_host::MonotonicClock for EditClock {
    fn now(&self) -> std::time::Instant {
        let read = self.reads.fetch_add(1, Ordering::SeqCst);
        if self.stop_at == Some(read) {
            self.control
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .stop(etas_host::execution::CancellationReason::Requested)
                .unwrap();
        }
        if self.expire_at.is_some_and(|threshold| read >= threshold) {
            self.now + std::time::Duration::from_millis(60_001)
        } else {
            self.now
        }
    }
}

async fn edit(methods: usize, stop_at: Option<u64>, expire_at: Option<u64>) -> (RunResult, u64) {
    let methods = (0..methods)
        .map(|index| format!("flow method_{index}() -> i32 ![] {{ return 1; }}\n"))
        .collect::<String>();
    let source = format!(
        r#"
module app.main;
import std.compiler.SyntaxBlock;
agent Worker() -> i32 ![] {{ return 1; }}
impl agent Worker {{ {methods} }}
flow main(syntax: SyntaxBlock) -> bool {{
    let draft = (^Worker).with_body(syntax);
    return true;
}}
"#
    );
    let (checked, definitions) = prepared(&source, &["app.main.Worker"]);
    let backend = definitions.definitions().syntax_operations().clone();
    let syntax = backend.parse_body("return 2;").unwrap();
    let ty = checked.types.known_std_types.code.syntax_block.unwrap();
    let argument =
        InterpValue::SyntaxBlock(crate::value::SyntaxBlockValue::new(ty, syntax, backend));
    let clock = Arc::new(EditClock {
        now: std::time::Instant::now(),
        reads: AtomicU64::new(0),
        stop_at,
        expire_at,
        control: std::sync::Mutex::new(None),
    });
    let budget = etas_host::ExecutionBudget::start_with_clock(
        etas_host::Budget {
            time: Some(etas_host::TimeBudget { max_millis: 60_000 }),
            ..Default::default()
        },
        clock.clone(),
    );
    let host = FakeHost::new(availability(&[]));
    let invocation = Interpreter.create_run(
        &checked,
        EntryPoint {
            item: checked.entry.unwrap(),
        },
        vec![argument],
        &host,
        RunOptions {
            code_definitions: Some(definitions),
            host_context: api::HostExecutionContext {
                budget,
                ..Default::default()
            },
            ..Default::default()
        },
    );
    *clock.control.lock().unwrap() = Some(invocation.control());
    let result = invocation.execute().await.unwrap();
    (result, clock.reads.load(Ordering::SeqCst))
}

#[tokio::test(flavor = "current_thread")]
async fn agent_definition_edit_preserves_cancellation_and_deadline() {
    let (small, small_reads) = edit(1, None, None).await;
    assert_eq!(small.value(), Some(&InterpValue::Bool(true)));
    let (large, large_reads) = edit(64, None, None).await;
    assert_eq!(large.value(), Some(&InterpValue::Bool(true)));
    assert!(
        large_reads >= small_reads + 50,
        "{small_reads} vs {large_reads}"
    );
    let (cancelled, reads) = edit(64, Some(small_reads + 2), None).await;
    assert!(reads > small_reads + 2);
    assert!(
        matches!(cancelled.outcome, api::RunOutcome::Cancelled(_)),
        "{:?}",
        cancelled.outcome
    );
    assert!(cancelled.value().is_none());
    assert!(
        cancelled.diagnostics.is_empty(),
        "{:?}",
        cancelled.diagnostics
    );

    let (expired, reads) = edit(64, None, Some(small_reads + 2)).await;
    assert!(reads > small_reads + 2);
    assert!(expired.value().is_none());
    assert!(!matches!(expired.outcome, api::RunOutcome::Cancelled(_)));
    assert!(
        expired
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("execution time budget")),
        "{:?}",
        expired.diagnostics
    );
}
