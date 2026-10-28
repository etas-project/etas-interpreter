use crate::{
    eval::host_value::host_to_checked_interp_value, testing::project::checked_project,
    value::InterpValue,
};
use etas_host::{memory::*, *};

fn project(result: &str) -> (etas_frontend::CheckedProject, etas_types::TypeId) {
    let checked = checked_project(&format!(
        "module app.main; flow main(value: {result}) -> {result} {{ return value; }}"
    ));
    let signature = &checked.types.item_signatures[&checked.entry.unwrap()];
    let etas_types::ItemSignature::Flow(signature) = signature else {
        panic!("flow");
    };
    let ty = signature.output;
    (checked, ty)
}

fn store() -> StoreRef {
    StoreRef {
        region: MemoryRegionRef {
            stable_id: "abi".into(),
            schema_fingerprint: Some("schema-v1".into()),
        },
        path: vec!["entries".into()],
    }
}
fn request(intent: MemoryWriteIntent) -> MemoryWriteRequest {
    intent
        .into_request(
            HostRequestId(1),
            AuthorityContext::deny_all(),
            TraceContext::root(TraceId(1)),
            ExecutionBudget::default(),
            &StorageLimits::default(),
        )
        .unwrap()
}
fn variant(name: &str, fields: Vec<HostValue>) -> HostValue {
    HostValue::Variant {
        name: name.into(),
        fields,
    }
}

fn payload_type(
    checked: &etas_frontend::CheckedProject,
    ty: etas_types::TypeId,
    variant: &str,
    field: usize,
) -> etas_types::TypeId {
    let (constructor, args) = match checked.type_store.get(ty).unwrap() {
        etas_types::Type::Applied { constructor, args } => {
            (etas_types::TypeId(constructor.0), args.as_slice())
        }
        etas_types::Type::Enum(_) => (ty, &[][..]),
        other => panic!("expected checked enum, received {other:?}"),
    };
    let layout = &checked.types.enum_layouts[&constructor];
    let bindings = layout
        .type_params
        .iter()
        .cloned()
        .zip(args.iter().copied())
        .collect();
    let field = layout
        .variants
        .iter()
        .find(|item| item.name == variant)
        .unwrap()
        .fields[field];
    etas_types::substitute_named_params_in_store(&checked.type_store, field, &bindings).unwrap()
}

#[tokio::test(flavor = "current_thread")]
async fn real_memory_receipts_decode_through_checked_generic_enum_layouts() {
    check_memory_receipts(&InMemoryMemoryClient::new()).await;
    let workspace = TestWorkspace::create("checked-receipt-abi").unwrap();
    check_memory_receipts(&SqliteMemoryClient::open(workspace.path().join("memory.db")).unwrap())
        .await;
}

async fn check_memory_receipts(client: &impl MemoryClient<Error = HostError>) {
    let intent = MemoryWriteIntent::prepare_put(
        store(),
        HostValue::String("key".into()),
        HostValue::String("secret-value".into()),
        WriteCondition::Missing,
        &StorageLimits::default(),
    )
    .unwrap();
    let operation = intent.operation_ref().clone();
    let result = client.write(request(intent)).await.unwrap().result.unwrap();
    let MemoryWriteResult::Outcome(WriteOutcome::Committed(receipt)) = &result else {
        panic!("commit");
    };
    let version = receipt.change.revision().clone();
    let (checked, ty) = project("WriteOutcome<MemoryWriteReceipt<string>, MemoryWriteRejection>");
    let committed = memory_write_result_value(result);
    let value =
        host_to_checked_interp_value(committed.clone(), ty, &checked, &StorageLimits::default())
            .unwrap();
    assert_eq!(
        super::super::host_value::interp_to_host_value(&value).unwrap(),
        committed
    );
    let InterpValue::Nominal { ty: actual, value } = value else {
        panic!("outcome owner");
    };
    assert_eq!(actual, ty);
    let InterpValue::Variant { name, fields } = value.as_ref() else {
        panic!("outcome");
    };
    assert_eq!(name, "Committed");
    assert!(matches!(&fields[0], InterpValue::Nominal { .. }));
    let (wrong, wrong_ty) = project("WriteOutcome<MemoryWriteReceipt<i32>, MemoryWriteRejection>");
    assert!(
        host_to_checked_interp_value(committed, wrong_ty, &wrong, &StorageLimits::default())
            .is_err()
    );
    let query = MemoryWriteRequest {
        id: HostRequestId(2),
        store: store(),
        operation: MemoryWriteOperation::Reconcile { operation },
        authority: AuthorityContext::deny_all(),
        trace: TraceContext::root(TraceId(2)),
        budget: ExecutionBudget::default(),
    };
    let found = memory_write_result_value(client.write(query).await.unwrap().result.unwrap());
    let (checked, ty) =
        project("ReconcileResult<MemoryWriteReceipt<string>, MemoryWriteRejection>");
    let decoded =
        host_to_checked_interp_value(found.clone(), ty, &checked, &StorageLimits::default())
            .unwrap();
    assert_eq!(
        super::super::host_value::interp_to_host_value(&decoded).unwrap(),
        found
    );
    let InterpValue::Nominal { ty: owner, value } = decoded else {
        panic!("reconcile owner");
    };
    assert_eq!(owner, ty);
    let InterpValue::Variant { name, fields } = value.as_ref() else {
        panic!("reconcile payload");
    };
    assert_eq!(name, "Found");
    let InterpValue::Nominal { ty: outcome, .. } = &fields[0] else {
        panic!("nested outcome owner");
    };
    assert_eq!(*outcome, payload_type(&checked, ty, "Found", 0));
    let deletion = MemoryWriteIntent::prepare_delete(
        store(),
        HostValue::String("key".into()),
        WriteCondition::Match(version),
        &StorageLimits::default(),
    )
    .unwrap();
    let deletion = memory_write_result_value(
        client
            .write(request(deletion))
            .await
            .unwrap()
            .result
            .unwrap(),
    );
    let (checked, ty) = project("WriteOutcome<MemoryWriteReceipt<string>, MemoryWriteRejection>");
    let deleted =
        host_to_checked_interp_value(deletion.clone(), ty, &checked, &StorageLimits::default())
            .unwrap();
    assert_eq!(
        super::super::host_value::interp_to_host_value(&deleted).unwrap(),
        deletion
    );
    let InterpValue::Nominal { ty: actual, value } = deleted else {
        panic!("outcome owner");
    };
    assert_eq!(actual, ty);
    let InterpValue::Variant { fields, .. } = value.as_ref() else {
        panic!("outcome");
    };
    let InterpValue::Nominal {
        ty: receipt_type,
        value,
    } = &fields[0]
    else {
        panic!("receipt");
    };
    assert_eq!(*receipt_type, payload_type(&checked, ty, "Committed", 0));
    let receipt = etas_types::record_fields_with_applied_params(&checked.type_store, *receipt_type)
        .unwrap()
        .unwrap();
    let expected_change = receipt
        .fields
        .iter()
        .find(|field| field.name == "change")
        .unwrap()
        .ty;
    let InterpValue::Record(fields) = &**value else {
        panic!("record");
    };
    let fields = fields.borrow();
    let change = fields.iter().find(|(name, _)| name == "change").unwrap();
    let InterpValue::Nominal {
        ty: change_type,
        value,
    } = &change.1
    else {
        panic!("change owner");
    };
    assert_eq!(*change_type, expected_change);
    assert!(
        matches!(checked.type_store.get(expected_change), Some(etas_types::Type::Enum(declaration)) if declaration.name == "std.memory.MemoryWriteChange")
    );
    let InterpValue::Variant { name, fields } = value.as_ref() else {
        panic!("change");
    };
    assert_eq!(name, "Deleted");
    let InterpValue::Nominal {
        ty: tombstone_type, ..
    } = &fields[0]
    else {
        panic!("tombstone");
    };
    assert_eq!(
        *tombstone_type,
        payload_type(&checked, expected_change, "Deleted", 0)
    );
    assert!(
        matches!(checked.type_store.get(*tombstone_type), Some(etas_types::Type::Nominal(nominal)) if nominal.name == "std.memory.MemoryTombstone")
    );
}

#[test]
fn checked_receipt_rejects_duplicate_target_fields() {
    let (checked, ty) = project("MemoryWriteTarget<string>");
    let value = HostValue::Record(vec![
        ("region".into(), HostValue::String("one".into())),
        ("region".into(), HostValue::String("two".into())),
        ("store".into(), HostValue::List(vec![])),
        ("schema_fingerprint".into(), variant("None", vec![])),
        ("key".into(), HostValue::String("key".into())),
    ]);
    let error =
        host_to_checked_interp_value(value, ty, &checked, &StorageLimits::default()).unwrap_err();
    assert!(error.contains("duplicate field `region`"), "{error}");
}

#[test]
fn checked_enum_decode_uses_the_supplied_limits() {
    let (checked, ty) = project("ReconcileResult<string, i32>");
    let value = variant(
        "Found",
        vec![variant(
            "Committed",
            vec![HostValue::String("payload".repeat(32))],
        )],
    );
    let limits = StorageLimits::default();
    let bytes = limits.value_size(&value).unwrap();
    let strict = StorageLimits {
        max_result_bytes: bytes - 1,
        ..limits.clone()
    };
    assert!(host_to_checked_interp_value(value.clone(), ty, &checked, &strict).is_err());
    let exact = StorageLimits {
        max_result_bytes: bytes,
        max_value_bytes: 1,
        ..limits.clone()
    };
    host_to_checked_interp_value(value.clone(), ty, &checked, &exact).unwrap();
    let shallow = StorageLimits {
        max_depth: 1,
        ..limits.clone()
    };
    assert!(host_to_checked_interp_value(value.clone(), ty, &checked, &shallow).is_err());
    let invalid = StorageLimits {
        max_depth: 65,
        ..limits
    };
    assert!(
        host_to_checked_interp_value(value, ty, &checked, &invalid)
            .unwrap_err()
            .contains("invalid storage limits")
    );
}

#[test]
fn checked_enum_decode_rejects_unknown_wrong_arity_wrong_fields_and_missing_layout() {
    let (mut checked, ty) = project("ReconcileResult<string, i32>");
    let valid = variant(
        "Found",
        vec![variant("Committed", vec![HostValue::String("ok".into())])],
    );
    host_to_checked_interp_value(valid.clone(), ty, &checked, &StorageLimits::default()).unwrap();
    for bad in [
        variant("Unknown", vec![]),
        variant("Found", vec![variant("Unknown", vec![])]),
        variant("Found", vec![variant("Committed", vec![])]),
        variant("Found", vec![variant("Committed", vec![HostValue::Int(3)])]),
        variant("Unresolved", vec![HostValue::String("extra".into())]),
    ] {
        assert!(
            host_to_checked_interp_value(bad, ty, &checked, &StorageLimits::default()).is_err()
        );
    }
    std::sync::Arc::make_mut(&mut std::sync::Arc::make_mut(&mut checked).types)
        .enum_layouts
        .clear();
    assert!(
        host_to_checked_interp_value(valid, ty, &checked, &StorageLimits::default())
            .unwrap_err()
            .contains("layout")
    );
}
