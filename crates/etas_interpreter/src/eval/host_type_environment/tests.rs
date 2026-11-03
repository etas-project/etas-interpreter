use crate::{
    eval::host_value::{host_to_checked_interp_value, host_to_typed_interp_value},
    value::InterpValue,
};
use etas_host::{HostValue, StorageLimits};
use etas_types::{ItemSignature, Type};

#[test]
fn json_host_decode_requires_the_checked_std_identity_and_preserves_it_in_generic_fields() {
    let checked = crate::testing::project::checked_project("module app.main;
        import std.json.JsonValue as StdJson;
        type JsonValue;
        type Holder<T> = { payload: T };
        flow main(actual: StdJson, nested: Holder<StdJson>, fake: JsonValue) -> unit ![] { return; }");
    let ItemSignature::Flow(signature) = &checked.types.item_signatures[&checked.entry.unwrap()]
    else {
        panic!("main signature");
    };
    let json = checked.types.known_std_types.json_value.unwrap();
    assert_eq!(signature.params[0], json);
    assert!(matches!(
        checked.type_store.get(json),
        Some(Type::Nominal(_))
    ));
    let value = || {
        HostValue::Record(
            [("answer".into(), HostValue::Int(42))]
                .into_iter()
                .collect(),
        )
    };
    assert!(matches!(
        host_to_checked_interp_value(value(), json, &checked, &StorageLimits::default()).unwrap(),
        InterpValue::Json(_)
    ));
    let nested = HostValue::Record([("payload".into(), value())].into_iter().collect());
    let nested = host_to_checked_interp_value(
        nested,
        signature.params[1],
        &checked,
        &StorageLimits::default(),
    )
    .unwrap();
    let InterpValue::Nominal { value, .. } = nested else {
        panic!("holder identity was lost");
    };
    let InterpValue::Record(fields) = &*value else {
        panic!("holder representation");
    };
    assert!(matches!(fields.get("payload"), Some(InterpValue::Json(_))));
    assert!(
        host_to_checked_interp_value(
            value_for_negative(),
            signature.params[2],
            &checked,
            &StorageLimits::default()
        )
        .is_err()
    );
    assert!(
        host_to_typed_interp_value(value_for_negative(), json, &checked.type_store).is_err(),
        "a TypeStore without checked std ABI facts cannot decode an opaque value"
    );
}

fn value_for_negative() -> HostValue {
    HostValue::Record(Default::default())
}

#[test]
fn a_named_std_path_is_not_json_abi_evidence() {
    let mut store = etas_types::TypeStore::default();
    let fake = store.intern(Type::Named(etas_types::NamedTypeRef {
        name: "std.json.JsonValue".into(),
        source_parameter: None,
    }));
    assert!(host_to_typed_interp_value(value_for_negative(), fake, &store).is_err());
}
