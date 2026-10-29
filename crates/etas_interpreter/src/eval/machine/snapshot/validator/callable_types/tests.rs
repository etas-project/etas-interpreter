use super::*;
use etas_hir::HirItemId;
use etas_types::{ItemSignature, SymbolTypeFact};

fn with_validator(source: &str, test: impl FnOnce(&SnapshotValidator<'_>)) {
    let checked = crate::testing::project::checked_project(source);
    let plan = crate::Interpreter
        .plan(&checked, crate::api::PlanOptions)
        .plan
        .unwrap();
    let limits = etas_host::StorageLimits::default();
    test(&SnapshotValidator::new(
        &checked,
        &plan.slots,
        &plan.dispatch,
        &plan.closures,
        &limits,
    ));
}

fn item(validator: &SnapshotValidator<'_>, name: &str) -> HirItemId {
    validator
        .checked
        .symbols
        .iter()
        .find_map(|symbol| match symbol.def {
            etas_hir::SymbolDef::Item { item } if symbol.name == name => Some(item),
            _ => None,
        })
        .unwrap()
}

fn parameter(validator: &SnapshotValidator<'_>, index: usize) -> TypeId {
    let Some(ItemSignature::Flow(signature)) = validator
        .checked
        .types
        .item_signatures
        .get(&item(validator, "main"))
    else {
        panic!("main signature");
    };
    signature.params[index]
}

#[test]
fn checkpoint_callable_cache_keeps_distinct_effect_row_instantiations_separate() {
    with_validator(
        r#"
module app.main;
flow apply<effect E>(callback: () -> unit ![E]) -> unit ![E] { callback(); }
flow main(
    pure: (() -> unit ![]) -> unit ![],
    loud: (() -> unit ![Console.stdout_write]) -> unit ![Console.stdout_write]
) -> unit ![] {
    let selected: (() -> unit ![]) -> unit ![] = apply;
    let loud_selected: (() -> unit ![Console.stdout_write]) -> unit ![Console.stdout_write] = apply;
    return;
}
"#,
        |validator| {
            let pure_type = parameter(validator, 0);
            let loud_type = parameter(validator, 1);
            let Some(Type::Function(loud)) = validator.checked.type_store.get(loud_type) else {
                panic!("loud function");
            };
            let row = loud.effects.clone().unwrap();
            let shared: crate::orchestration::CallTargetSnapshotLink =
                Target::FlowItem(item(validator, "apply")).into();
            let specialize = |row| Target::Specialized {
                target: shared.clone(),
                type_bindings: vec![],
                effect_row_bindings: vec![("E".into(), row)],
            };
            let pure = specialize(etas_types::EffectRowRef {
                effects: vec![],
                tail: None,
            });
            let loud = specialize(row);
            validator.reference_target_type(&pure, pure_type).unwrap();
            validator.reference_target_type(&loud, loud_type).unwrap();
            assert!(validator.reference_target_type(&loud, pure_type).is_err());
            assert!(validator.reference_target_type(&pure, loud_type).is_err());
            validator.reference_target_type(&pure, pure_type).unwrap();
        },
    );
}

#[test]
fn checkpoint_callable_bindings_do_not_share_validation_across_generic_environments() {
    with_validator(
        r#"
module app.main;
spec Allowed;
type Good = i32;
impl Good ~ Allowed;
flow keep<T ~ Allowed>(value: T) -> T { return value; }
flow main(good: Good -> Good, wrong: i32 -> i32) -> Good {
    return keep<Good>(Good(1));
}
"#,
        |validator| {
            let Some(Type::Function(good)) =
                validator.checked.type_store.get(parameter(validator, 0))
            else {
                panic!("good signature");
            };
            let good_type = good.output;
            let Some(Type::Function(wrong)) =
                validator.checked.type_store.get(parameter(validator, 1))
            else {
                panic!("wrong signature");
            };
            let shared: crate::orchestration::CallTargetSnapshotLink =
                Target::FlowItem(item(validator, "keep")).into();
            let specialize = |ty| Target::Specialized {
                effect_row_bindings: Vec::new(),
                target: shared.clone(),
                type_bindings: vec![("T".into(), ty)],
            };
            validator
                .reference_target_type(&specialize(good_type), parameter(validator, 0))
                .unwrap();
            let external = Bindings::from([("T".into(), good_type)]);
            validator
                .reference_target_type_with_bindings(
                    &specialize(good_type),
                    parameter(validator, 0),
                    &external,
                )
                .unwrap();
            for missing in [
                Target::Specialized {
                    effect_row_bindings: Vec::new(),
                    target: shared.clone(),
                    type_bindings: vec![],
                },
                Target::Limited {
                    target: shared.clone(),
                    limits: vec![],
                },
            ] {
                let error = validator
                    .reference_target_type_with_bindings(
                        &missing,
                        parameter(validator, 0),
                        &external,
                    )
                    .unwrap_err();
                assert!(
                    error.contains("missing a bounded generic argument"),
                    "external bindings concealed missing saved specialization: {error}"
                );
            }
            let error = validator
                .reference_target_type(&specialize(wrong.output), parameter(validator, 1))
                .unwrap_err();
            assert!(error.contains("spec bound"), "{error}");
            let composed =
                Target::Composed(vec![specialize(good_type), specialize(wrong.output)].into());
            let error = validator
                .reference_target_type(&composed, parameter(validator, 0))
                .unwrap_err();
            assert!(
                error.contains("spec bound"),
                "generic environment cache skipped validation: {error}"
            );
        },
    );
}

#[test]
fn checkpoint_phantom_arguments_must_match_a_prepared_native_application() {
    with_validator(
        "module app.main;
        type Allowed; type Unchecked;
        flow phantom<R>() -> i32 ![] { return 0; }
        flow main(callback: () -> i32 ![]) -> i32 ![] { return phantom<Allowed>(); }",
        |validator| {
            let nominal = |name: &str| {
                let symbol = validator
                    .checked
                    .symbols
                    .iter()
                    .find(|symbol| symbol.name == name)
                    .unwrap()
                    .id;
                match &validator.checked.types.symbol_types[&symbol] {
                    SymbolTypeFact::NominalType { constructor, .. }
                    | SymbolTypeFact::Type { constructor } => TypeId(constructor.0),
                    _ => panic!("nominal declaration"),
                }
            };
            let target = |name| Target::Specialized {
                target: Target::FlowItem(item(validator, "phantom")).into(),
                type_bindings: vec![("R".into(), nominal(name))],
                effect_row_bindings: vec![],
            };
            let expected = parameter(validator, 0);
            validator
                .reference_target_type(&target("Allowed"), expected)
                .unwrap();
            let error = validator
                .reference_target_type(&target("Unchecked"), expected)
                .unwrap_err();
            assert!(
                error.contains("not prepared by this checked program"),
                "{error}"
            );
            assert!(
                validator
                    .reference_target_type(&Target::FlowItem(item(validator, "phantom")), expected)
                    .is_err()
            );
            validator
                .reference_target_type(&target("Allowed"), expected)
                .unwrap();
        },
    );
}

#[test]
fn checkpoint_source_agent_slots_preserve_nominal_identity_and_callable_kind() {
    with_validator(
        r#"
module app.main;
agent First() -> i32 { return 1; }
agent Second() -> i32 { return 2; }
flow main() -> i32 {
    let selected = First;
    return selected();
}
"#,
        |validator| {
            let ty = validator
                .checked
                .symbols
                .iter()
                .find_map(|symbol| {
                    if symbol.name != "selected" {
                        return None;
                    }
                    match validator.checked.types.symbol_types.get(&symbol.id) {
                        Some(SymbolTypeFact::Local { ty, .. }) => Some(*ty),
                        _ => None,
                    }
                })
                .unwrap();
            validator
                .reference_target_type(&Target::AgentItem(item(validator, "First")), ty)
                .unwrap();
            assert!(
                validator
                    .reference_target_type(&Target::AgentItem(item(validator, "Second")), ty)
                    .unwrap_err()
                    .contains("checked slot type")
            );
            assert!(
                validator
                    .reference_target_type(&Target::FlowItem(item(validator, "First")), ty)
                    .unwrap_err()
                    .contains("callable kind")
            );
        },
    );
}

#[test]
fn checked_callable_interfaces_visit_shared_compositions_without_recursive_expansion() {
    with_validator(
        r#"
module app.main;
flow identity(value: i32) -> i32 { return value; }
flow main(callback: i32 -> i32) -> i32 { return identity(1); }
"#,
        |validator| {
            let mut target = Target::FlowItem(item(validator, "identity"));
            for _ in 0..4096 {
                target = Target::Composed(vec![target.clone(), target].into());
            }
            validator
                .reference_target_type(&target, parameter(validator, 0))
                .unwrap();
            assert!(
                validator
                    .reference_target_type(
                        &Target::Composed(vec![].into()),
                        parameter(validator, 0)
                    )
                    .unwrap_err()
                    .contains("composition is empty")
            );
        },
    );
}

#[test]
fn checked_callable_type_cache_preserves_cow_errors_and_the_original_graph() {
    with_validator(
        r#"
module app.main;
flow good(value: i32) -> i32 { return value; }
flow wrong(value: bool) -> i32 { return 0; }
flow main(callback: i32 -> i32) -> i32 { return good(1); }
"#,
        |validator| {
            let mut target =
                Target::Composed(vec![Target::FlowItem(item(validator, "good"))].into());
            validator
                .reference_target_type(&target, parameter(validator, 0))
                .unwrap();
            let original = target.clone();
            let Target::Composed(children) = &mut target else {
                unreachable!();
            };
            children[0] = Target::FlowItem(item(validator, "wrong"));
            for _ in 0..2 {
                assert!(
                    validator
                        .reference_target_type(&target, parameter(validator, 0))
                        .unwrap_err()
                        .contains("checked slot type")
                );
                validator
                    .reference_target_type(&original, parameter(validator, 0))
                    .unwrap();
            }
        },
    );
}
