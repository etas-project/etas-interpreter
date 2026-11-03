use super::*;
use crate::{
    api::{InterpValue, RunOptions},
    control::CallTarget,
    value::AgentRevisionValue,
};

#[tokio::test(flavor = "current_thread")]
async fn callable_roundtrips_do_not_accumulate_routes_or_retain_unrelated_revisions() {
    let mut source = String::from(
        r#"
module app;
agent Worker() -> i32 { return 1; }
impl agent Worker {
    flow make(value: i32) -> () -> i32 ![] { return () => value; }
    flow forward(action: () -> i32 ![]) -> () -> i32 ![] { return action; }
}
flow main(first: Worker, second: Worker) -> () -> i32 ![] {
    let f0 = first.make(42);
"#,
    );
    for index in 1..=32 {
        source.push_str(&format!("let f{index} = second.forward(f{});\n", index - 1));
    }
    source.push_str("return f32; } flow inspect() -> unit { let definition = ^Worker; return; }");
    let base = frozen(&source);
    let first = InterpreterCandidatePublisher
        .publish(compile(base.clone(), "return 2;"))
        .unwrap();
    let second = InterpreterCandidatePublisher
        .publish(compile(base, "return 3;"))
        .unwrap();
    let retained = Arc::downgrade(&first);
    let unrelated = Arc::downgrade(&second);
    let result = run_many(vec![first, second], false).await;
    let Some(InterpValue::Callable(CallTarget::Scoped(value))) = result.value() else {
        panic!("native callable result");
    };
    assert_eq!(value.projection_count(), 1);
    assert!(
        retained.upgrade().is_some(),
        "a live closure must pin its revision"
    );
    assert!(
        unrelated.upgrade().is_none(),
        "a reversed projection must not pin an unrelated revision"
    );
    drop(result);
    assert!(
        retained.upgrade().is_none(),
        "dropping the last closure must release its revision"
    );
}

async fn run(prepared: Arc<PreparedCodeRevision>) -> crate::api::RunResult {
    run_many(vec![prepared], false).await
}

#[tokio::test(flavor = "current_thread")]
async fn a_standard_pure_callable_crosses_into_a_revision_with_its_checked_abi() {
    let base = frozen(
        r#"
module app;
import std.text.uppercase;
agent Worker() -> i32 { return 1; }
impl agent Worker {
    flow apply(action: (string) -> string ![], text: string) -> string { return action(text); }
}

flow main(worker: Worker) -> string { return worker.apply(uppercase, "revision"); }
flow inspect() -> unit { let definition = ^Worker; return; }
"#,
    );
    let prepared = InterpreterCandidatePublisher
        .publish(compile(base, "return 2;"))
        .unwrap();
    let result = run(prepared).await;
    assert_eq!(
        result.value(),
        Some(&InterpValue::String("REVISION".into()))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_standard_codec_callback_preserves_nominal_payloads_through_both_program_boundaries() {
    let base = frozen(
        r#"
module app;
import std.http.codec.{decode_response_head, HttpWireResponseHead, HttpCodecError};
import std.codec.text.utf8_encode;
agent Worker() -> i32 { return 1; }
impl agent Worker {
    flow decode(action: (bytes) -> Result<HttpWireResponseHead, HttpCodecError> ![])
        -> Result<HttpWireResponseHead, HttpCodecError> {
        return action(utf8_encode("HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n"));
    }
}
flow main(worker: Worker) -> Result<HttpWireResponseHead, HttpCodecError> {
    return worker.decode(decode_response_head);
}
flow inspect() -> unit { let definition = ^Worker; return; }
"#,
    );
    let etas_types::ItemSignature::Flow(main) =
        &base.checked().types.item_signatures[&base.checked().entry.unwrap()]
    else {
        panic!("main signature");
    };
    let Some(etas_types::Type::Result { ok, .. }) = base.checked().type_store.get(main.output)
    else {
        panic!("checked Result type");
    };
    let expected_head = *ok;
    let prepared = InterpreterCandidatePublisher
        .publish(compile(base, "return 2;"))
        .unwrap();
    let result = run(prepared).await;
    let Some(InterpValue::Variant { name, fields }) = result.value() else {
        panic!("checked Result value: {:?}", result.value());
    };
    assert_eq!(name.as_str(), "Ok");
    let [InterpValue::Nominal { ty, value }] = fields.as_slice() else {
        panic!("nominal HTTP response head: {fields:?}");
    };
    assert_eq!(*ty, expected_head);
    let InterpValue::Record(fields) = value.as_ref() else {
        panic!("response head representation");
    };
    assert_eq!(fields.get("status"), Some(InterpValue::i32(200)));
}

#[tokio::test(flavor = "current_thread")]
async fn a_revision_can_return_a_standard_callable_without_losing_its_execution_plan() {
    let base = frozen(
        r#"
module app;
import std.text.uppercase;
agent Worker() -> i32 { return 1; }
impl agent Worker {
    flow make() -> (string) -> string ![] { return uppercase; }
}
flow main(worker: Worker) -> string { let action = worker.make(); return action("retained"); }
flow inspect() -> unit { let definition = ^Worker; return; }
"#,
    );
    let prepared = InterpreterCandidatePublisher
        .publish(compile(base, "return 2;"))
        .unwrap();
    let result = run(prepared).await;
    assert_eq!(
        result.value(),
        Some(&InterpValue::String("RETAINED".into()))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_standard_host_callable_crosses_a_revision_and_suspends_in_the_same_invocation() {
    let base = frozen(
        r#"
module app;
import std.io.println;
agent Worker() -> i32 { return 1; }
impl agent Worker {
    flow apply(action: (string) -> unit ![Error<IOError>]) -> unit
        ![Error<IOError>] { action("callback"); }
}
flow main(worker: Worker) -> i32 { worker.apply(println)?; return 42; }
flow inspect() -> unit { let definition = ^Worker; return; }
"#,
    );
    let prepared = InterpreterCandidatePublisher
        .publish(compile(base, "return 2;"))
        .unwrap();
    let result = run_many(vec![prepared], true).await;
    assert_eq!(result.value(), Some(&InterpValue::i32(42)));
}

#[tokio::test(flavor = "current_thread")]
async fn entering_scoped_callbacks_does_not_reset_the_invocation_step_budget() {
    let base = frozen(
        r#"
module app;
agent Worker() -> i32 { return 1; }
impl agent Worker { flow apply(action: () -> i32 ![]) -> i32 { return action(); } }
flow main(worker: Worker) -> i32 {
    var value = 0;
    while value < 1000 limit Iterations(1000) { value = value + worker.apply(() => 1); }
    return value;
}
flow inspect() -> unit { let definition = ^Worker; return; }
"#,
    );
    let prepared = InterpreterCandidatePublisher
        .publish(compile(base, "return 2;"))
        .unwrap();
    let original = prepared.type_bridge().original_program().clone();
    let agent = AgentRevisionValue::new_in_program(
        prepared.clone(),
        None,
        &original,
        prepared.targets.agent_component().unwrap(),
    )
    .unwrap();
    let host = crate::testing::FakeHost::new(Default::default());
    let mut options = RunOptions::default();
    options.execution_limits.max_steps = std::num::NonZeroU64::new(128);
    let result = Interpreter
        .run_checked(
            &original,
            EntryPoint {
                item: original.entry.unwrap(),
            },
            vec![InterpValue::Callable(CallTarget::AgentRevision(agent))],
            &host,
            options,
        )
        .await
        .unwrap();
    assert!(result.value().is_none());
    assert!(
        result.diagnostics.iter().any(|diagnostic| diagnostic
            .message
            .contains("maximum interpreter execution steps (128)")),
        "{:?}",
        result.diagnostics
    );
}

async fn run_many(
    revisions: Vec<Arc<PreparedCodeRevision>>,
    console: bool,
) -> crate::api::RunResult {
    let prepared = &revisions[0];
    let original = prepared.type_bridge().original_program().clone();
    let args = revisions
        .iter()
        .map(|prepared| {
            let agent = AgentRevisionValue::new_in_program(
                prepared.clone(),
                None,
                &original,
                prepared.targets.agent_component().unwrap(),
            )
            .unwrap();
            InterpValue::Callable(CallTarget::AgentRevision(agent))
        })
        .collect();
    let host = crate::testing::FakeHost::new(crate::host::HostServiceAvailability {
        console,
        ..Default::default()
    });
    let mut options = RunOptions::default();
    if console {
        options
            .host_context
            .authority
            .grants
            .push(etas_host::HostActionGrant::allow("Console", "stdout_write"));
    }
    let result = Interpreter
        .run_checked(
            &original,
            EntryPoint {
                item: original.entry.unwrap(),
            },
            args,
            &host,
            options,
        )
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(host.console_call_count(), usize::from(console));
    if console {
        assert_eq!(host.stdout_text(), "callback\n");
    }
    result
}

#[tokio::test(flavor = "current_thread")]
async fn a_scoped_callback_resumes_after_a_host_boundary_in_the_same_invocation() {
    let base = frozen(
        r#"
module app;
import std.io.println;
agent Worker() -> i32 { return 1; }
impl agent Worker {
    flow apply(f: () -> i32 ![]) -> i32 { return f() + 1; }
}
flow main(worker: Worker) -> i32 {
    let captured = 40;
    return worker.apply(() => { println("callback")?; return captured + 1; });
}
flow inspect() -> unit { let definition = ^Worker; return; }
"#,
    );
    let prepared = InterpreterCandidatePublisher
        .publish(compile(base, "return 2;"))
        .unwrap();
    let result = run_many(vec![prepared], true).await;
    assert_eq!(result.value(), Some(&InterpValue::i32(42)));
}

#[tokio::test(flavor = "current_thread")]
async fn a_returned_revision_closure_preserves_frozen_declaration_bindings() {
    let base = frozen(
        r#"
module app;
agent Worker() -> i32 { return 1; }
impl agent Worker {
    flow make() -> () -> i32 ![] { return () => Worker(); }
}
flow main(worker: Worker) -> i32 { let action = worker.make(); return action(); }
flow inspect() -> unit { let definition = ^Worker; return; }
"#,
    );
    let prepared = InterpreterCandidatePublisher
        .publish(compile(base, "return 2;"))
        .unwrap();
    let result = run(prepared).await;
    assert_eq!(result.value(), Some(&InterpValue::i32(1)));
}

#[tokio::test(flavor = "current_thread")]
async fn forwarding_a_closure_through_another_revision_does_not_rebind_its_captures() {
    let base = frozen(
        r#"
module app;
agent Worker() -> i32 { return 1; }
impl agent Worker {
    flow make(value: i32) -> () -> i32 ![] { return () => value; }
    flow forward(action: () -> i32 ![]) -> () -> i32 ![] { return action; }
}
flow main(first: Worker, second: Worker) -> i32 {
    let action = first.make(21);
    let forwarded = second.forward(action);
    return action() + forwarded();
}
flow inspect() -> unit { let definition = ^Worker; return; }
"#,
    );
    let first = InterpreterCandidatePublisher
        .publish(compile(base.clone(), "return 2;"))
        .unwrap();
    let second = InterpreterCandidatePublisher
        .publish(compile(base, "return 3;"))
        .unwrap();
    let result = run_many(vec![first, second], false).await;
    assert_eq!(result.value(), Some(&InterpValue::i32(42)));
}

#[tokio::test(flavor = "current_thread")]
async fn original_closure_arguments_execute_in_their_own_program_with_nominal_values() {
    let base = frozen(
        r#"
module app;
type UserId = string;
agent Worker() -> i32 { return 1; }
impl agent Worker {
    flow apply(f: (UserId) -> UserId ![], value: UserId) -> UserId { return f(value); }
}
flow main(worker: Worker) -> UserId {
    let captured = UserId("original capture");
    return worker.apply((value: UserId) => { return captured; }, UserId("argument"));
}
flow inspect() -> unit { let definition = ^Worker; return; }
"#,
    );
    let original_type = match &base.checked().types.item_signatures[&base.checked().entry.unwrap()]
    {
        etas_types::ItemSignature::Flow(flow) => flow.output,
        _ => panic!("main signature"),
    };
    let prepared = InterpreterCandidatePublisher
        .publish(compile(base, "return 2;"))
        .unwrap();
    let result = run(prepared).await;
    let Some(InterpValue::Nominal { ty, value }) = result.value() else {
        panic!("nominal result: {:?}", result.value());
    };
    assert_eq!(*ty, original_type);
    assert_eq!(
        value.as_ref(),
        &InterpValue::String("original capture".into())
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_closure_returned_by_a_revision_keeps_its_program_and_captured_nominal_values() {
    let base = frozen(
        r#"
module app;
type UserId = string;
agent Worker() -> i32 { return 1; }
impl agent Worker {
    flow make(value: UserId) -> () -> UserId ![] { return () => value; }
}
flow main(worker: Worker) -> UserId {
    let action = worker.make(UserId("revision capture"));
    return action();
}
flow inspect() -> unit { let definition = ^Worker; return; }
"#,
    );
    let original_type = match &base.checked().types.item_signatures[&base.checked().entry.unwrap()]
    {
        etas_types::ItemSignature::Flow(flow) => flow.output,
        _ => panic!("main signature"),
    };
    let prepared = InterpreterCandidatePublisher
        .publish(compile(base, "return 2;"))
        .unwrap();
    let result = run(prepared).await;
    let Some(InterpValue::Nominal { ty, value }) = result.value() else {
        panic!("nominal result: {:?}", result.value());
    };
    assert_eq!(*ty, original_type);
    assert_eq!(
        value.as_ref(),
        &InterpValue::String("revision capture".into())
    );
}
