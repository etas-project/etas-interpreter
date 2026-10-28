use super::{AdapterError, PureAbiProjector, execute_pure_intrinsic};
use crate::{intrinsic::dispatch::CheckedPureIntrinsicCall, value::InterpValue};
use etas_std::{StdIntrinsicId, intrinsic::pure};
use etas_types::{PrimitiveType, TrustWrapper, Type, TypeInterner};

#[test]
fn adt_fast_paths_reject_wrong_nominal_and_trust_payload_identity() {
    let checked = crate::testing::project::checked_project(
        "module app.main; type A = string; type B = string; flow main(a: A, b: B) -> unit { return; }",
    );
    let etas_types::ItemSignature::Flow(signature) =
        &checked.types.item_signatures[&checked.entry.unwrap()]
    else {
        panic!("flow signature")
    };
    let (a, b) = (signature.params[0], signature.params[1]);
    let mut types = TypeInterner::from_store(checked.type_store.as_ref().clone());
    let boolean = types.primitive(PrimitiveType::Bool);
    let trusted = types.intern(Type::Trust {
        wrapper: TrustWrapper::Trusted,
        inner: a,
    });
    let option = types.intern(Type::Option(a));
    let result = types.intern(Type::Result { ok: a, err: a });
    let trusted_option = types.intern(Type::Option(trusted));
    let projector = PureAbiProjector::build(&types.into_store()).unwrap();
    let bad = || InterpValue::Nominal {
        ty: b,
        value: InterpValue::String("payload".into()).into(),
    };
    let variant = |name: &str| InterpValue::Variant {
        name: name.into(),
        fields: vec![bad()].into(),
    };
    let cases = [
        ("Some", pure::OPTION_SOME, a, option, bad()),
        ("Ok", pure::RESULT_OK, a, result, bad()),
        ("Err", pure::RESULT_ERR, a, result, bad()),
        (
            "Option.is_some",
            pure::OPTION_IS_SOME,
            option,
            boolean,
            InterpValue::OptionSome(bad().into()),
        ),
        (
            "Option.is_none",
            pure::OPTION_IS_NONE,
            option,
            boolean,
            InterpValue::OptionSome(bad().into()),
        ),
        (
            "Option.unwrap",
            pure::OPTION_UNWRAP,
            option,
            a,
            InterpValue::OptionSome(bad().into()),
        ),
        (
            "Result.is_ok",
            pure::RESULT_IS_OK,
            result,
            boolean,
            variant("Ok"),
        ),
        (
            "Result.is_err",
            pure::RESULT_IS_ERR,
            result,
            boolean,
            variant("Err"),
        ),
        (
            "Result.unwrap",
            pure::RESULT_UNWRAP,
            result,
            a,
            variant("Ok"),
        ),
    ];
    let mut accepted = Vec::new();
    for (label, id, parameter, output, value) in cases {
        let call = CheckedPureIntrinsicCall {
            intrinsic: StdIntrinsicId(id),
            parameter_types: vec![parameter],
            result_type: output,
        };
        match execute_pure_intrinsic(&call, vec![value], &projector) {
            Err(AdapterError::NominalIdentity { expected, actual }) => {
                assert_eq!((expected, actual), (a, b))
            }
            other => accepted.push((label, other.is_ok())),
        }
    }
    let call = CheckedPureIntrinsicCall {
        intrinsic: StdIntrinsicId(pure::OPTION_SOME),
        parameter_types: vec![trusted],
        result_type: trusted_option,
    };
    let bad_trust = InterpValue::Trust {
        wrapper: TrustWrapper::Untrusted,
        value: InterpValue::Nominal {
            ty: a,
            value: InterpValue::String("payload".into()).into(),
        }
        .into(),
    };
    if !matches!(execute_pure_intrinsic(&call, vec![bad_trust], &projector), Err(AdapterError::TypeMismatch { expected, .. }) if expected == trusted)
    {
        accepted.push(("trust Some", true));
    }
    assert!(
        accepted.is_empty(),
        "wrong identity crossed fast paths: {accepted:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn checked_entry_cannot_return_a_different_nominal_through_some() {
    let checked = crate::testing::project::checked_project(
        r#"
module app.main;
type A = string;
type B = string;
flow main(a: A, b: B) -> Option<A> { return Some(a); }
"#,
    );
    let entry = checked.entry.unwrap();
    let etas_types::ItemSignature::Flow(signature) = &checked.types.item_signatures[&entry] else {
        panic!("flow signature")
    };
    let b = signature.params[1];
    let value = || InterpValue::Nominal {
        ty: b,
        value: InterpValue::String("payload".into()).into(),
    };
    let host = crate::testing::FakeHost::new(crate::host::HostServiceAvailability::default());
    let result = crate::Interpreter
        .run_checked(
            &checked,
            crate::api::EntryPoint { item: entry },
            vec![value(), value()],
            &host,
            crate::api::RunOptions::default(),
        )
        .await
        .unwrap();
    assert!(
        result.value().is_none(),
        "a B value escaped the checked Option<A> result: {:?}",
        result.value()
    );
    assert!(!result.diagnostics.is_empty());
}

#[test]
fn native_prompt_payloads_keep_their_backing_through_adt_fast_paths() {
    let mut types = TypeInterner::new();
    let prompt = types.intern(Type::Prompt);
    let option = types.intern(Type::Option(prompt));
    let boolean = types.primitive(PrimitiveType::Bool);
    let projector = PureAbiProjector::build(&types.into_store()).unwrap();
    let original = crate::value::PromptValue::new(vec![crate::value::PromptMessage {
        role: crate::value::PromptRole::User,
        text: "large native payload".repeat(1024).into(),
        trust: None,
    }]);
    let pointer = original.as_ptr();
    let call = |id, input, output, value| {
        execute_pure_intrinsic(
            &CheckedPureIntrinsicCall {
                intrinsic: StdIntrinsicId(id),
                parameter_types: vec![input],
                result_type: output,
            },
            vec![value],
            &projector,
        )
        .unwrap()
    };
    let wrapped = call(
        pure::OPTION_SOME,
        prompt,
        option,
        InterpValue::Prompt(original),
    );
    assert_eq!(
        call(pure::OPTION_IS_SOME, option, boolean, wrapped.clone()),
        InterpValue::Bool(true)
    );
    let InterpValue::Prompt(value) = call(pure::OPTION_UNWRAP, option, prompt, wrapped) else {
        panic!("native prompt payload")
    };
    assert_eq!(value.as_ptr(), pointer);
}

#[test]
fn identity_checks_walk_deep_trust_wrappers_without_payload_projection() {
    let mut types = TypeInterner::new();
    let text = types.primitive(PrimitiveType::String);
    let mut payload = InterpValue::String("payload".repeat(1024).into());
    let mut expected = text;
    for _ in 0..1024 {
        expected = types.intern(Type::Trust {
            wrapper: TrustWrapper::Trusted,
            inner: expected,
        });
        payload = InterpValue::Trust {
            wrapper: TrustWrapper::Trusted,
            value: payload.into(),
        };
    }
    let option = types.intern(Type::Option(expected));
    let boolean = types.primitive(PrimitiveType::Bool);
    let projector = PureAbiProjector::build(&types.into_store()).unwrap();
    let call = CheckedPureIntrinsicCall {
        intrinsic: StdIntrinsicId(pure::OPTION_IS_SOME),
        parameter_types: vec![option],
        result_type: boolean,
    };
    let args = vec![InterpValue::OptionSome(payload.into())];
    let (result, cost) = crate::testing::allocation::measure(|| {
        execute_pure_intrinsic(&call, args, &projector).unwrap()
    });
    assert_eq!(result, InterpValue::Bool(true));
    assert_eq!(
        cost.count, 0,
        "identity checking copied or projected the payload: {cost:?}"
    );
}
