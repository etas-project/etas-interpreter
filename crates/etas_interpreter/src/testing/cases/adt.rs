use super::super::*;

async fn run(source: &str, expected: InterpValue) {
    let checked = checked_project(source);
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            vec![],
            &FakeHost::new(HostServiceAvailability::default()),
            RunOptions::default(),
        )
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&expected));
}

#[tokio::test(flavor = "current_thread")]
async fn directly_recursive_generic_record_remains_nominal_and_finite() {
    run(
        r#"
module app.main;
type Chain<T> = { value: T, next: Option<Chain<T>> }
flow sum(chain: Chain<i32>) -> i32 {
    return chain.value + match chain.next { Some(next) => sum(next), None => 0 };
}
flow main() -> i32 {
    let end = Chain<i32> { value = 3, next = None };
    let head = Chain<i32> { value = 2, next = Some(end) };
    return sum(head);
}
"#,
        InterpValue::i32(5),
    )
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn recursive_enum_tree_constructs_and_traverses() {
    run(r#"
module app.main;
enum Tree<T> { Leaf(T), Branch(Tree<T>, Tree<T>), }
flow sum(tree: Tree<i32>) -> i32 {
    return match tree {
        Tree.Leaf(value) => value,
        Tree.Branch(left, right) => sum(left) + sum(right),
    };
}
flow main() -> i32 { return sum(Tree.Branch(Tree.Leaf(2), Tree.Branch(Tree.Leaf(3), Tree.Leaf(5)))); }
"#, InterpValue::i32(10)).await;
}

#[tokio::test(flavor = "current_thread")]
async fn named_enum_fields_construct_match_and_shorthand() {
    run(
        r#"
module app.main;
enum Response { Success(string), Failure { code: i32, message: string }, }
flow main() -> string {
    let message = "unavailable";
    let value = Response.Failure { message, code = 503 };
    return match value {
        Response.Success(text) => text,
        Response.Failure { code: 503, message } => message,
        Response.Failure { code: _, message: _ } => "other",
    };
}
"#,
        InterpValue::String("unavailable".into()),
    )
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn mutually_recursive_record_enum_and_forward_names() {
    run(r#"
module app.main;
flow main() -> i32 {
    let child = Directory { name = "child", entries = [] };
    let root = Directory { name = "root", entries = [Entry.Subdirectory(child), Entry.File { name = "a", content = "data" }] };
    return count(root);
}
type Directory = { name: string, entries: Array<Entry> }
enum Entry { File { name: string, content: string }, Subdirectory(Directory), }
flow count(directory: Directory) -> i32 {
    var total = 0;
    for entry in directory.entries limit Iterations(8) {
        total = total + match entry {
            Entry.File { name: _, content: _ } => 1,
            Entry.Subdirectory(child) => 1 + count(child),
        };
    }
    return total;
}
"#, InterpValue::i32(2)).await;
}

#[tokio::test(flavor = "current_thread")]
async fn labeled_positional_and_nullary_enum_remain_distinct() {
    run(
        r#"
module app.main;
enum Decision { Accepted, Rejected(reason: string), }
flow label(value: Decision) -> string {
    return match value { Decision.Accepted => "yes", Decision.Rejected(reason) => reason };
}
flow main() -> string { return label(Decision.Rejected("no")) + label(Decision.Accepted); }
"#,
        InterpValue::String("noyes".into()),
    )
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn user_enum_option_names_do_not_invoke_standard_option_intrinsics() {
    run(r#"
module app.main;
enum Choice { Some(i32), None }
flow label(value: Choice) -> i32 { return match value { Choice.Some(value) => value, Choice.None => 0 }; }
flow main() -> i32 { return label(Choice.Some(7)) + label(Choice.None); }
"#, InterpValue::i32(7)).await;
}

#[tokio::test(flavor = "current_thread")]
async fn checkpoint_preserves_pending_named_variant_and_recursive_value() {
    let checked = checked_project(
        r#"
module app.main;
import std.runtime.checkpoint;
enum Tree { Leaf(i32), Node { child: Tree, value: i32 }, Sibling { child: Tree, value: i32 } }
enum Other { Foreign { child: Tree, value: i32 } }
flow pause() -> i32 { checkpoint("inside-field"); return 3; }
flow sum(tree: Tree) -> i32 { return match tree { Tree.Leaf(n) => n, Tree.Node { child, value } => sum(child) + value, Tree.Sibling { child, value } => sum(child) + value }; }
flow main() -> i32 { return sum(Tree.Node { child = Tree.Leaf(4), value = pause() }); }
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
    assert_eq!(result.value(), Some(&InterpValue::i32(7)));
    let artifact =
        api::codec::checkpoint_artifact_json(&[], "main", &result.checkpoints[0]).unwrap();
    let positional = checked
        .symbols
        .iter()
        .find(|symbol| symbol.name == "Leaf")
        .unwrap()
        .id;
    let wrong_type = checked
        .type_store
        .iter()
        .find_map(|(id, ty)| {
            matches!(
                ty,
                etas_types::Type::Primitive(etas_types::PrimitiveType::I32)
            )
            .then_some(id)
        })
        .unwrap();
    let sibling = checked
        .symbols
        .iter()
        .find(|symbol| symbol.name == "Sibling")
        .unwrap()
        .id;
    let foreign = checked
        .symbols
        .iter()
        .find(|symbol| symbol.name == "Foreign")
        .unwrap()
        .id;
    for mutation in [
        RecordMutation::EraseVariant,
        RecordMutation::Variant(positional),
        RecordMutation::Variant(sibling),
        RecordMutation::Variant(foreign),
        RecordMutation::Type(wrong_type),
        RecordMutation::EraseType,
        RecordMutation::EraseBoth,
    ] {
        let mut tampered = artifact.clone();
        let record = record_field_json(&mut tampered).unwrap();
        mutation.json(record);
        let error = api::codec::checkpoint_from_json(&tampered, &checked).unwrap_err();
        assert!(
            error.to_string().contains("named enum continuation"),
            "{error}"
        );

        let mut checkpoint = result.checkpoints[0].clone();
        let record = record_field_snapshot(&mut checkpoint.machine).unwrap();
        mutation.snapshot(record);
        let rejected = Interpreter
            .resume_checkpoint(&checked, &checkpoint, &host, RunOptions::default())
            .await
            .unwrap();
        assert!(matches!(
            rejected.outcome,
            crate::api::RunOutcome::Failed(crate::api::RunFailure::RestoreRejected { .. })
        ));
        assert!(
            rejected
                .diagnostics
                .iter()
                .any(|d| d.message.contains("named enum continuation")),
            "{:?}",
            rejected.diagnostics
        );
        assert!(rejected.events.is_empty());
    }
    let checkpoint = api::codec::checkpoint_from_json(&artifact, &checked).unwrap();
    let resumed = Interpreter
        .resume_checkpoint(&checked, &checkpoint, &host, RunOptions::default())
        .await
        .unwrap();
    assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
    assert_eq!(resumed.value(), Some(&InterpValue::i32(7)));
}

fn record_field_json(value: &mut serde_json::Value) -> Option<&mut serde_json::Value> {
    if value.get("kind").and_then(serde_json::Value::as_str) == Some("record_field") {
        return Some(value);
    }
    match value {
        serde_json::Value::Object(object) => object.values_mut().find_map(record_field_json),
        serde_json::Value::Array(array) => array.iter_mut().find_map(record_field_json),
        _ => None,
    }
}

enum RecordMutation {
    EraseVariant,
    Variant(etas_hir::SymbolId),
    Type(etas_types::TypeId),
    EraseType,
    EraseBoth,
}

impl RecordMutation {
    fn json(&self, record: &mut serde_json::Value) {
        match self {
            Self::EraseVariant => record["variant_symbol"] = serde_json::Value::Null,
            Self::Variant(symbol) => record["variant_symbol"] = serde_json::json!(symbol.0),
            Self::Type(ty) => record["nominal_type"] = serde_json::json!(ty.0),
            Self::EraseType => record["nominal_type"] = serde_json::Value::Null,
            Self::EraseBoth => {
                record["variant_symbol"] = serde_json::Value::Null;
                record["nominal_type"] = serde_json::Value::Null;
            }
        }
    }

    fn snapshot(&self, record: &mut crate::orchestration::ContinuationSnapshot) {
        let crate::orchestration::ContinuationSnapshot::RecordField {
            nominal_type,
            variant_symbol,
            ..
        } = record
        else {
            panic!("expected record continuation")
        };
        match self {
            Self::EraseVariant => *variant_symbol = None,
            Self::Variant(symbol) => *variant_symbol = Some(*symbol),
            Self::Type(ty) => *nominal_type = Some(*ty),
            Self::EraseType => *nominal_type = None,
            Self::EraseBoth => {
                *nominal_type = None;
                *variant_symbol = None;
            }
        }
    }
}

fn record_field_snapshot(
    machine: &mut crate::orchestration::MachineSnapshot,
) -> Option<&mut crate::orchestration::ContinuationSnapshot> {
    use crate::orchestration::{ContinuationSnapshot as C, MachineFrameSnapshot as F};
    fn find(c: &mut C) -> Option<&mut C> {
        match c {
            C::RecordField { .. } => Some(c),
            C::Chain { inner, outer } => find(inner).or_else(|| find(outer)),
            C::CallBoundary { outer } | C::HandlerDispatch { outer } => find(outer),
            C::HandleBoundary { inner, .. }
            | C::ScopedModelPolicy { inner, .. }
            | C::RestoreModelPolicy { inner, .. } => find(inner),
            _ => None,
        }
    }
    machine.frames.iter_mut().find_map(|frame| match frame {
        F::Block { continuation }
        | F::Expr { continuation }
        | F::Call { continuation, .. }
        | F::Continuation { continuation }
        | F::Handler { continuation }
        | F::Retry { continuation } => find(continuation),
        _ => None,
    })
}

#[tokio::test(flavor = "current_thread")]
async fn checkpoint_restores_plain_and_nominal_record_construction() {
    for construction in ["{ value = pause() }", "Row { value = pause() }"] {
        let checked = checked_project(&format!(
            r#"
module app.main;
import std.runtime.checkpoint;
type Row = {{ value: i32 }}
flow pause() -> i32 {{ checkpoint("field"); return 8; }}
flow main() -> i32 {{ let row = {construction}; return row.value; }}
"#
        ));
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
        assert_eq!(result.value(), Some(&InterpValue::i32(8)));
        let artifact =
            api::codec::checkpoint_artifact_json(&[], "main", &result.checkpoints[0]).unwrap();
        let checkpoint = api::codec::checkpoint_from_json(&artifact, &checked).unwrap();
        let resumed = Interpreter
            .resume_checkpoint(&checked, &checkpoint, &host, RunOptions::default())
            .await
            .unwrap();
        assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
        assert_eq!(resumed.value(), Some(&InterpValue::i32(8)));
    }
}

#[tokio::test(flavor = "current_thread")]
async fn unsupported_recursive_model_schema_is_rejected_before_dispatch() {
    let checked = checked_project(
        r#"
module app.main;
import std.agent.prompt.Prompt;
enum Plan { Task(string), Retry(Plan, u32) }
@model(model = "local-qwen")
agent Planner(input: string) -> Plan ![] { return Prompt.new().user(Public(input)); }
flow main() -> Plan { return Planner.run("plan"); }
"#,
    );
    let host = FakeHost::new(availability(&[HostRequirementKind::Agentic]));
    let mut options = RunOptions::default();
    options
        .host_context
        .authority
        .grants
        .push(HostActionGrant::allow("Agentic", "infer"));
    options.model_policy.provider_capabilities = Some(full_model_capabilities());
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            vec![],
            &host,
            options,
        )
        .await
        .unwrap();
    assert!(
        result
            .diagnostics
            .iter()
            .any(|d| d.message.contains("schema")),
        "{:?}",
        result.diagnostics
    );
    assert!(host.model_requests().is_empty());
}
