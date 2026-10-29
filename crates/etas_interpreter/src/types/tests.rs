use super::*;
use etas_types::{PrimitiveType, Type, TypeId};
use std::{
    collections::HashMap,
    hash::{Hash, Hasher},
};

fn fixture() -> (etas_frontend::CheckedProject, TypeId, TypeId, TypeId) {
    let checked = crate::testing::project::checked_project(
        "module app.main; enum Growing<T> { Leaf(T), Next(Growing<List<T>>) } flow main() -> unit { return; }",
    );
    let owner = source_enum(&checked);
    let layout = checked.types.enum_layouts.get(&owner).unwrap();
    let recursive = layout.variants[1].fields[0];
    let Some(Type::Applied { args, .. }) = checked.type_store.get(recursive) else {
        panic!("recursive checked application")
    };
    let list = args[0];
    (checked, owner, recursive, list)
}

fn source_enum(checked: &etas_frontend::CheckedProject) -> TypeId {
    let symbol = checked
        .hir
        .items
        .iter()
        .find_map(|(_, item)| match item {
            etas_hir::HirItem::Enum(declaration) => Some(declaration.symbol),
            _ => None,
        })
        .expect("source enum");
    let Some(etas_types::SymbolTypeFact::Type { constructor }) =
        checked.types.symbol_types.get(&symbol)
    else {
        panic!("checked enum constructor")
    };
    TypeId(constructor.0)
}

fn primitive(checked: &etas_frontend::CheckedProject, primitive: PrimitiveType) -> TypeId {
    checked
        .type_store
        .iter()
        .find_map(|(id, ty)| (*ty == Type::Primitive(primitive)).then_some(id))
        .expect("checked primitive")
}

fn hash(value: &CheckedTypeIdentity) -> u64 {
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut hash);
    hash.finish()
}

#[test]
fn closed_nonuniform_instances_do_not_expand_definitions_or_mutate_the_type_store() {
    let (checked, owner, recursive, list) = fixture();
    let original_len = checked.type_store.len();
    let catalog = CheckedTypeCatalog::build(&checked).unwrap();
    let empty = HashMap::new();
    let mut argument = catalog
        .resolve(
            primitive(&checked, PrimitiveType::I32),
            &empty,
            &Default::default(),
        )
        .unwrap();
    let declaration = catalog.declaration(owner, 1).unwrap();
    for _ in 0..1000 {
        let bindings = HashMap::from([("T".into(), argument.clone())]);
        let instance = catalog
            .resolve(recursive, &bindings, &Default::default())
            .unwrap();
        let same = catalog
            .resolve(recursive, &bindings, &Default::default())
            .unwrap();
        assert_eq!(instance.declaration(), Some(declaration));
        assert_eq!(instance, same);
        assert_eq!(hash(&instance), hash(&same));
        let next = catalog
            .resolve(list, &bindings, &Default::default())
            .unwrap();
        assert_eq!(instance.arguments(), std::slice::from_ref(&next));
        let leaf = catalog.variant(&instance, "Leaf").unwrap();
        assert_eq!(leaf.fields, instance.arguments());
        assert!(leaf.names.is_none());
        let child = catalog.variant(&instance, "Next").unwrap();
        assert_eq!(child.fields.len(), 1);
        assert_eq!(child.fields[0].declaration(), instance.declaration());
        assert_eq!(
            child.fields[0].arguments()[0].arguments(),
            instance.arguments()
        );
        argument = next;
    }
    assert_eq!(checked.type_store.len(), original_len);
}

fn parameter_of(checked: &etas_frontend::CheckedProject, name: &str) -> TypeId {
    let item = checked
        .hir
        .items
        .iter()
        .find_map(|(item, data)| match data {
            etas_hir::HirItem::Flow(flow)
                if checked.symbols.get(flow.symbol).unwrap().name == name =>
            {
                Some(item)
            }
            _ => None,
        })
        .unwrap();
    let Some(etas_types::ItemSignature::Flow(signature)) = checked.types.item_signatures.get(&item)
    else {
        panic!("checked callable header")
    };
    signature.params[0]
}

#[test]
fn closed_layouts_apply_named_payloads_without_flattening_nominal_identity() {
    let checked = crate::testing::project::checked_project(
        "module app.main; enum Response<T> { Named { value: T }, Empty } type Inner<T> = { value: T }; type Outer<T> = Inner<T>; flow response<T>(value: Response<List<T>>) -> unit { return; } flow outer<T>(value: Outer<List<T>>) -> unit { return; } flow main() -> unit { return; }",
    );
    let catalog = CheckedTypeCatalog::build(&checked).unwrap();
    let atom = catalog
        .resolve(
            primitive(&checked, PrimitiveType::I32),
            &Default::default(),
            &Default::default(),
        )
        .unwrap();
    let bindings = HashMap::from([("T".into(), atom.clone())]);
    let response = catalog
        .resolve(
            parameter_of(&checked, "response"),
            &bindings,
            &Default::default(),
        )
        .unwrap();
    let payload = catalog.variant(&response, "Named").unwrap();
    assert_eq!(
        payload.names.as_deref(),
        Some(["value".to_owned()].as_slice())
    );
    assert_eq!(payload.fields, response.arguments());
    assert_eq!(payload.fields[0].arguments(), &[atom]);
    assert!(catalog.variant(&response, "Missing").is_err());
    assert!(catalog.representation(&response).is_err());
    let outer = catalog
        .resolve(
            parameter_of(&checked, "outer"),
            &bindings,
            &Default::default(),
        )
        .unwrap();
    let inner = catalog.representation(&outer).unwrap();
    assert_ne!(outer.declaration(), inner.declaration());
    assert!(inner.declaration().is_some());
    let record = catalog.representation(&inner).unwrap();
    assert!(matches!(record.head(), TypeHead::Record(fields) if fields == &["value"]));
    assert_eq!(record.arguments(), inner.arguments());
    assert!(catalog.variant(&outer, "Named").is_err());
}

#[test]
fn closed_type_equality_hash_and_drop_are_stack_safe_for_deep_shared_arguments() {
    let (checked, _, _, list) = fixture();
    let catalog = CheckedTypeCatalog::build(&checked).unwrap();
    let empty = HashMap::new();
    let atom = primitive(&checked, PrimitiveType::I32);
    let mut left = catalog.resolve(atom, &empty, &Default::default()).unwrap();
    let mut right = catalog.resolve(atom, &empty, &Default::default()).unwrap();
    for _ in 0..20_000 {
        left = catalog
            .resolve(
                list,
                &HashMap::from([("T".into(), left)]),
                &Default::default(),
            )
            .unwrap();
        right = catalog
            .resolve(
                list,
                &HashMap::from([("T".into(), right)]),
                &Default::default(),
            )
            .unwrap();
    }
    assert_eq!(left, right);
    assert_eq!(hash(&left), hash(&right));
    drop((left, right));
}

#[test]
fn closed_types_reject_open_arguments_wrong_arity_and_foreign_declarations() {
    let (checked, owner, recursive, list) = fixture();
    let catalog = CheckedTypeCatalog::build(&checked).unwrap();
    let empty = HashMap::new();
    assert!(
        catalog
            .resolve(recursive, &empty, &Default::default())
            .is_err()
    );
    assert!(catalog.resolve(owner, &empty, &Default::default()).is_err());
    assert!(catalog.declaration(owner, 0).is_err());
    let foreign = crate::testing::project::checked_project(
        "module app.main; enum Other { Empty } flow main() -> unit { return; }",
    );
    let other = source_enum(&foreign);
    let foreign = CheckedTypeCatalog::build(&foreign).unwrap();
    let other = foreign.resolve(other, &empty, &Default::default()).unwrap();
    assert!(catalog.validate_identity(&other).is_err());
    assert!(
        catalog
            .resolve(
                list,
                &HashMap::from([("T".into(), other)]),
                &Default::default()
            )
            .is_err()
    );
    assert!(
        catalog
            .declaration_template(DeclarationIdentity {
                compilation: catalog.compilation,
                declaration: u32::MAX,
            })
            .is_err()
    );
}

#[test]
fn closed_arguments_with_different_types_do_not_share_nominal_identity() {
    let (checked, _, recursive, _) = fixture();
    let catalog = CheckedTypeCatalog::build(&checked).unwrap();
    let empty = HashMap::new();
    let int = catalog
        .resolve(
            primitive(&checked, PrimitiveType::I32),
            &empty,
            &Default::default(),
        )
        .unwrap();
    let string = catalog
        .resolve(
            primitive(&checked, PrimitiveType::String),
            &empty,
            &Default::default(),
        )
        .unwrap();
    let left = catalog
        .resolve(
            recursive,
            &HashMap::from([("T".into(), int)]),
            &Default::default(),
        )
        .unwrap();
    let right = catalog
        .resolve(
            recursive,
            &HashMap::from([("T".into(), string)]),
            &Default::default(),
        )
        .unwrap();
    assert_eq!(left.declaration(), right.declaration());
    assert_ne!(left, right);
}

#[test]
fn closed_shared_type_expressions_do_not_expand_exponentially() {
    let checked = crate::testing::project::checked_project(
        "module app.main; flow pair<T>(value: (T, T)) -> unit { return; } flow main() -> unit { return; }",
    );
    let template = checked.type_store.iter().find_map(|(id, ty)| match ty {
        Type::Tuple(elements) if elements.len() == 2 && elements[0] == elements[1] => {
            matches!(checked.type_store.get(elements[0]), Some(Type::Named(parameter)) if parameter.name == "T").then_some(id)
        }
        _ => None,
    }).expect("checked tuple parameter template");
    let catalog = CheckedTypeCatalog::build(&checked).unwrap();
    let atom = primitive(&checked, PrimitiveType::I32);
    let mut left = catalog
        .resolve(atom, &Default::default(), &Default::default())
        .unwrap();
    let mut right = catalog
        .resolve(atom, &Default::default(), &Default::default())
        .unwrap();
    for _ in 0..64 {
        left = catalog
            .resolve(
                template,
                &HashMap::from([("T".into(), left)]),
                &Default::default(),
            )
            .unwrap();
        right = catalog
            .resolve(
                template,
                &HashMap::from([("T".into(), right)]),
                &Default::default(),
            )
            .unwrap();
    }
    assert_eq!(left, right);
    assert_eq!(hash(&left), hash(&right));
    assert_eq!(left.arguments()[0].pointer(), left.arguments()[1].pointer());
}

#[test]
fn closed_callable_types_require_resolved_effect_rows_and_type_selectors() {
    let checked = crate::testing::project::checked_project(
        "module app.main; flow accepts<T, effect E>(f: () -> unit ![Error<T>, E]) -> unit { return; } flow main() -> unit { return; }",
    );
    let template = checked
        .type_store
        .iter()
        .find_map(|(id, ty)| match ty {
            Type::Function(flow)
                if flow.input.is_empty()
                    && flow
                        .effects
                        .as_ref()
                        .is_some_and(|row| row.tail.as_deref() == Some("E")) =>
            {
                Some(id)
            }
            _ => None,
        })
        .expect("checked effect-polymorphic callable");
    let catalog = CheckedTypeCatalog::build(&checked).unwrap();
    let atom = catalog
        .resolve(
            primitive(&checked, PrimitiveType::I32),
            &Default::default(),
            &Default::default(),
        )
        .unwrap();
    let types = HashMap::from([("T".into(), atom)]);
    assert!(
        catalog
            .resolve(template, &types, &Default::default())
            .is_err()
    );
    let cyclic = HashMap::from([(
        "E".into(),
        etas_types::EffectRowRef {
            effects: vec![],
            tail: Some("E".into()),
        },
    )]);
    assert!(catalog.resolve(template, &types, &cyclic).is_err());
    let rows = HashMap::from([(
        "E".into(),
        etas_types::EffectRowRef {
            effects: vec![],
            tail: None,
        },
    )]);
    let closed = catalog.resolve(template, &types, &rows).unwrap();
    let TypeHead::Function {
        effects: Some(effects),
        ..
    } = closed.head()
    else {
        panic!("closed function")
    };
    assert_eq!(effects.len(), 1);
    assert!(matches!(
        &effects[0].arguments[..],
        [super::identity::StaticArgument::Type(_)]
    ));
}

#[test]
fn the_catalog_never_supplies_empty_or_invented_missing_constructor_layouts() {
    let (checked, owner, recursive, _) = fixture();
    let original = CheckedTypeCatalog::build(&checked).unwrap();
    let mut missing = checked.clone();
    let facts = std::sync::Arc::make_mut(&mut std::sync::Arc::make_mut(&mut missing).types);
    facts.enum_layouts.remove(&owner);
    let missing = CheckedTypeCatalog::build(&missing).unwrap();
    let atom = missing
        .resolve(
            primitive(&checked, PrimitiveType::I32),
            &Default::default(),
            &Default::default(),
        )
        .unwrap();
    assert!(
        missing
            .resolve(
                recursive,
                &HashMap::from([("T".into(), atom)]),
                &Default::default()
            )
            .is_err()
    );
    let mut duplicate = checked.clone();
    let facts = std::sync::Arc::make_mut(&mut std::sync::Arc::make_mut(&mut duplicate).types);
    let variants = &mut facts.enum_layouts.get_mut(&owner).unwrap().variants;
    variants[1].name = variants[0].name.clone();
    assert!(CheckedTypeCatalog::build(&duplicate).is_err());
    let mut wrong_owner = checked.clone();
    let facts = std::sync::Arc::make_mut(&mut std::sync::Arc::make_mut(&mut wrong_owner).types);
    let layout = facts.enum_layouts[&owner].clone();
    facts
        .enum_layouts
        .insert(primitive(&checked, PrimitiveType::I32), layout);
    assert!(CheckedTypeCatalog::build(&wrong_owner).is_err());
    let mut changed = checked.clone();
    let facts = std::sync::Arc::make_mut(&mut std::sync::Arc::make_mut(&mut changed).types);
    facts.enum_layouts.get_mut(&owner).unwrap().variants[0].fields[0] =
        primitive(&checked, PrimitiveType::String);
    let changed = CheckedTypeCatalog::build(&changed).unwrap();
    let original_atom = original
        .resolve(
            primitive(&checked, PrimitiveType::I32),
            &Default::default(),
            &Default::default(),
        )
        .unwrap();
    assert!(changed.validate_identity(&original_atom).is_err());
}
