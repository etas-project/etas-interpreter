use super::super::*;

const HMAC_SOURCE: &str = r#"
module app.main;
import std.secret.SecretValue;
import std.crypto.{Digest, CryptoError, hmac_sha256};
type ApiKey;
type OtherKey;
flow witness(key: SecretValue<OtherKey>) -> unit { return; }
flow main(key: SecretValue<ApiKey>, body: bytes) -> Digest ![Error<CryptoError>] {
    return hmac_sha256(key, body);
}
"#;

const GENERIC_HMAC_SOURCE: &str = r#"
module app.main;
import std.secret.SecretValue;
import std.crypto.{Digest, CryptoError, hmac_sha256};
type ApiKey;
flow sign<KeyKind>(key: SecretValue<KeyKind>, body: bytes) -> Digest ![Error<CryptoError>] {
    return hmac_sha256(key, body);
}
flow main(key: SecretValue<ApiKey>, body: bytes) -> Digest ![Error<CryptoError>] {
    return sign(key, body);
}
"#;

#[test]
fn hmac_uses_the_canonical_generic_secret_type_and_checked_key_selector() {
    let checked = checked_project(HMAC_SOURCE);
    let action = checked
        .effect_registry
        .action_by_name("Secret.use")
        .unwrap();
    let summary = &checked.effects.item_effects[&checked.entry.unwrap()];
    let requested = summary.requested_actions.effects.iter().collect::<Vec<_>>();
    let [etas_effects::Effect::AppliedAction(instance)] = requested.as_slice() else {
        panic!("HMAC must request exactly its typed Secret.use<K> action")
    };
    assert_eq!(instance.action, action);
    let [etas_types::EffectArgRef::Type(key)] = instance.args.as_slice() else {
        panic!("HMAC must not broaden its key selector to a wildcard")
    };
    assert!(
        matches!(checked.type_store.get(*key), Some(Type::Nominal(key)) if key.name == "app.main.ApiKey")
    );
}

#[test]
fn hmac_rejects_an_explicit_type_argument_from_another_key_domain() {
    let source = HMAC_SOURCE.replace("hmac_sha256(key, body)", "hmac_sha256<OtherKey>(key, body)");
    let output = etas_frontend::Frontend.check_project(etas_frontend::ProjectInput::single_source(
        etas_frontend::SourceInput::anonymous(source),
    ));
    assert!(
        output.diagnostics.iter().any(|diagnostic| diagnostic.code
            == DiagnosticCode::Type(etas_core::TypeDiagnosticCode::TypeMismatch)),
        "{:?}",
        output.diagnostics
    );
    assert!(output.checked.is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn hmac_preserves_the_opaque_secret_and_nominal_digest_through_the_host_boundary() {
    for source in [HMAC_SOURCE, GENERIC_HMAC_SOURCE] {
        let checked = checked_project(source);
        let entry = checked.entry.unwrap();
        let etas_types::ItemSignature::Flow(signature) = &checked.types.item_signatures[&entry]
        else {
            panic!("checked entry")
        };
        let secret = crate::value::HostHandleValue::secret_value(
            signature.params[0],
            etas_host::SecretValue::new(etas_host::SecretRef::new("sealed-key"), "<redacted>"),
        );
        let host = FakeHost::new(HostServiceAvailability {
            secret_access: true,
            ..Default::default()
        });
        let digest = vec![7; 32];
        host.seed_secret_payload(etas_host::SecretPayload::Bytes(digest.clone()));
        let mut options = RunOptions::default();
        options
            .host_context
            .authority
            .grants
            .push(HostActionGrant::allow("Secret", "use"));
        let body = b"message to sign".to_vec();
        let result = Interpreter
            .run_checked(
                &checked,
                EntryPoint { item: entry },
                vec![
                    value::InterpValue::HostHandle(secret),
                    value::InterpValue::Bytes(body.clone().into()),
                ],
                &host,
                options,
            )
            .await
            .unwrap();
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        assert_eq!(
            result.value(),
            Some(&value::InterpValue::Nominal {
                ty: signature.output,
                value: value::InterpValue::Bytes(digest.into()).into(),
            })
        );
        let requests = host.secret_requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(
            requests[0].operation,
            etas_host::SecretOperation::HmacSha256 {
                key: etas_host::SecretRef::new("sealed-key"),
                body,
            }
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn hmac_rejects_a_sealed_handle_with_the_wrong_checked_key_type_before_dispatch() {
    let checked = checked_project(HMAC_SOURCE);
    let wrong = checked
        .types
        .item_signatures
        .iter()
        .find_map(|(item, signature)| {
            let etas_hir::HirItem::Flow(flow) = checked.hir.items.get(*item)? else {
                return None;
            };
            if checked.symbols.get(flow.symbol)?.name != "witness" {
                return None;
            }
            let etas_types::ItemSignature::Flow(signature) = signature else {
                return None;
            };
            Some(signature.params[0])
        })
        .unwrap();
    let secret = crate::value::HostHandleValue::secret_value(
        wrong,
        etas_host::SecretValue::new(etas_host::SecretRef::new("other-key"), "<redacted>"),
    );
    let host = FakeHost::new(HostServiceAvailability {
        secret_access: true,
        ..Default::default()
    });
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            vec![
                value::InterpValue::HostHandle(secret),
                value::InterpValue::Bytes(vec![].into()),
            ],
            &host,
            RunOptions::default(),
        )
        .await
        .unwrap();
    assert!(
        result.diagnostics.iter().any(|diagnostic| diagnostic.code
            == DiagnosticCode::Analysis(AnalysisDiagnosticCode::InvalidArguments)),
        "{:?}",
        result.diagnostics
    );
    assert!(host.secret_requests().is_empty());
    assert!(result.events.is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn a_secret_grant_approved_for_this_boundary_reaches_the_provider() {
    let checked = checked_project(HMAC_SOURCE);
    let entry = checked.entry.unwrap();
    let etas_types::ItemSignature::Flow(signature) = &checked.types.item_signatures[&entry] else {
        panic!("checked entry")
    };
    let secret = crate::value::HostHandleValue::secret_value(
        signature.params[0],
        etas_host::SecretValue::new(etas_host::SecretRef::new("approved-key"), "<redacted>"),
    );
    let host = FakeHost::new(availability(&[
        HostRequirementKind::SecretAccess,
        HostRequirementKind::Approval,
    ]));
    let grant = HostActionGrant::allow("Secret", "use");
    host.seed_policy_decision(PolicyDecision::RequireApproval {
        request: ApprovalRequest {
            id: HostRequestId(905),
            reason: "approve this operation".into(),
            requested_grants: vec![grant.clone()],
            trace: TraceContext::root(TraceId(90)),
        },
    });
    host.seed_approval_decision(ApprovalDecision::Approved {
        grant: etas_host::ApprovalGrant {
            id: HostRequestId(905),
            grants: vec![grant.clone()],
        },
    });
    host.seed_secret_payload(etas_host::SecretPayload::Bytes(vec![9; 32]));
    let mut options = RunOptions::default();
    options.host_context.authority.policy =
        boundary_policy_context(HostValue::String("secret-policy".into()));
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint { item: entry },
            vec![
                value::InterpValue::HostHandle(secret),
                value::InterpValue::Bytes(vec![1].into()),
            ],
            &host,
            options,
        )
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.value(),
        Some(&value::InterpValue::Nominal {
            ty: signature.output,
            value: value::InterpValue::Bytes(vec![9; 32].into()).into(),
        })
    );
    assert_eq!(host.approval_call_count(), 1);
    assert_eq!(host.secret_requests()[0].authority.grants, vec![grant]);
}
