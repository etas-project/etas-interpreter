use super::*;

fn key(kind: etas_code::DefinitionKind) -> DefinitionKey {
    DefinitionKey::new(
        etas_code::PackageKey::from_canonical_fingerprint(etas_code::CodeFingerprint::of_bytes(
            "etas.test.identity-snapshot",
            b"package",
        )),
        vec!["app".into()],
        vec!["Worker".into()],
        kind,
    )
    .unwrap()
}

#[test]
fn compiler_identity_snapshots_require_the_current_checked_type_role() {
    let known = KnownCodeTypes {
        agent_id: Some(TypeId(11)),
        definition_id: Some(TypeId(12)),
        callable_id: Some(TypeId(13)),
        tool_ref: Some(TypeId(14)),
        ..Default::default()
    };
    let flow = key(etas_code::DefinitionKind::Flow);
    for (ty, identity) in [
        (
            TypeId(11),
            CodeIdentitySnapshot::Agent(key(etas_code::DefinitionKind::Agent)),
        ),
        (TypeId(12), CodeIdentitySnapshot::Definition(flow.clone())),
        (
            TypeId(13),
            CodeIdentitySnapshot::Callable(CallableId::declaration(flow).unwrap()),
        ),
        (
            TypeId(14),
            CodeIdentitySnapshot::Tool(key(etas_code::DefinitionKind::Tool)),
        ),
    ] {
        let value = identity.restore(&known, ty).unwrap();
        assert_eq!(CodeIdentitySnapshot::capture(&value).unwrap(), identity);
        assert!(identity.restore(&known, TypeId(999)).is_err());
        assert!(identity.restore(&KnownCodeTypes::default(), ty).is_err());
        let snapshot = crate::orchestration::ValueSnapshot::capture(
            &crate::value::InterpValue::CodeMetadata(value),
        )
        .unwrap();
        assert_eq!(snapshot, snapshot.clone());
        assert!(snapshot.restore().unwrap_err().contains("checked program"));
    }
    assert!(
        CodeIdentitySnapshot::Agent(key(etas_code::DefinitionKind::Tool))
            .restore(&known, TypeId(11))
            .is_err()
    );
    assert!(
        CodeIdentitySnapshot::Tool(key(etas_code::DefinitionKind::Agent))
            .restore(&known, TypeId(14))
            .is_err()
    );
}
