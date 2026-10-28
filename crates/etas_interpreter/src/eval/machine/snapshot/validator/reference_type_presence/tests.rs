use super::*;
use etas_types::ItemSignature;

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

#[test]
fn reference_type_presence_reuses_checked_shared_child_results() {
    with_validator(
        r#"
module app.main;
flow main(first: Option<() -> i32 ![]>, second: Option<Option<() -> i32 ![]>>) -> unit { return; }
"#,
        |validator| {
            let Some(ItemSignature::Flow(signature)) = validator
                .checked
                .types
                .item_signatures
                .get(&validator.checked.entry.unwrap())
            else {
                panic!("entry signature");
            };
            assert!(
                validator
                    .type_requires_reference(signature.params[0])
                    .unwrap()
            );
            assert!(
                validator
                    .type_requires_reference(signature.params[1])
                    .unwrap()
            );
            let (_, cost) = crate::testing::allocation::measure(|| {
                for _ in 0..1000 {
                    assert!(
                        validator
                            .type_requires_reference(signature.params[1])
                            .unwrap()
                    );
                }
            });
            assert_eq!(cost.count, 0, "revisited a cached type graph: {cost:?}");
        },
    );
}

#[test]
fn reference_type_presence_does_not_cache_abstract_parameters_as_valid_concrete_slots() {
    with_validator(
        r#"
module app.main;
type Holder<T> = { value: T };
flow generic<T>(value: T) -> unit { return; }
flow main(plain: Holder<i32>) -> unit { return; }
"#,
        |validator| {
            let Some(ItemSignature::Flow(signature)) = validator
                .checked
                .types
                .item_signatures
                .get(&validator.checked.entry.unwrap())
            else {
                panic!("entry signature");
            };
            assert!(
                !validator
                    .type_requires_reference(signature.params[0])
                    .unwrap()
            );
            let generic = validator
                .checked
                .hir
                .items
                .iter()
                .find_map(|(id, item)| match item {
                    etas_hir::HirItem::Flow(flow)
                        if validator.checked.symbols.get(flow.symbol).unwrap().name
                            == "generic" =>
                    {
                        Some(id)
                    }
                    _ => None,
                })
                .unwrap();
            let Some(ItemSignature::Flow(signature)) =
                validator.checked.types.item_signatures.get(&generic)
            else {
                panic!("generic signature");
            };
            for _ in 0..2 {
                let error = validator
                    .type_requires_reference(signature.generic_params[0].subject)
                    .unwrap_err();
                assert!(error.contains("unresolved generic value type"), "{error}");
            }
        },
    );
}
