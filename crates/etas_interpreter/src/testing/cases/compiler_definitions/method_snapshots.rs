use super::*;

#[tokio::test(flavor = "current_thread")]
async fn direct_and_enumerated_methods_preserve_their_own_read_only_snapshots() {
    let (checked, definitions) = prepared(
        r#"
module app.main;
agent Worker(value: bool) -> bool ![] { return value; }
impl agent Worker {
    public flow helper(value: bool) -> bool ![] { return !value; }
}
flow main() -> bool {
    let current = Worker;
    let agent = ^Worker;
    let method = match agent.methods.get(0) { Some(method) => method, None => { return false; }, };
    let direct = ^Worker.helper;
    return (^Worker.run).revision == agent.revision
        && (^current.run).revision == agent.revision
        && (^current.helper).revision == direct.revision
        && (^Worker.run).dependencies == agent.dependencies
        && method.id == direct.id && method.revision == direct.revision
        && method.dependencies == direct.dependencies
        && method.dependencies.lexical_owner == agent.dependencies.lexical_owner
        && method.revision.key != agent.revision.key;
}
"#,
        &[],
    );
    let result = execute(
        &checked,
        RunOptions {
            code_definitions: Some(definitions),
            ..Default::default()
        },
    )
    .await;
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&InterpValue::Bool(true)));
}

#[tokio::test(flavor = "current_thread")]
async fn method_snapshots_keep_spec_selected_implementations_distinct() {
    let (checked, definitions) = prepared(
        r#"
module app.main;
spec Left<I, O> I => O ![];
spec Right<I, O> I => O ![];
agent Worker;
impl agent Worker ~ Left<i32, i32> { public flow run(value: i32) -> i32 ![] { return value; } }
impl agent Worker ~ Right<string, string> { public flow run(value: string) -> string ![] { return value; } }
flow main() -> bool {
    let current = Worker;
    let left = ^Worker::Left<i32, i32>.run;
    let right = ^Worker::Right<string, string>.run;
    return left.id != right.id && left.revision.key != right.revision.key
        && left.revision == (^current::Left<i32, i32>.run).revision
        && left.dependencies.lexical_owner == right.dependencies.lexical_owner;
}
"#,
        &[],
    );
    let result = execute(
        &checked,
        RunOptions {
            code_definitions: Some(definitions),
            ..Default::default()
        },
    )
    .await;
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&InterpValue::Bool(true)));
}
