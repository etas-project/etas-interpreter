use super::*;

fn identity() -> Value {
    let key = etas_code::DefinitionKey::new(
        etas_code::PackageKey::from_canonical_fingerprint(etas_code::CodeFingerprint::of_bytes(
            "etas.test.identity-codec",
            b"package",
        )),
        vec!["app".into()],
        vec!["Worker".into()],
        etas_code::DefinitionKind::Agent,
    )
    .unwrap();
    json!({"kind":"compiler_identity", "schema":1, "ty":11, "value":crate::value::CodeIdentitySnapshot::Agent(key)})
}

#[test]
fn identity_codec_is_data_only_and_rejects_unknown_contracts() {
    let limits = etas_host::StorageLimits::default();
    let encoded = identity();
    assert!(matches!(
        decode(&limits, &encoded).unwrap(),
        ValueSnapshot::CodeIdentity { .. }
    ));
    assert!(
        super::super::value_from_json(&encoded).is_err(),
        "untyped decode must not construct checked compiler metadata"
    );
    let mut unknown_schema = encoded.clone();
    unknown_schema["schema"] = json!(2);
    assert!(
        decode(&limits, &unknown_schema)
            .unwrap_err()
            .message()
            .contains("schema")
    );
    let mut extra_field = encoded.clone();
    extra_field["executable"] = json!(true);
    assert!(decode(&limits, &extra_field).is_err());
    let mut wrong_role = encoded;
    wrong_role["value"]["role"] = json!("Type");
    assert!(
        decode(&limits, &wrong_role).is_err(),
        "a graph-backed descriptor is not an identity snapshot"
    );
}
