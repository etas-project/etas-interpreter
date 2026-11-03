use super::*;
use crate::value::AgentDefinitionValue;
use etas_frontend::code::definition::NativeDefinitionStore;

fn definitions(prepared: &PreparedCodeRevision) -> Arc<NativeDefinitionStore> {
    let program = prepared.type_bridge().original_program().clone();
    let package = PackageKey::from_canonical_fingerprint(CodeFingerprint::of_bytes(
        "etas.test.value-bridge",
        b"app",
    ));
    let origins = DefinitionOrigins {
        source_modules: DefinitionOrigins::declared_source_modules(&program, package).unwrap(),
        standard_package: package,
        external_definitions: BTreeMap::new(),
    };
    Arc::new(NativeDefinitionStore::new(
        Arc::new(DefinitionCatalog::build(program, &origins, Default::default()).unwrap()),
        Arc::new(NativeCodeSyntaxOperations::default()),
        Default::default(),
    ))
}

#[test]
fn checked_agent_definition_values_relocate_the_abi_but_keep_the_backend_lease() {
    let (prepared, scope, ty) = prepared_body(
        "",
        "std.compiler.AgentDef<Worker>",
        "string",
        "return \"ok\";",
    );
    let backend = definitions(&prepared);
    let program = prepared.type_bridge().original_program();
    let expr = *program.types.reflections.keys().next().unwrap();
    let definition = backend.reflect_agent(expr).unwrap();
    let value = AgentDefinitionValue::new(ty, definition.clone(), backend.clone());
    let target = prepared
        .type_bridge()
        .value_to_candidate(program, &scope, ty, InterpValue::AgentDefinition(value))
        .unwrap();
    let InterpValue::AgentDefinition(target) = target else {
        panic!("definition")
    };
    assert!(Arc::ptr_eq(target.program(), prepared.program()));
    assert!(Arc::ptr_eq(target.backend(), &backend));
    assert!(target.definition().is_same_object(&definition));
    let returned = prepared
        .type_bridge()
        .value_to_original(
            prepared.program(),
            &scope,
            target.ty(),
            InterpValue::AgentDefinition(target),
        )
        .unwrap();
    let InterpValue::AgentDefinition(returned) = returned else {
        panic!("definition")
    };
    assert!(Arc::ptr_eq(returned.program(), program));
    assert_eq!(returned.ty(), ty);
    assert!(returned.definition().is_same_object(&definition));
}

#[test]
fn a_foreign_definition_cannot_use_colliding_local_type_ids_to_enter_a_revision() {
    let (prepared, scope, ty) = prepared_body(
        "",
        "std.compiler.AgentDef<Worker>",
        "string",
        "return \"ok\";",
    );
    let (foreign, _, foreign_ty) = prepared_body(
        "",
        "std.compiler.AgentDef<Worker>",
        "string",
        "return \"ok\";",
    );
    assert_eq!(ty, foreign_ty);
    let backend = definitions(&foreign);
    let expr = *foreign
        .type_bridge()
        .original_program()
        .types
        .reflections
        .keys()
        .next()
        .unwrap();
    let definition = backend.reflect_agent(expr).unwrap();
    let value = AgentDefinitionValue::new(foreign_ty, definition, backend);
    assert!(matches!(
        prepared.type_bridge().value_to_candidate(
            prepared.type_bridge().original_program(),
            &scope,
            ty,
            InterpValue::AgentDefinition(value)
        ),
        Err(RevisionValueBridgeError::Value {
            failure: RevisionValueFailure::WrongProgram,
            ..
        })
    ));
}
