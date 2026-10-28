use super::super::*;
use crate::api::codec::{checkpoint_artifact_json, checkpoint_from_json};

async fn assert_checkpointed_call(
    source: &str,
    expected: InterpValue,
    stdout: &str,
) -> (
    etas_frontend::CheckedProject,
    crate::api::codec::JsonDocument,
) {
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
            vec![],
            &host,
            RunOptions::default(),
        )
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&expected));
    assert_eq!(host.stdout_text(), stdout);
    assert_eq!(host.console_call_count(), usize::from(!stdout.is_empty()));
    assert_eq!(result.checkpoints.len(), 1);

    let artifact =
        checkpoint_artifact_json(&["main.es".into()], "main", &result.checkpoints[0]).unwrap();
    let decoded = checkpoint_from_json(&artifact, &checked).unwrap();
    let resumed_host = FakeHost::new(services);
    let resumed = Interpreter
        .resume_checkpoint(&checked, &decoded, &resumed_host, RunOptions::default())
        .await
        .unwrap();
    assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
    assert_eq!(resumed.value(), Some(&expected));
    assert_eq!(resumed_host.stdout_text(), stdout);
    assert_eq!(
        resumed_host.console_call_count(),
        usize::from(!stdout.is_empty())
    );
    (checked, artifact)
}

#[tokio::test(flavor = "current_thread")]
async fn generic_std_constructor_alias_preserves_nominal_payload_after_checkpoint() {
    assert_checkpointed_call(
        r#"
module app.main;
import std.runtime.checkpoint;
type UserId = string;
flow wrap<T>(value: T) -> Option<T> {
    let constructor: (T) -> Option<T> ![] = Some;
    checkpoint("inside-owner");
    return constructor(value);
}
flow main() -> bool {
    let id = UserId("payload");
    return match wrap(id) { Some(value) => value == id, None => false, };
}
"#,
        InterpValue::Bool(true),
        "",
    )
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn returned_generic_std_constructor_keeps_owner_bindings_across_checkpoint() {
    assert_checkpointed_call(
        r#"
module app.main;
import std.runtime.checkpoint;
type UserId = string;
flow factory<T>() -> ((T) -> Option<T> ![]) { return Some; }
flow main() -> bool {
    let constructor = factory<UserId>();
    checkpoint("after-owner");
    let id = UserId("payload");
    return match constructor(id) { Some(value) => value == id, None => false, };
}
"#,
        InterpValue::Bool(true),
        "",
    )
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn returned_result_constructor_keeps_owner_bindings_across_checkpoint() {
    assert_checkpointed_call(
        r#"
module app.main;
import std.runtime.checkpoint;
type UserId = string;
flow factory<T>() -> ((T) -> Result<T, string> ![]) { return Ok; }
flow main() -> bool {
    let constructor = factory<UserId>();
    checkpoint("after-owner");
    let id = UserId("payload");
    return match constructor(id) { Ok(value) => value == id, Err(_) => false, };
}
"#,
        InterpValue::Bool(true),
        "",
    )
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn returned_error_constructor_keeps_owner_bindings_across_checkpoint() {
    assert_checkpointed_call(
        r#"
module app.main;
import std.runtime.checkpoint;
type ErrorCode = string;
flow factory<E>() -> ((E) -> Result<i32, E> ![]) { return Err; }
flow main() -> bool {
    let constructor = factory<ErrorCode>();
    checkpoint("after-owner");
    let code = ErrorCode("failure");
    return match constructor(code) { Err(value) => value == code, Ok(_) => false, };
}
"#,
        InterpValue::Bool(true),
        "",
    )
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn returned_trust_constructor_keeps_owner_bindings_across_checkpoint() {
    assert_checkpointed_call(
        r#"
module app.main;
import std.runtime.checkpoint;
type UserId = string;
flow factory<T>() -> ((T) -> Public<T> ![]) { return Public; }
flow main() -> bool {
    let constructor = factory<UserId>();
    checkpoint("after-owner");
    let id = UserId("payload");
    return constructor(id) == Public(id);
}
"#,
        InterpValue::Bool(true),
        "",
    )
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn std_constructor_captures_effect_row_only_environment_and_callback_origin() {
    assert_checkpointed_call(
        r#"
module app.main;
import std.runtime.checkpoint;
import std.io.println;
flow factory<effect E>() -> ((() -> unit ![E]) -> Option<() -> unit ![E]> ![]) ![] {
    return Some;
}
flow write() -> unit ![Console.stdout_write] { perform Console.stdout_write("payload"); }
flow main() -> i32 {
    let constructor = factory<![Console.stdout_write]>();
    checkpoint("after-row-owner");
    let optional = constructor(write);
    let text = handle {
        match optional { Some(callback) => { callback(); }, None => { return 1; }, }
        "missing payload"
    } with {
        Console.stdout_write(text) => { finish text; }
    };
    println(text);
    return 0;
}
"#,
        InterpValue::i32(0),
        "payload\n",
    )
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn std_callable_checkpoint_rejects_missing_or_rebound_owner_type() {
    let (checked, artifact) = assert_checkpointed_call(
        r#"
module app.main;
import std.runtime.checkpoint;
type UserId = string;
flow factory<T>() -> ((T) -> Option<T> ![]) { return Some; }
flow main() -> bool {
    let constructor = factory<UserId>();
    checkpoint("after-owner");
    let id = UserId("payload");
    return match constructor(id) { Some(value) => value == id, None => false, };
}
"#,
        InterpValue::Bool(true),
        "",
    )
    .await;
    let string = checked
        .type_store
        .iter()
        .find_map(|(ty, kind)| {
            matches!(kind, Type::Primitive(etas_types::PrimitiveType::String)).then_some(ty)
        })
        .unwrap();
    for replacement in [None, Some(string)] {
        let mut malformed = artifact.clone();
        assert!(rewrite_owner_binding(&mut malformed, replacement) > 0);
        checkpoint_from_json(&malformed, &checked)
            .expect_err("std callable must retain its checked owner specialization");
    }
}

fn rewrite_owner_binding(
    value: &mut serde_json::Value,
    replacement: Option<etas_types::TypeId>,
) -> usize {
    let mut changed = 0;
    if value.get("kind").and_then(serde_json::Value::as_str) == Some("specialized")
        && let Some(bindings) = value
            .get_mut("type_bindings")
            .and_then(serde_json::Value::as_array_mut)
        && bindings.iter().any(|binding| binding["name"] == "T")
    {
        if let Some(ty) = replacement {
            for binding in bindings.iter_mut().filter(|binding| binding["name"] == "T") {
                binding["type"] = serde_json::json!(ty.0);
            }
        } else {
            bindings.clear();
        }
        changed += 1;
    }
    match value {
        serde_json::Value::Object(fields) => {
            for field in fields.values_mut() {
                changed += rewrite_owner_binding(field, replacement);
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                changed += rewrite_owner_binding(value, replacement);
            }
        }
        _ => {}
    }
    changed
}
