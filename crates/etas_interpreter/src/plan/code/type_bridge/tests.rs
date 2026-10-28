use super::*;
use etas_code::{DefinitionKey, DefinitionKind, PackageKey};
use etas_frontend::{
    EntryPolicy, Frontend, ProjectInput, SourceInput, code::identity::DefinitionOrigins,
};

fn package_key(name: &[u8]) -> PackageKey {
    PackageKey::from_canonical_fingerprint(CodeFingerprint::of_bytes(
        "etas.test.revision-types",
        name,
    ))
}

fn catalog(source: &str, owner: PackageKey) -> DefinitionCatalog {
    let mut input = ProjectInput::single_source(SourceInput::anonymous(source));
    input.options.entry_policy = EntryPolicy::Optional;
    let output = Frontend.check_project(input);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.unwrap();
    let origins = DefinitionOrigins {
        source_modules: DefinitionOrigins::declared_source_modules(&checked, owner).unwrap(),
        standard_package: package_key(b"std"),
        external_definitions: BTreeMap::new(),
    };
    DefinitionCatalog::build(checked, &origins, Default::default()).unwrap()
}

fn local(catalog: &DefinitionCatalog, name: &str) -> TypeId {
    let checked = catalog.checked_program();
    checked
        .types
        .symbol_types
        .iter()
        .find_map(|(symbol, fact)| {
            if checked.hir.symbols.get(*symbol)?.name != name {
                return None;
            }
            match fact {
                etas_types::SymbolTypeFact::NominalType { constructor, .. } => {
                    Some(TypeId(constructor.0))
                }
                _ => None,
            }
        })
        .expect("checked nominal declaration")
}

fn owner(package: PackageKey, name: &str) -> GenericOwner {
    GenericOwner::Declaration(
        DefinitionKey::new(
            package,
            vec!["app".into()],
            vec![name.into()],
            DefinitionKind::Type,
        )
        .unwrap(),
    )
}

fn bridge(original: &DefinitionCatalog, candidate: &DefinitionCatalog) -> RevisionTypeBridge {
    let runtime_types = runtime::RuntimeTypeBridge::new(original, candidate).unwrap();
    let actions = actions::ActionBridge::new(original, candidate, &runtime_types).unwrap();
    RevisionTypeBridge {
        runtime_owner: Default::default(),
        original: original.checked_program().clone(),
        candidate: candidate.checked_program().clone(),
        forward: correspond(original, candidate).unwrap(),
        backward: correspond(candidate, original).unwrap(),
        original_callables: original
            .native_callables()
            .map(|(id, item)| (id.clone(), item))
            .collect(),
        candidate_callables: candidate
            .native_callables()
            .map(|(id, item)| (id.clone(), item))
            .collect(),
        original_agents: agent_items(original).unwrap(),
        candidate_agents: agent_items(candidate).unwrap(),
        runtime_types,
        actions,
    }
}

const FIRST: &str = "module app; type A = string; type B = string; agent Worker(value: A) -> B { return B(\"b\"); } flow inspect() -> unit { let definition = ^Worker; return; }";
const SECOND: &str = "module app; type B = string; type A = string; agent Worker(value: A) -> B { return B(\"b\"); } flow inspect() -> unit { let definition = ^Worker; return; }";

#[test]
fn reordered_type_arenas_map_by_checked_nominal_identity_not_index_or_representation() {
    let package = package_key(b"app");
    let first = catalog(FIRST, package);
    let second = catalog(SECOND, package);
    let (a, a2, b, b2) = (
        local(&first, "A"),
        local(&second, "A"),
        local(&first, "B"),
        local(&second, "B"),
    );
    assert_ne!(
        a, a2,
        "the regression must cross different native arena indices"
    );
    let bridge = bridge(&first, &second);
    assert_eq!(
        bridge
            .to_candidate(first.checked_program(), &owner(package, "A"), a)
            .unwrap(),
        a2
    );
    assert_eq!(
        bridge
            .to_candidate(first.checked_program(), &owner(package, "B"), b)
            .unwrap(),
        b2
    );
    assert_eq!(
        bridge
            .to_original(second.checked_program(), &owner(package, "A"), a2)
            .unwrap(),
        a
    );
    assert_ne!(a2, b2);
    assert!(
        bridge
            .to_candidate(first.checked_program(), &owner(package, "A"), b)
            .is_err()
    );
    assert!(
        bridge
            .to_candidate(
                first.checked_program(),
                &owner(package, "A"),
                TypeId(u32::MAX)
            )
            .is_err()
    );
}

#[test]
fn same_text_and_native_indices_do_not_bridge_different_package_owners() {
    let package = package_key(b"app");
    let first = catalog(FIRST, package);
    let second = catalog(FIRST, package_key(b"unrelated"));
    let a = local(&first, "A");
    assert_eq!(a, local(&second, "A"));
    let bridge = bridge(&first, &second);
    assert!(
        bridge
            .to_candidate(first.checked_program(), &owner(package, "A"), a)
            .is_err()
    );
    assert!(matches!(
        bridge.to_candidate(second.checked_program(), &owner(package, "A"), a),
        Err(RevisionTypeBridgeError::WrongProgram)
    ));
    assert!(bridge.forward.values().all(Option::is_none));
}

#[test]
fn nominal_values_retag_across_reordered_arenas_and_reject_same_shape_siblings() {
    use crate::value::InterpValue;
    let package = package_key(b"app");
    let first = catalog(FIRST, package);
    let second = catalog(SECOND, package);
    let bridge = bridge(&first, &second);
    let (a, a2, b) = (local(&first, "A"), local(&second, "A"), local(&first, "B"));
    assert_ne!(a, a2);
    let value = InterpValue::Nominal {
        ty: a,
        value: InterpValue::String("a".into()).into(),
    };
    let translated = bridge
        .value_to_candidate(
            first.checked_program(),
            &owner(package, "A"),
            a,
            value.clone(),
        )
        .unwrap();
    assert!(matches!(&translated, InterpValue::Nominal { ty, .. } if *ty == a2));
    assert_eq!(
        bridge
            .value_to_original(
                second.checked_program(),
                &owner(package, "A"),
                a2,
                translated
            )
            .unwrap(),
        value
    );
    let wrong = InterpValue::Nominal {
        ty: b,
        value: InterpValue::String("a".into()).into(),
    };
    assert!(
        matches!(bridge.value_to_candidate(first.checked_program(), &owner(package, "A"), a, wrong), Err(super::super::RevisionValueBridgeError::Value { failure: super::super::RevisionValueFailure::WrongNominalIdentity { actual }, .. }) if actual == b)
    );
    assert!(matches!(
        bridge.value_to_candidate(
            first.checked_program(),
            &owner(package, "A"),
            a,
            InterpValue::String("a".into())
        ),
        Err(super::super::RevisionValueBridgeError::Value {
            failure: super::super::RevisionValueFailure::WrongValueKind { .. },
            ..
        })
    ));
}

#[test]
fn recursive_nominal_origins_are_finite_and_keep_generic_declaration_scope() {
    let package = package_key(b"app");
    let declarations = "enum Tree<T> { Leaf(T), Branch(Tree<T>, Tree<T>), }";
    let main = "agent Worker(value: Tree<i32>) -> Tree<i32> { return value; } flow inspect() -> unit { let definition = ^Worker; return; }";
    let first = catalog(
        &format!("module app; {declarations} type Extra = string; {main}"),
        package,
    );
    let second = catalog(
        &format!("module app; type Extra = string; {declarations} {main}"),
        package,
    );
    let bridge = bridge(&first, &second);
    assert!(!bridge.forward.is_empty());
    assert!(bridge.forward.values().all(Option::is_some));
    assert!(bridge.forward.len() < 128);
}
