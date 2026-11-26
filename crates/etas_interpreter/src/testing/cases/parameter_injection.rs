use super::super::*;

pub(super) fn checked(source: &str) -> etas_frontend::CheckedProgram {
    project::checked_project_with_environment(
        source,
        etas_frontend::ProjectEnvironmentInput {
            local_source_package: Some(etas_code::PackageKey::from_canonical_fingerprint(
                etas_code::CodeFingerprint::of_bytes("etas.test.source-tools", b"app"),
            )),
            ..Default::default()
        },
    )
}

async fn evaluate(source: &str, expected: i32) {
    let checked = checked(source);
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            vec![],
            &FakeHost::new(availability(&[])),
            RunOptions::default(),
        )
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&InterpValue::i32(expected)));
}

#[tokio::test(flavor = "current_thread")]
async fn injected_parameters_interleave_defaults_and_resolve_once_per_omitted_call() {
    evaluate(
        r#"module app.main;
effect Supply { action next() -> i32; }
flow total(x: i32, @inject(Supply.next) a: i32, b: i32 = a + 1,
    @inject(Supply.next) c: i32) -> i32 ![] { return x + a + b + c; }
flow main() -> i32 ![] {
    var n = 0;
    let value = handle {
        let a = total(1);
        let b = total(1, a = 10, c = 20);
        a + b
    } with { Supply.next() => { n = n + 1; resume n; } };
    return value + n;
}"#,
        50,
    )
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn injected_parameter_uses_caller_handler_before_trailing_body_handler() {
    evaluate(
        r#"module app.main;
effect Supply { action next() -> i32; }
flow value(@inject(Supply.next) x: i32) -> i32 ![Supply.next] {
    return x + perform Supply.next();
} with { Supply.next() => 100; }
flow main() -> i32 ![] {
    return value() with { Supply.next() => 7; };
}"#,
        107,
    )
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn generic_spec_methods_inherit_injection_at_the_named_call_interface() {
    evaluate(
        r#"module app.main;
effect Supply<T> { action value() -> T; }
type Box<T> = { n: T };
impl Box<T> { flow read(self, @inject(Supply<T>.value) value: T) -> T ![] { return value; } }
spec Value<A> { flow read(self, @inject(Supply<A>.value) fallback: A) -> A ![]; }
type ValueBox<B> = { n: B };
impl ValueBox<B> ~ Value<B> { flow read(self, fallback: B) -> B ![] { return fallback; } }
flow main() -> i32 ![] {
    let a = Box { n = 0 }.read() with { Supply<i32>.value() => 21; };
    let b = ValueBox { n = 0 }.read() with { Supply<i32>.value() => 21; };
    return a + b;
}"#,
        42,
    )
    .await;
}
