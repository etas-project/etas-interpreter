use super::super::*;

async fn run(source: &str, args: Vec<value::InterpValue>, host: &FakeHost) -> api::RunResult {
    let checked = checked_project(source);
    Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            args,
            host,
            RunOptions::default(),
        )
        .await
        .unwrap()
}

const TCP_IMPORT: &str =
    "import std.net.tcp.{Host, Port, TcpOptions, NetworkError, connect}; import std.effects.Net;";
const CONNECT: &str =
    "connect(Host { host = \"example.test\" }, Port { port = 443 }, TcpOptions {})";

#[test]
fn secret_and_browser_helpers_have_checked_action_mediation_plans() {
    for (imports, parameter, body, action) in [
        (
            "import std.secret.{SecretKey, read};",
            "key: SecretKey<string>",
            "let value = read(key);",
            "Secret.read",
        ),
        (
            "import std.browser.protocol.{BrowserProfile, attach};",
            "profile: BrowserProfile",
            "let session = attach(profile);",
            "Browser.attach",
        ),
        (
            "import std.browser.protocol.{BrowserProfile, create};",
            "profile: BrowserProfile",
            "let session = create(profile);",
            "Browser.attach",
        ),
        (
            "import std.browser.protocol.{BrowserSession, BrowserMessage, send};",
            "session: BrowserSession, message: BrowserMessage",
            "send(session, message);",
            "Browser.send",
        ),
        (
            "import std.browser.protocol.{BrowserSession, recv};",
            "session: BrowserSession",
            "let message = recv(session);",
            "Browser.recv",
        ),
        (
            "import std.browser.protocol.{BrowserSession, screenshot};",
            "session: BrowserSession",
            "let image = screenshot(session);",
            "Browser.screenshot",
        ),
        (
            "import std.browser.protocol.{BrowserSession, close};",
            "session: BrowserSession",
            "close(session);",
            "Browser.close",
        ),
    ] {
        let checked = checked_project(&format!(
            "module app.main; {imports} flow main({parameter}) -> unit {{ {body} return; }}"
        ));
        let fact = checked
            .effects
            .std_action_calls
            .values()
            .next()
            .unwrap_or_else(|| {
                panic!(
                    "{action} helper must retain a checked invocation, not bypass action mediation"
                )
            });
        assert_eq!(checked.effects.std_action_calls.len(), 1);
        let expected = checked.effect_registry.action_by_name(action).unwrap();
        let actual = match &fact.invocation {
            etas_effects::Effect::AppliedAction(instance) => &instance.action,
            etas_effects::Effect::Action(action) => action,
            effect => panic!("expected action, got {effect:?}"),
        };
        assert_eq!(actual, &expected);
        if action == "Secret.read" {
            let etas_effects::Effect::AppliedAction(instance) = &fact.invocation else {
                panic!("generic secret action")
            };
            let [etas_types::EffectArgRef::Type(key)] = instance.args.as_slice() else {
                panic!("Secret.read must preserve its checked type selector")
            };
            let Some(etas_types::Type::Applied { args, .. }) = checked.type_store.get(fact.output)
            else {
                panic!("Secret.read must return an instantiated SecretValue<K>")
            };
            assert_eq!(args.as_slice(), &[*key]);
        }
        let plan = Interpreter.plan(&checked, api::PlanOptions);
        assert!(plan.plan.is_some(), "{action}: {:?}", plan.diagnostics);
    }
}

#[test]
fn a_native_action_fact_must_match_its_resolved_intrinsic_origin() {
    let source = format!(
        "module app.main; {TCP_IMPORT} flow main() -> unit {{ let stream = {CONNECT}; return; }}"
    );
    let mut checked = checked_project(&source);
    let project = std::sync::Arc::make_mut(&mut checked);
    let effects = std::sync::Arc::make_mut(&mut project.effects);
    effects
        .std_action_calls
        .values_mut()
        .next()
        .unwrap()
        .intrinsic = etas_std::intrinsic::runtime::TLS_CONNECT;
    let result = Interpreter.plan(&checked, api::PlanOptions);
    assert!(result.plan.is_none());
    assert!(
        result.diagnostics.iter().any(|diagnostic| diagnostic
            .message
            .contains("differs from its resolved intrinsic origin")),
        "{:?}",
        result.diagnostics
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_native_action_rejects_a_selector_broadened_after_type_checking() {
    let mut checked = checked_project(
        r#"
module app.main;
import std.fs.{Region, WorkspacePath, read_bytes};
import std.effects.Fs;
type Root;
impl Root ~ Region;
flow main(path: WorkspacePath<Root>) -> i32 ![] {
    return handle { let content = read_bytes(path); 0 } with { Fs.read<Root>(_) => { finish 7; } };
}
"#,
    );
    let project = std::sync::Arc::make_mut(&mut checked);
    let effects = std::sync::Arc::make_mut(&mut project.effects);
    let fact = effects.std_action_calls.values_mut().next().unwrap();
    let etas_effects::Effect::AppliedAction(instance) = &mut fact.invocation else {
        panic!("typed selector")
    };
    instance.args[0] = etas_types::EffectArgRef::Wildcard;
    let host = FakeHost::new(HostServiceAvailability::default());
    let path = etas_host::WorkspacePathRef::new(
        etas_host::WorkspaceRegionId::new("app.main.Root").unwrap(),
        "cached",
    )
    .unwrap();
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            vec![value::InterpValue::WorkspacePath(path)],
            &host,
            RunOptions::default(),
        )
        .await
        .unwrap();
    assert!(result.value().is_none());
    assert!(
        result.diagnostics.iter().any(|diagnostic| diagnostic
            .message
            .contains("differs from its generic instantiation")),
        "{:?}",
        result.diagnostics
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_user_defined_byte_stream_reaches_its_handler_before_host_handle_decoding() {
    let host = FakeHost::new(HostServiceAvailability::default());
    let result = run(
        r#"
module app.main;
import std.stream.{ByteStream, read, Timeout};
import std.effects.Stream;
import std.option.None;
type Buffer = string;
impl Buffer ~ ByteStream;
flow main() -> i32 ![] {
    return handle { let unused = read(Buffer("local"), 16, None<Timeout>()); 0 } with {
        Stream.read<Buffer>(_) => { finish 41; }
    };
}

"#,
        vec![],
        &host,
    )
    .await;
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&value::InterpValue::i32(41)));
    assert!(host.stream_requests().is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn a_generic_wrapper_instantiates_the_stream_selector_at_its_active_call_frame() {
    let host = FakeHost::new(HostServiceAvailability::default());
    let result = run(r#"
module app.main;
import std.stream.{ByteStream, read, Timeout};
import std.effects.Stream;
import std.option.None;
type Buffer = string;
impl Buffer ~ ByteStream;
flow pull<SourceStream ~ ByteStream>(stream: SourceStream) -> i32 { let unused = read(stream, 16, None<Timeout>()); return 0; }
flow main() -> i32 ![] {
    return handle { pull(Buffer("local")) } with { Stream.read<Buffer>(_) => { finish 43; } };
}
"#, vec![], &host).await;
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&value::InterpValue::i32(43)));
    assert!(host.stream_requests().is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn a_tcp_handler_receives_declared_payload_without_a_host_connection() {
    let host = FakeHost::new(HostServiceAvailability::default());
    let source = format!(
        r#"
module app.main;
{TCP_IMPORT}
flow main() -> bool ![] {{
    return handle {{ let stream = {CONNECT}; false }} with {{
        Net.tcp_connect<_, _>(host, port) => {{ finish host.host == "example.test" && port.port == 443; }}
    }};
}}
"#
    );
    let result = run(&source, vec![], &host).await;
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&value::InterpValue::Bool(true)));
    assert!(host.tcp_requests().is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn a_standard_callable_value_preserves_its_native_action_origin() {
    let host = FakeHost::new(HostServiceAvailability::default());
    let source = format!(
        r#"
module app.main;
{TCP_IMPORT}
flow main() -> bool ![] {{
    let connect_later = connect;
    return handle {{
        let stream = connect_later(Host {{ host = "example.test" }}, Port {{ port = 443 }}, TcpOptions {{}});
        false
    }} with {{ Net.tcp_connect<_, _>(host, port) => {{ finish host.host == "example.test" && port.port == 443; }} }};
}}
"#
    );
    let result = run(&source, vec![], &host).await;
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&value::InterpValue::Bool(true)));
    assert!(host.tcp_requests().is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn a_filesystem_handler_matches_the_checked_region_without_file_io() {
    let host = FakeHost::new(HostServiceAvailability::default());
    let source = r#"
module app.main;
import std.fs.{Region, WorkspacePath, read_bytes};
import std.effects.Fs;
type ProjectRoot;
type OtherRoot;
impl ProjectRoot ~ Region;
impl OtherRoot ~ Region;
flow main(path: WorkspacePath<ProjectRoot>, content: bytes) -> bytes ![] {
    return handle { read_bytes(path) } with {
        Fs.read<OtherRoot>(_) => { finish abort("wrong region selected"); },
        Fs.read<ProjectRoot>(_) => { resume content; }
    };
}
"#;
    let path = etas_host::WorkspacePathRef::new(
        etas_host::WorkspaceRegionId::new("app.main.ProjectRoot").unwrap(),
        "cached",
    )
    .unwrap();
    let expected = value::InterpValue::Bytes(b"from handler".to_vec().into());
    let result = run(
        source,
        vec![value::InterpValue::WorkspacePath(path), expected.clone()],
        &host,
    )
    .await;
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&expected));
}

#[tokio::test(flavor = "current_thread")]
async fn stream_handlers_intercept_each_basic_operation_before_host_dispatch() {
    for (call, action) in [
        ("read(stream, 16, None<Timeout>())", "read"),
        ("write_all(stream, input)", "write"),
        ("flush(stream)", "flush"),
        ("close(stream)", "close"),
    ] {
        let host = FakeHost::new(availability(&[HostRequirementKind::Tcp]));
        host.seed_tcp_connect_stream("native-1", "example.test", 443);
        let source = format!(
            r#"
module app.main;
{TCP_IMPORT}
import std.stream.{{read, write_all, flush, close, Timeout}};
import std.effects.Stream;
import std.option.None;
flow main(input: bytes) -> i32 ![Error<NetworkError>] {{
    let stream = {CONNECT};
    return handle {{ let unused = {call}; 0 }} with {{
        Stream.{action}<std.net.tcp.TcpStream>(_) => {{ finish 17; }}
    }};
}}
"#
        );
        let result = run(
            &source,
            vec![value::InterpValue::Bytes(b"payload".to_vec().into())],
            &host,
        )
        .await;
        assert!(
            result.diagnostics.is_empty(),
            "{action}: {:?}",
            result.diagnostics
        );
        assert_eq!(
            result.value(),
            Some(&value::InterpValue::i32(17)),
            "{action}"
        );
        assert_eq!(host.tcp_requests().len(), 1);
        assert!(host.stream_requests().is_empty(), "{action}");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_tls_handler_uses_the_registry_payload_projection_without_a_tls_host() {
    let host = FakeHost::new(availability(&[HostRequirementKind::Tcp]));
    host.seed_tcp_connect_stream("native-tls", "example.test", 443);
    let source = format!(
        r#"
module app.main;
{TCP_IMPORT}
import std.tls.{{connect as handshake, Host as ServerName, TlsConfig}};
import std.effects.Tls;
flow main() -> bool ![Error<NetworkError>] {{
    let stream = {CONNECT};
    return handle {{ let tls = handshake(stream, ServerName {{ host = "example.test" }}, TlsConfig {{}}); false }} with {{
        Tls.handshake<_>(host) => {{ finish host.host == "example.test"; }}
    }};
}}
"#
    );
    let result = run(&source, vec![], &host).await;
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&value::InterpValue::Bool(true)));
    assert_eq!(host.tcp_requests().len(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn a_native_action_cannot_reconstruct_a_missing_checked_call_fact() {
    let source = format!(
        r#"
module app.main;
{TCP_IMPORT}
flow main() -> i32 {{ return handle {{ let stream = {CONNECT}; 0 }} with {{ Net.tcp_connect<_, _>(_, _) => {{ finish 7; }} }}; }}
"#
    );
    let mut checked = checked_project(&source);
    std::sync::Arc::make_mut(&mut std::sync::Arc::make_mut(&mut checked).effects)
        .std_action_calls
        .clear();
    let host = FakeHost::new(HostServiceAvailability::default());
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
    assert!(result.value().is_none());
    assert!(
        result.diagnostics.iter().any(|diagnostic| diagnostic
            .message
            .contains("requires a checked invocation fact")),
        "{:?}",
        result.diagnostics
    );
    assert!(host.tcp_requests().is_empty());
}
