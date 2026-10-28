use super::*;
use crate::{
    control::CallTarget,
    value::{AgentRevisionValue, HostHandleValue, InterpValue},
};

#[tokio::test(flavor = "current_thread")]
async fn a_parent_handler_resumes_a_compiled_browser_action_without_a_browser_host() {
    let source = r#"
module app;
import std.browser.protocol.{BrowserSession, close};
import std.effects.Browser;
agent Worker(session: BrowserSession) -> i32 { close(session)?; return 1; }
flow main(worker: Worker, session: BrowserSession) -> i32 {
    return handle { worker(session) } with {
        Browser.close<_>(_) => { resume; }
    };
}
flow inspect() -> unit { let definition = ^Worker; return; }
"#;
    let frozen = frozen(source);
    let original = frozen.checked().clone();
    let prepared = InterpreterCandidatePublisher
        .publish(compile(frozen, "close(session)?; return 2;"))
        .unwrap();
    let agent = AgentRevisionValue::new_in_program(
        prepared.clone(),
        None,
        &original,
        prepared.targets.component,
    )
    .unwrap();
    let entry = original.entry.unwrap();
    let etas_types::ItemSignature::Flow(signature) = &original.types.item_signatures[&entry] else {
        panic!("checked entry")
    };
    let session = HostHandleValue::browser_session(signature.params[1], "session-1".into());
    let host = crate::testing::FakeHost::new(Default::default());
    let result = Interpreter
        .run_checked(
            &original,
            EntryPoint { item: entry },
            vec![
                InterpValue::Callable(CallTarget::AgentRevision(agent)),
                InterpValue::HostHandle(session),
            ],
            &host,
            Default::default(),
        )
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&InterpValue::i32(2)));
}

#[tokio::test(flavor = "current_thread")]
async fn a_compiled_agent_returns_a_generic_secret_handle_in_the_callers_type_universe() {
    let source = r#"
module app;
import std.secret.SecretValue;
type Key;
agent Worker(value: SecretValue<Key>) -> SecretValue<Key> { return value; }
flow main(worker: Worker, secret: SecretValue<Key>) -> SecretValue<Key> {
    return worker(secret);
}
flow inspect() -> unit { let definition = ^Worker; return; }
"#;
    let frozen = frozen(source);
    let original = frozen.checked().clone();
    let prepared = InterpreterCandidatePublisher
        .publish(compile(frozen, "let result = value; return result;"))
        .unwrap();
    let agent = AgentRevisionValue::new_in_program(
        prepared.clone(),
        None,
        &original,
        prepared.targets.component,
    )
    .unwrap();
    let entry = original.entry.unwrap();
    let etas_types::ItemSignature::Flow(signature) = &original.types.item_signatures[&entry] else {
        panic!("checked entry")
    };
    let secret = HostHandleValue::secret_value(
        signature.params[1],
        etas_host::SecretValue::new(etas_host::SecretRef::new("secret-1"), "<redacted>"),
    );
    let host = crate::testing::FakeHost::new(Default::default());
    let result = Interpreter
        .run_checked(
            &original,
            EntryPoint { item: entry },
            vec![
                InterpValue::Callable(CallTarget::AgentRevision(agent)),
                InterpValue::HostHandle(secret.clone()),
            ],
            &host,
            Default::default(),
        )
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&InterpValue::HostHandle(secret)));
}

#[tokio::test(flavor = "current_thread")]
async fn a_preserved_hmac_agent_uses_its_checked_secret_and_returns_a_relocated_digest() {
    // This exercises the native ABI; an edited body needs separate outcome-contract evidence.
    let body = "return hmac_sha256(secret, body);";
    let source = r#"
module app;
import std.secret.SecretValue;
import std.crypto.{Digest, CryptoError, hmac_sha256};
type Key;
agent Worker(secret: SecretValue<Key>, body: bytes) -> Digest ![Error<CryptoError>] {return hmac_sha256(secret, body);}
flow main(worker: Worker, secret: SecretValue<Key>, body: bytes) -> Digest ![Error<CryptoError>] {
    return worker(secret, body);
}
flow inspect() -> unit { let definition = ^Worker; return; }
"#;
    let frozen = frozen(source);
    let original = frozen.checked().clone();
    let prepared = InterpreterCandidatePublisher
        .publish(compile(frozen, body))
        .unwrap();
    let agent = AgentRevisionValue::new_in_program(
        prepared.clone(),
        None,
        &original,
        prepared.targets.component,
    )
    .unwrap();
    let entry = original.entry.unwrap();
    let etas_types::ItemSignature::Flow(signature) = &original.types.item_signatures[&entry] else {
        panic!("checked entry")
    };
    let secret = HostHandleValue::secret_value(
        signature.params[1],
        etas_host::SecretValue::new(etas_host::SecretRef::new("compiled-key"), "<redacted>"),
    );
    let host = crate::testing::FakeHost::new(crate::host::HostServiceAvailability {
        secret_access: true,
        ..Default::default()
    });
    host.seed_secret_payload(etas_host::SecretPayload::Bytes(vec![11; 32]));
    let mut options = crate::api::RunOptions::default();
    options
        .host_context
        .authority
        .grants
        .push(etas_host::HostActionGrant::allow("Secret", "use"));
    let result = Interpreter
        .run_checked(
            &original,
            EntryPoint { item: entry },
            vec![
                InterpValue::Callable(CallTarget::AgentRevision(agent)),
                InterpValue::HostHandle(secret),
                InterpValue::Bytes(vec![1, 2, 3].into()),
            ],
            &host,
            options,
        )
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.value(),
        Some(&InterpValue::Nominal {
            ty: signature.output,
            value: InterpValue::Bytes(vec![11; 32].into()).into(),
        })
    );
    let requests = host.secret_requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].operation,
        etas_host::SecretOperation::HmacSha256 {
            key: etas_host::SecretRef::new("compiled-key"),
            body: vec![1, 2, 3],
        }
    );
}
