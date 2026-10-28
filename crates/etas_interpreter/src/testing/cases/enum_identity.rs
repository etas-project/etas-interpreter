use super::super::*;

#[tokio::test(flavor = "current_thread")]
async fn generic_record_constructors_materialize_identity_before_checkpoint() {
    let checked = checked_project(
        r#"
module app.main;
import std.runtime.checkpoint;
type Box<T> = { value: T }
flow boxed<T>(value: T) -> Box<T> { return Box<T> { value }; }
flow main() -> (Box<i32>, Box<bool>) {
    let first = boxed<i32>(7);
    let second = boxed<bool>(true);
    checkpoint("record identities");
    return (first, second);
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
            vec![],
            &host,
            RunOptions::default(),
        )
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let etas_types::ItemSignature::Flow(signature) =
        &checked.types.item_signatures[&checked.entry.unwrap()]
    else {
        panic!("signature");
    };
    let Some(etas_types::Type::Tuple(expected)) = checked.type_store.get(signature.output) else {
        panic!("tuple type");
    };
    let Some(InterpValue::Tuple(values)) = result.value() else {
        panic!("tuple value");
    };
    for (value, expected) in values.iter().zip(expected) {
        assert!(
            matches!(value, InterpValue::Nominal {ty, ..} if ty == expected),
            "{value:?}: {expected:?}"
        );
    }
    let checkpoint = result.checkpoints.first().unwrap();
    let artifact = crate::api::codec::checkpoint_artifact_json(&[], "main", checkpoint).unwrap();
    let restored = crate::api::codec::checkpoint_from_json(&artifact, &checked).unwrap();
    let resumed = Interpreter
        .resume_checkpoint(&checked, &restored, &host, RunOptions::default())
        .await
        .unwrap();
    assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
    assert_eq!(resumed.value(), result.value());
}

#[tokio::test(flavor = "current_thread")]
async fn generic_nominal_call_constructor_keeps_the_callers_specialization() {
    let checked = checked_project(
        r#"
module app.main;
import std.runtime.checkpoint;
type Tagged<T> = T;
flow tagged<T>(value: T) -> Tagged<T> { return Tagged<T>(value); }
flow main() -> (Tagged<i32>, Tagged<bool>) {
    let first = tagged<i32>(7);
    let second = tagged<bool>(true);
    checkpoint("nominal call identities");
    return (first, second);
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
            vec![],
            &host,
            RunOptions::default(),
        )
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let etas_types::ItemSignature::Flow(signature) =
        &checked.types.item_signatures[&checked.entry.unwrap()]
    else {
        panic!("signature");
    };
    let Some(etas_types::Type::Tuple(expected)) = checked.type_store.get(signature.output) else {
        panic!("tuple type");
    };
    let Some(InterpValue::Tuple(values)) = result.value() else {
        panic!("tuple value");
    };
    for (value, expected) in values.iter().zip(expected) {
        assert!(
            matches!(value, InterpValue::Nominal {ty, ..} if ty == expected),
            "{value:?}: {expected:?}"
        );
    }
    let artifact =
        crate::api::codec::checkpoint_artifact_json(&[], "main", &result.checkpoints[0]).unwrap();
    let checkpoint = crate::api::codec::checkpoint_from_json(&artifact, &checked).unwrap();
    let resumed = Interpreter
        .resume_checkpoint(&checked, &checkpoint, &host, RunOptions::default())
        .await
        .unwrap();
    assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
    assert_eq!(resumed.value(), result.value());
}

#[tokio::test(flavor = "current_thread")]
async fn generic_enum_constructors_keep_applied_identity_after_call_and_checkpoint() {
    let checked = checked_project(
        r#"
module app.main;
import std.runtime.checkpoint;
enum Choice<T> { Empty, Positional(T), Named { value: T }, }
flow named<T>(value: T) -> Choice<T> { return Choice.Named { value }; }
flow positional<T>(value: T) -> Choice<T> {
    let constructor: T -> Choice<T> = Choice.Positional;
    return constructor(value);
}
flow empty<T>() -> Choice<T> { return Choice.Empty; }
flow main() -> (Choice<i32>, Choice<bool>, Choice<i32>, Choice<bool>, Choice<i32>) {
    let first = named<i32>(7);
    let second = positional<bool>(true);
    let third = empty<i32>();
    let fourth = empty<bool>();
    let fifth = Choice.Positional(9);
    checkpoint("enum identities");
    return (first, second, third, fourth, fifth);
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
            vec![],
            &host,
            RunOptions::default(),
        )
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let etas_types::ItemSignature::Flow(signature) =
        &checked.types.item_signatures[&checked.entry.unwrap()]
    else {
        panic!("signature");
    };
    let Some(etas_types::Type::Tuple(expected)) = checked.type_store.get(signature.output) else {
        panic!("tuple type");
    };
    let Some(InterpValue::Tuple(values)) = result.value() else {
        panic!("tuple value: {:?}", result.value());
    };
    for (value, expected) in values.iter().zip(expected) {
        assert!(
            matches!(value, InterpValue::Nominal { ty, .. } if ty == expected),
            "{value:?}: {expected:?}"
        );
    }
    assert_ne!(expected[2], expected[3]);
    let checkpoint = result.checkpoints.first().unwrap();
    let artifact = crate::api::codec::checkpoint_artifact_json(&[], "main", checkpoint).unwrap();
    let restored = crate::api::codec::checkpoint_from_json(&artifact, &checked).unwrap();
    let resumed = Interpreter
        .resume_checkpoint(&checked, &restored, &host, RunOptions::default())
        .await
        .unwrap();
    assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
    assert_eq!(resumed.value(), result.value());
}

#[tokio::test(flavor = "current_thread")]
async fn checkpoint_rejects_an_enum_local_with_its_owner_envelope_removed() {
    let checked = checked_project(
        r#"
module app.main;
import std.runtime.checkpoint;
enum Response { Failure { code: i32 }, }
flow main() -> i32 {
    let response = Response.Failure { code = 503 };
    checkpoint("enum local");
    return match response { Response.Failure { code } => code };
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
            vec![],
            &host,
            RunOptions::default(),
        )
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&InterpValue::i32(503)));
    let checkpoint = result.checkpoints.first().unwrap();
    let mut artifact =
        crate::api::codec::checkpoint_artifact_json(&[], "main", checkpoint).unwrap();
    fn strip(value: &mut serde_json::Value) -> usize {
        if value["kind"] == "nominal"
            && value["value"]["kind"] == "variant"
            && value["value"]["name"] == "Failure"
        {
            *value = value["value"].take();
            return 1;
        }
        match value {
            serde_json::Value::Object(fields) => fields.values_mut().map(strip).sum(),
            serde_json::Value::Array(values) => values.iter_mut().map(strip).sum(),
            _ => 0,
        }
    }
    assert!(strip(&mut artifact) > 0);
    assert!(
        crate::api::codec::checkpoint_from_json(&artifact, &checked).is_err(),
        "ownerless enum local restored"
    );
}
