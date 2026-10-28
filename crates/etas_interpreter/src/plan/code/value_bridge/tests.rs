use std::{collections::BTreeMap, sync::Arc};

use etas_code::{
    CodeFingerprint, PackageKey, descriptor::GenericOwner, syntax::CodeSyntaxOperations,
};
use etas_frontend::{
    EntryPolicy, Frontend, FrontendSession, ProjectInput, SourceInput,
    code::{
        definition::DefinitionCatalog,
        draft::DefaultBodyOverlay,
        identity::DefinitionOrigins,
        service::{CandidatePublisher, NativeCodeSyntaxOperations},
    },
};
use etas_types::{ItemSignature, TypeId};

use super::*;
use crate::api::code::{InterpreterCandidatePublisher, PreparedAgentRevision};

mod action;
mod code;

#[test]
fn generic_secret_handles_preserve_their_checked_key_identity_across_revisions() {
    let (prepared, scope, ty) = prepared(
        "import std.secret.SecretValue; type Key;",
        "SecretValue<Key>",
    );
    let value = InterpValue::HostHandle(crate::value::HostHandleValue::secret_value(
        ty,
        etas_host::SecretValue::new(etas_host::SecretRef::new("sealed-key"), "<redacted>"),
    ));
    assert_eq!(roundtrip(&prepared, &scope, ty, value.clone()), value);
    let bridge = prepared.type_bridge();
    let InterpValue::HostHandle(handle) = value else {
        unreachable!()
    };
    let invalid = handle.into_revision_type(etas_types::TypeId(u32::MAX));
    assert!(matches!(
        bridge.value_to_candidate(
            bridge.original_program(),
            &scope,
            ty,
            InterpValue::HostHandle(invalid)
        ),
        Err(RevisionValueBridgeError::Value {
            failure: RevisionValueFailure::WrongNominalIdentity { .. },
            ..
        })
    ));
}

#[test]
fn workspace_paths_preserve_their_checked_region_and_reject_region_substitution() {
    let (prepared, scope, ty) = prepared(
        "import std.fs.{WorkspacePath, Region}; type Root; impl Root ~ Region;",
        "WorkspacePath<Root>",
    );
    let path = etas_host::WorkspacePathRef::new(
        etas_host::WorkspaceRegionId::new("app.Root").unwrap(),
        "input/data",
    )
    .unwrap();
    let value = InterpValue::WorkspacePath(path);
    assert_eq!(roundtrip(&prepared, &scope, ty, value.clone()), value);
    let invalid = etas_host::WorkspacePathRef::new(
        etas_host::WorkspaceRegionId::new("app.OtherRoot").unwrap(),
        "input/data",
    )
    .unwrap();
    let bridge = prepared.type_bridge();
    assert!(
        matches!(bridge.value_to_candidate(bridge.original_program(), &scope, ty, InterpValue::WorkspacePath(invalid)), Err(RevisionValueBridgeError::Value { failure: RevisionValueFailure::InvalidField(field), .. }) if field == "workspace_path.region")
    );
}

fn prepared(declarations: &str, input: &str) -> (Arc<PreparedAgentRevision>, GenericOwner, TypeId) {
    prepared_body(declarations, input, input, "return value;")
}

fn prepared_body(
    declarations: &str,
    input: &str,
    output: &str,
    body: &str,
) -> (Arc<PreparedAgentRevision>, GenericOwner, TypeId) {
    let source = format!(
        "module app; {declarations} agent Worker(value: {input}) -> {output} {{ {body} }} flow inspect() -> unit {{ let definition = ^Worker; return; }}"
    );
    let mut input = ProjectInput::single_source(SourceInput::anonymous(source));
    input.options.entry_policy = EntryPolicy::Optional;
    let output = Frontend.check_project(input);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.unwrap();
    let package = PackageKey::from_canonical_fingerprint(CodeFingerprint::of_bytes(
        "etas.test.value-bridge",
        b"app",
    ));
    let origins = DefinitionOrigins {
        source_modules: DefinitionOrigins::declared_source_modules(&checked, package).unwrap(),
        standard_package: package,
        external_definitions: BTreeMap::new(),
    };
    let reflection = *checked.types.reflections.keys().next().unwrap();
    let frozen = DefinitionCatalog::build(checked, &origins, Default::default())
        .unwrap()
        .freeze_agent(reflection)
        .unwrap();
    let callable = frozen.contract().header().default_entry.as_ref().unwrap();
    let scope = GenericOwner::Method(callable.as_method().unwrap().clone());
    let param = match &frozen.checked().types.item_signatures[&frozen.default_body().unwrap()] {
        ItemSignature::Agent(signature) => signature.callable().unwrap().params[0],
        ItemSignature::Flow(signature) => signature.params[0],
        _ => panic!("checked agent entry"),
    };
    let syntax = NativeCodeSyntaxOperations::default();
    let overlay =
        DefaultBodyOverlay::prepare(frozen, syntax.parse_body(body).unwrap(), &syntax).unwrap();
    let candidate = FrontendSession::new()
        .compile_body_overlay(overlay)
        .unwrap();
    (
        InterpreterCandidatePublisher.publish(candidate).unwrap(),
        scope,
        param,
    )
}

fn roundtrip(
    prepared: &PreparedAgentRevision,
    scope: &GenericOwner,
    ty: TypeId,
    value: InterpValue,
) -> InterpValue {
    let bridge = prepared.type_bridge();
    let target = bridge
        .to_candidate(bridge.original_program(), scope, ty)
        .unwrap();
    let value = bridge
        .value_to_candidate(bridge.original_program(), scope, ty, value)
        .unwrap();
    bridge
        .value_to_original(prepared.program(), scope, target, value)
        .unwrap()
}

#[test]
fn nested_record_option_result_tuple_and_collection_values_roundtrip() {
    let (prepared, scope, ty) = prepared(
        "",
        "{ a: Array<Option<i32>>, b: List<Result<string, i32>>, c: (bool, bytes), d: Map<string, i32>, e: Set<i32> }",
    );
    let value = InterpValue::Record(
        vec![
            (
                "e".into(),
                InterpValue::Set(vec![InterpValue::i32(7)].into()),
            ),
            (
                "c".into(),
                InterpValue::Tuple(
                    vec![
                        InterpValue::Bool(true),
                        InterpValue::Bytes(vec![1, 2].into()),
                    ]
                    .into(),
                ),
            ),
            (
                "a".into(),
                InterpValue::Array(
                    vec![
                        InterpValue::OptionSome(InterpValue::i32(42).into()),
                        InterpValue::OptionNone,
                    ]
                    .into(),
                ),
            ),
            (
                "d".into(),
                InterpValue::Map(
                    vec![(InterpValue::String("k".into()), InterpValue::i32(3))].into(),
                ),
            ),
            (
                "b".into(),
                InterpValue::List(
                    vec![
                        InterpValue::Variant {
                            name: "Ok".into(),
                            fields: vec![InterpValue::String("ok".into())].into(),
                        },
                        InterpValue::Variant {
                            name: "Err".into(),
                            fields: vec![InterpValue::i32(2)].into(),
                        },
                    ]
                    .into(),
                ),
            ),
        ]
        .into(),
    );
    assert_eq!(roundtrip(&prepared, &scope, ty, value.clone()), value);
}

#[test]
fn applied_nominal_record_keeps_outer_and_inner_type_identity() {
    let (prepared, scope, ty) = prepared(
        "type Id = string; type Envelope<T> = { value: T };",
        "Envelope<Id>",
    );
    let bridge = prepared.type_bridge();
    let etas_types::Type::Applied { args, .. } =
        bridge.original_program().type_store.get(ty).unwrap()
    else {
        panic!("applied type")
    };
    let value = InterpValue::Nominal {
        ty,
        value: InterpValue::Record(
            vec![(
                "value".into(),
                InterpValue::Nominal {
                    ty: args[0],
                    value: InterpValue::String("id".into()).into(),
                },
            )]
            .into(),
        )
        .into(),
    };
    assert_eq!(roundtrip(&prepared, &scope, ty, value.clone()), value);
}

#[test]
fn recursive_generic_enum_values_use_a_worklist_not_the_rust_stack() {
    let (prepared, scope, ty) = prepared("enum Chain<T> { End, Link(T, Chain<T>), }", "Chain<i32>");
    let mut value = InterpValue::Variant {
        name: "End".into(),
        fields: Vec::new().into(),
    };
    for n in 0..12_000 {
        value = InterpValue::Variant {
            name: "Link".into(),
            fields: vec![InterpValue::i32(n), value].into(),
        };
    }
    let translated = roundtrip(&prepared, &scope, ty, value.clone());
    assert_eq!(translated, value);
}

#[test]
fn named_enum_fields_are_checked_against_the_variant_payload() {
    let (prepared, scope, ty) = prepared(
        "enum Response { Success(string), Failure { code: i32, message: string }, }",
        "Response",
    );
    let value = InterpValue::Variant {
        name: "Failure".into(),
        fields: vec![
            InterpValue::i32(503),
            InterpValue::String("unavailable".into()),
        ]
        .into(),
    };
    assert_eq!(roundtrip(&prepared, &scope, ty, value.clone()), value);
    let invalid = InterpValue::Variant {
        name: "Failure".into(),
        fields: vec![InterpValue::String("503".into()), InterpValue::i32(1)].into(),
    };
    assert!(matches!(
        prepared.type_bridge().value_to_candidate(
            prepared.type_bridge().original_program(),
            &scope,
            ty,
            invalid
        ),
        Err(RevisionValueBridgeError::Value {
            failure: RevisionValueFailure::InvalidPrimitive,
            ..
        })
    ));
    let unknown = InterpValue::Variant {
        name: "Unknown".into(),
        fields: Vec::new().into(),
    };
    assert!(matches!(
        prepared.type_bridge().value_to_candidate(
            prepared.type_bridge().original_program(),
            &scope,
            ty,
            unknown
        ),
        Err(RevisionValueBridgeError::Value {
            failure: RevisionValueFailure::UnknownVariant(_),
            ..
        })
    ));
}

#[test]
fn record_layout_and_numeric_abi_cannot_be_guessed_from_value_shape() {
    let (prepared, scope, ty) = prepared("", "{ count: i32, name: string }");
    let bridge = prepared.type_bridge();
    for fields in [
        vec![("count".into(), InterpValue::i32(1))],
        vec![
            ("count".into(), InterpValue::i32(1)),
            ("count".into(), InterpValue::i32(2)),
        ],
        vec![
            ("count".into(), InterpValue::i32(1)),
            ("other".into(), InterpValue::String("x".into())),
        ],
        vec![
            ("count".into(), InterpValue::i64(1)),
            ("name".into(), InterpValue::String("x".into())),
        ],
    ] {
        assert!(
            bridge
                .value_to_candidate(
                    bridge.original_program(),
                    &scope,
                    ty,
                    InterpValue::Record(fields.into())
                )
                .is_err()
        );
    }
}

#[test]
fn callable_values_cannot_copy_unpinned_hir_ids_into_a_candidate() {
    let (prepared, scope, ty) = prepared_body("", "() -> i32", "i32", "return 1;");
    let value = InterpValue::Callable(crate::control::CallTarget::FlowItem(prepared.entry().item));
    assert!(matches!(
        prepared.type_bridge().value_to_candidate(
            prepared.type_bridge().original_program(),
            &scope,
            ty,
            value
        ),
        Err(RevisionValueBridgeError::Value {
            failure: RevisionValueFailure::ExecutableScopeRequired,
            ..
        })
    ));
}

#[test]
fn invalid_char_and_wrong_program_are_rejected_before_execution() {
    let (first, scope, ty) = prepared("", "char");
    let (other, _, _) = prepared("", "char");
    let bridge = first.type_bridge();
    assert!(matches!(
        bridge.value_to_candidate(
            bridge.original_program(),
            &scope,
            ty,
            InterpValue::String("two".into())
        ),
        Err(RevisionValueBridgeError::Value {
            failure: RevisionValueFailure::InvalidPrimitive,
            ..
        })
    ));
    assert!(matches!(
        bridge.value_to_candidate(
            other.type_bridge().original_program(),
            &scope,
            ty,
            InterpValue::String("a".into())
        ),
        Err(RevisionValueBridgeError::Type(
            RevisionTypeBridgeError::WrongProgram
        ))
    ));
    assert_eq!(
        roundtrip(&first, &scope, ty, InterpValue::String("\u{754c}".into())),
        InterpValue::String("\u{754c}".into())
    );
}

#[test]
fn sealed_host_handle_keeps_capability_generation_and_origin() {
    let (prepared, scope, ty) = prepared("import std.net.tcp.TcpStream;", "TcpStream");
    let bridge = prepared.type_bridge();
    let stream = etas_host::TcpStreamRef::issued(
        etas_host::StreamHandleRef::issued("host-issued-capability", 17),
        etas_host::ByteStreamOrigin::Tcp {
            host: "approved.example".into(),
            port: 443,
        },
    );
    let handle = crate::value::HostHandleValue::tcp_stream(ty, stream.clone());
    let target_type = bridge
        .to_candidate(bridge.original_program(), &scope, ty)
        .unwrap();
    let value = bridge
        .value_to_candidate(
            bridge.original_program(),
            &scope,
            ty,
            InterpValue::HostHandle(handle),
        )
        .unwrap();
    let InterpValue::HostHandle(translated) = &value else {
        panic!("sealed handle")
    };
    assert_eq!(translated.nominal_type(), target_type);
    assert_eq!(translated.tcp_stream_ref().as_ref(), Some(&stream));
    let value = bridge
        .value_to_original(prepared.program(), &scope, target_type, value)
        .unwrap();
    let InterpValue::HostHandle(restored) = value else {
        panic!("restored sealed handle")
    };
    assert_eq!(restored.nominal_type(), ty);
    assert_eq!(restored.tcp_stream_ref(), Some(stream));
    assert!(
        bridge
            .value_to_candidate(
                bridge.original_program(),
                &scope,
                ty,
                InterpValue::Record(Vec::new().into())
            )
            .is_err()
    );
}

#[test]
fn shared_immutable_subvalues_stay_shared_in_the_candidate() {
    let (prepared, scope, ty) = prepared("type Id = string;", "Array<Id>");
    let bridge = prepared.type_bridge();
    let etas_types::Type::Array(element) = bridge.original_program().type_store.get(ty).unwrap()
    else {
        panic!("array type")
    };
    let original = InterpValue::Nominal {
        ty: *element,
        value: InterpValue::String("shared".into()).into(),
    };
    let original_alias = original.clone();
    let value = InterpValue::Array(vec![original.clone(); 256].into());
    let converted = bridge
        .value_to_candidate(bridge.original_program(), &scope, ty, value)
        .unwrap();
    let InterpValue::Array(values) = converted else {
        panic!("converted array")
    };
    let payload = |value: &InterpValue| match value {
        InterpValue::Nominal { value, .. } => value.shared_capture_identity().unwrap(),
        _ => panic!("converted nominal"),
    };
    let identity = payload(&values.borrow()[0]);
    assert!(
        values
            .borrow()
            .iter()
            .all(|value| payload(value) == identity)
    );
    let InterpValue::Nominal { value, .. } = &original else {
        panic!("original nominal")
    };
    assert_ne!(identity, value.shared_capture_identity().unwrap());
    assert_eq!(value.as_ref(), &InterpValue::String("shared".into()));
    assert_eq!(original, original_alias);
}

#[test]
fn shared_payload_cache_does_not_hide_a_wrong_nominal_tag() {
    let (prepared, scope, ty) = prepared("type A = string; type B = string;", "Array<A>");
    let bridge = prepared.type_bridge();
    let etas_types::Type::Array(element) = bridge.original_program().type_store.get(ty).unwrap()
    else {
        panic!("array type")
    };
    let other = bridge
        .original_program()
        .type_store
        .iter()
        .find_map(|(id, ty)| {
            matches!(ty, etas_types::Type::Nominal(nominal) if nominal.name.ends_with("B"))
                .then_some(id)
        })
        .unwrap();
    let payload: crate::value::SharedValue = InterpValue::String("same".into()).into();
    let value = InterpValue::Array(
        vec![
            InterpValue::Nominal {
                ty: *element,
                value: payload.clone(),
            },
            InterpValue::Nominal {
                ty: other,
                value: payload,
            },
        ]
        .into(),
    );
    assert!(
        matches!(bridge.value_to_candidate(bridge.original_program(), &scope, ty, value), Err(RevisionValueBridgeError::Value { failure: RevisionValueFailure::WrongNominalIdentity { actual }, .. }) if actual == other)
    );
}

#[test]
fn shared_enum_payload_cache_still_checks_each_constructor_identity() {
    let (prepared, scope, ty) = prepared(
        "enum Choice { Number(i32), Text(string), }",
        "Array<Choice>",
    );
    let bridge = prepared.type_bridge();
    let fields: crate::value::SharedFields = vec![InterpValue::i32(7)].into();
    let values = InterpValue::Array(
        vec![
            InterpValue::Variant {
                name: "Number".into(),
                fields: fields.clone(),
            },
            InterpValue::Variant {
                name: "Text".into(),
                fields,
            },
        ]
        .into(),
    );
    assert!(matches!(
        bridge.value_to_candidate(bridge.original_program(), &scope, ty, values),
        Err(RevisionValueBridgeError::Value {
            failure: RevisionValueFailure::InvalidPrimitive,
            ..
        })
    ));
}
