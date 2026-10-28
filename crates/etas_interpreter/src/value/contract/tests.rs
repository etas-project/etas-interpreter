use crate::{
    api::PlanOptions,
    control::{CallTarget, Frame},
    eval::machine::snapshot::SnapshotValidator,
    orchestration::ValueSnapshot,
    value::{InterpValue as V, SharedValue},
};
use etas_types::{ItemSignature, TypeId};

fn checked(source: &str) -> etas_frontend::CheckedProject {
    crate::testing::project::checked_project(source)
}

fn parameter(checked: &etas_frontend::CheckedProject) -> TypeId {
    let Some(ItemSignature::Flow(signature)) =
        checked.types.item_signatures.get(&checked.entry.unwrap())
    else {
        panic!("entry signature");
    };
    signature.params[0]
}

fn validate(checked: &etas_frontend::CheckedProject, value: &V) -> Result<(), String> {
    let plan = crate::Interpreter.plan(checked, PlanOptions).plan.unwrap();
    super::runtime::validate_arguments(
        checked,
        &plan,
        checked.entry.unwrap(),
        std::slice::from_ref(value),
        &Default::default(),
    )
}

fn validate_both(checked: &etas_frontend::CheckedProject, value: &V, valid: bool) {
    let result = validate(checked, value);
    assert_eq!(result.is_ok(), valid, "runtime: {result:?}");
    let plan = crate::Interpreter.plan(checked, PlanOptions).plan.unwrap();
    let limits = Default::default();
    let validator = SnapshotValidator::new(
        checked,
        &plan.slots,
        &plan.dispatch,
        &plan.closures,
        &limits,
    );
    let snapshot = ValueSnapshot::capture(value).unwrap();
    let result = validator.validate_arguments(checked.entry.unwrap(), &[snapshot]);
    assert_eq!(result.is_ok(), valid, "snapshot: {result:?}");
}

#[test]
fn ingress_contract_requires_checked_standard_constant_role_and_owner() {
    use crate::value::StdConstantValue;
    use etas_std::StdValueRole;
    let checked = checked(
        "module app.main; flow main(value: std.host.sandbox.SandboxProfile) -> unit { let canonical = DefaultCommandSandbox; return; }",
    );
    let ty = parameter(&checked);
    let native = StdConstantValue::checked(
        StdValueRole::DefaultCommandSandbox,
        ty,
        &checked.types.known_std_types,
    )
    .unwrap();
    validate_both(&checked, &V::StdConstant(native), true);
    assert!(
        validate(
            &checked,
            &V::HostHandle(crate::value::HostHandleValue::browser_session(
                ty,
                "forged-profile".into()
            ))
        )
        .is_err()
    );
    validate_both(
        &checked,
        &V::StdConstant(StdConstantValue::from_parts(
            StdValueRole::LogicalTraceLabel,
            ty,
        )),
        false,
    );
    validate_both(
        &checked,
        &V::StdConstant(StdConstantValue::from_parts(
            StdValueRole::DefaultCommandSandbox,
            TypeId(u32::MAX),
        )),
        false,
    );
    validate_both(
        &checked,
        &V::Variant {
            name: "DefaultCommandSandbox".into(),
            fields: vec![].into(),
        },
        false,
    );
    let mut missing = checked.clone();
    std::sync::Arc::make_mut(&mut std::sync::Arc::make_mut(&mut missing).types)
        .known_std_types
        .runtime_values
        .clear();
    validate_both(&missing, &V::StdConstant(native), false);
}

#[test]
fn ingress_contract_requires_canonical_session_and_limit_values() {
    for ty in ["SessionId", "ContextPolicy", "RetentionPolicy", "Limit"] {
        let checked = checked(&format!(
            "module app.main; flow main(value: {ty}) -> unit {{ return; }}"
        ));
        let expected = parameter(&checked);
        let raw = match ty {
            "SessionId" => V::String("session".into()),
            _ => V::Variant {
                name: match ty {
                    "ContextPolicy" => "LastTurns",
                    "RetentionPolicy" => "Days",
                    _ => "Tokens",
                }
                .into(),
                fields: vec![V::usize(8)].into(),
            },
        };
        validate_both(&checked, &raw, false);
        let owned = V::Nominal {
            ty: expected,
            value: raw.into(),
        };
        validate_both(&checked, &owned, true);
        let mut wrong = owned.clone();
        let V::Nominal { value, .. } = &mut wrong else {
            unreachable!();
        };
        *value.make_mut() = match ty {
            "SessionId" => V::Bool(true),
            _ => V::Variant {
                name: match ty {
                    "ContextPolicy" => "LastTurns",
                    "RetentionPolicy" => "Days",
                    _ => "Tokens",
                }
                .into(),
                fields: vec![V::i32(8)].into(),
            },
        };
        validate_both(&checked, &wrong, false);
    }
}

#[test]
fn ingress_contract_preserves_nominal_record_identity_and_field_shape() {
    let checked = checked(
        "module app.main; type Row = { x: i32, y: bool }; flow main(value: Row) -> unit { return; }",
    );
    let ty = parameter(&checked);
    let row = |fields| V::Nominal {
        ty,
        value: SharedValue::new(V::Record(fields)),
    };
    validate_both(
        &checked,
        &row(vec![("y".into(), V::Bool(true)), ("x".into(), V::i32(3))].into()),
        true,
    );
    for fields in [
        vec![("y".into(), V::Bool(true)), ("x".into(), V::Bool(false))],
        vec![("x".into(), V::i32(3))],
        vec![("x".into(), V::i32(3)), ("x".into(), V::i32(3))],
        vec![("z".into(), V::Bool(true)), ("x".into(), V::i32(3))],
    ] {
        validate_both(&checked, &row(fields.into()), false);
    }
    validate_both(
        &checked,
        &V::Record(vec![("y".into(), V::Bool(true)), ("x".into(), V::i32(3))].into()),
        false,
    );
}

#[test]
fn ingress_contract_preserves_memory_query_type_evidence() {
    let checked =
        checked("module app.main; flow main(value: MemorySelection<string>) -> unit { return; }");
    let string = checked
        .type_store
        .iter()
        .find_map(|(id, ty)| {
            matches!(
                ty,
                etas_types::Type::Primitive(etas_types::PrimitiveType::String)
            )
            .then_some(id)
        })
        .unwrap();
    let selection = |kind, predicate_type, predicate: Option<V>| V::MemorySelection {
        region_stable_id: "project".into(),
        path: vec!["papers".into()],
        key_type: string,
        value_type: string,
        kind,
        predicate_type,
        predicate: predicate.map(Box::new),
        limit: Some(2),
    };
    use crate::value::MemorySelectionKind as Kind;
    for kind in [Kind::Select, Kind::Query, Kind::RelatedTo] {
        validate_both(
            &checked,
            &selection(kind.clone(), Some(string), Some(V::String("paper".into()))),
            true,
        );
        validate_both(
            &checked,
            &selection(kind.clone(), Some(string), Some(V::Bool(true))),
            false,
        );
        validate_both(
            &checked,
            &selection(kind.clone(), None, Some(V::String("paper".into()))),
            false,
        );
        validate_both(
            &checked,
            &selection(
                kind.clone(),
                Some(TypeId(u32::MAX)),
                Some(V::String("paper".into())),
            ),
            false,
        );
        validate_both(&checked, &selection(kind, Some(string), None), false);
    }
    validate_both(&checked, &selection(Kind::Scan, None, None), true);
    validate_both(
        &checked,
        &selection(Kind::Scan, Some(string), Some(V::String("paper".into()))),
        false,
    );
}

#[test]
fn ingress_contract_checks_recursive_adts_without_recursive_type_expansion() {
    let checked = checked(
        "module app.main; enum Tree { Leaf(i32), Branch(Tree), } flow main(value: Tree) -> unit { return; }",
    );
    let ty = parameter(&checked);
    let wrap = |name: &str, value| V::Nominal {
        ty,
        value: SharedValue::new(V::Variant {
            name: name.into(),
            fields: vec![value].into(),
        }),
    };
    let valid = (0..20_000).fold(wrap("Leaf", V::i32(7)), |inner, _| wrap("Branch", inner));
    validate_both(&checked, &valid, true);
    let invalid = (0..20_000).fold(wrap("Leaf", V::Bool(false)), |inner, _| {
        wrap("Branch", inner)
    });
    validate_both(&checked, &invalid, false);
    validate_both(
        &checked,
        &V::Variant {
            name: "Leaf".into(),
            fields: vec![V::i32(7)].into(),
        },
        false,
    );
}

#[test]
fn ingress_contract_memoizes_shared_adts_and_borrows_their_payloads() {
    let checked = checked(
        "module app.main; enum Tree { Leaf(bytes), Pair(Tree, Tree), } flow main(value: Tree) -> unit { return; }",
    );
    let ty = parameter(&checked);
    let bytes = crate::value::BytesValue::from(vec![7; 1024 * 1024]);
    let pointer = bytes.as_ptr();
    let mut value = V::Nominal {
        ty,
        value: V::Variant {
            name: "Leaf".into(),
            fields: vec![V::Bytes(bytes.clone())].into(),
        }
        .into(),
    };
    for _ in 0..32 {
        value = V::Nominal {
            ty,
            value: V::Variant {
                name: "Pair".into(),
                fields: vec![value.clone(), value].into(),
            }
            .into(),
        };
    }
    validate(&checked, &value).unwrap();
    assert_eq!(bytes.as_ptr(), pointer);
    let V::Nominal { value: backing, .. } = &value else {
        unreachable!();
    };
    let original = backing.as_ref() as *const V;
    validate(&checked, &value).unwrap();
    let V::Nominal { value: backing, .. } = &value else {
        unreachable!();
    };
    assert_eq!(backing.as_ref() as *const V, original);
}

#[test]
fn ingress_contract_validates_canonical_native_collection_elements() {
    for (name, valid, wrong) in [
        (
            "Deque",
            V::Deque(vec![V::i32(7)].into()),
            V::Deque(vec![V::Bool(true)].into()),
        ),
        (
            "Queue",
            V::Queue(vec![V::i32(7)].into()),
            V::Queue(vec![V::Bool(true)].into()),
        ),
        (
            "Stack",
            V::Stack(vec![V::i32(7)].into()),
            V::Stack(vec![V::Bool(true)].into()),
        ),
    ] {
        let checked = checked(&format!(
            "module app.main; flow main(value: {name}<i32>) -> unit {{ return; }}"
        ));
        validate_both(&checked, &valid, true);
        validate_both(&checked, &wrong, false);
        validate_both(&checked, &V::Array(vec![V::i32(7)].into()), false);
    }
}

#[test]
fn ingress_contract_checks_callable_signature_and_effect_row() {
    let checked = checked(
        r#"
module app.main;
flow pure(value: i32) -> i32 ![] { return value; }
flow wrong(value: bool) -> bool ![] { return value; }
flow loud(value: i32) -> i32 ![Console.stdout_write] { perform Console.stdout_write("bad"); return value; }
flow main(callback: i32 -> i32 ![]) -> unit { return; }
"#,
    );
    for (name, valid) in [("pure", true), ("wrong", false), ("loud", false)] {
        let item = checked
            .symbols
            .iter()
            .find_map(|symbol| match symbol.def {
                etas_hir::SymbolDef::Item { item } if symbol.name == name => Some(item),
                _ => None,
            })
            .unwrap();
        validate_both(&checked, &V::Callable(CallTarget::FlowItem(item)), valid);
    }
}

#[test]
fn ingress_contract_checks_selected_lambda_capture_values() {
    let checked = checked(
        r#"
module app.main;
flow make(base: i32) -> i32 -> i32 { return (value: i32) => base + value; }
flow main(callback: i32 -> i32 ![]) -> unit { return; }
"#,
    );
    let expr = checked
        .hir
        .exprs
        .iter()
        .find_map(|(id, expr)| matches!(expr, etas_hir::HirExpr::Lambda { .. }).then_some(id))
        .unwrap();
    let plan = crate::Interpreter.plan(&checked, PlanOptions).plan.unwrap();
    let capture = plan.closures.get(expr).unwrap().captures[0];
    for (value, valid) in [(V::i32(7), true), (V::Bool(true), false)] {
        let frame = Frame::from_snapshot(vec![(capture, value)]).unwrap();
        let input = V::Callable(CallTarget::Lambda {
            expr,
            captured: frame,
        });
        let result = super::runtime::validate_arguments(
            &checked,
            &plan,
            checked.entry.unwrap(),
            &[input],
            &Default::default(),
        );
        assert_eq!(result.is_ok(), valid, "{result:?}");
    }
}

#[test]
fn ingress_contract_requires_the_actual_enum_owner_not_a_matching_variant_label() {
    let checked = checked(
        "module app.main; enum First { Same(i32), } enum Second { Same(i32), } flow witness(value: Second) -> unit { return; } flow main(value: First) -> unit { return; }",
    );
    let other = checked
        .types
        .item_signatures
        .iter()
        .find_map(|(item, signature)| {
            let etas_hir::HirItem::Flow(flow) = checked.hir.items.get(*item)? else {
                return None;
            };
            if checked.symbols.get(flow.symbol)?.name != "witness" {
                return None;
            }
            let ItemSignature::Flow(signature) = signature else {
                return None;
            };
            Some(signature.params[0])
        })
        .unwrap();
    let value = V::Nominal {
        ty: other,
        value: V::Variant {
            name: "Same".into(),
            fields: vec![V::i32(7)].into(),
        }
        .into(),
    };
    validate_both(&checked, &value, false);
}

#[test]
fn ingress_contract_requires_checked_json_numbers_and_unique_keys() {
    use crate::value::HostJsonSupportValue as J;
    let checked = checked(
        "module app.main; import std.json.JsonValue; flow main(value: JsonValue) -> unit { return; }",
    );
    validate_both(
        &checked,
        &V::Json(J::Array(vec![J::NumberBits(7f64.to_bits())].into())),
        true,
    );
    validate_both(
        &checked,
        &V::Json(J::Array(vec![J::NumberBits(f64::NAN.to_bits())].into())),
        false,
    );
    validate_both(
        &checked,
        &V::Json(J::Object(
            vec![("x".into(), J::Bool(true)), ("x".into(), J::Bool(false))].into(),
        )),
        false,
    );
}

#[test]
fn ingress_contract_checks_conversation_payloads_against_the_canonical_json_type() {
    let checked = checked("module app.main; flow main(value: Conversation) -> unit { return; }");
    let conversation = |payload| {
        V::Conversation(crate::value::ConversationValue {
            selected_context: None,
            session: "session".into(),
            history_fence: None,
            cursor: None,
            messages: vec![crate::value::MessageValue {
                id: "message".into(),
                from: None,
                to: None,
                role: crate::value::MessageRoleValue::User,
                session: Some("session".into()),
                created_at: "1".into(),
                payload: SharedValue::new(payload),
                provenance: None,
            }]
            .into(),
        })
    };
    validate_both(
        &checked,
        &conversation(V::Json(crate::value::HostJsonSupportValue::String(
            "hello".into(),
        ))),
        true,
    );
    validate_both(
        &checked,
        &conversation(V::String("not a typed JSON value".into())),
        false,
    );
}

#[test]
fn ingress_contract_rejects_cyclic_closure_capture_frames() {
    let checked = checked(
        "module app.main; flow make(f: () -> unit ![]) -> () -> unit ![] { return () => { f(); }; } flow main(callback: () -> unit ![]) -> unit { return; }",
    );
    let plan = crate::Interpreter.plan(&checked, PlanOptions).plan.unwrap();
    let expr = checked
        .hir
        .exprs
        .iter()
        .find_map(|(id, expr)| matches!(expr, etas_hir::HirExpr::Lambda { .. }).then_some(id))
        .unwrap();
    let capture = plan.closures.get(expr).unwrap().captures[0];
    let mut frame = Frame::from_snapshot(vec![(capture, V::Unit)]).unwrap();
    let value = V::Callable(CallTarget::Lambda {
        expr,
        captured: frame.clone(),
    });
    assert!(frame.set(capture, value.clone()));
    let result = super::runtime::validate_arguments(
        &checked,
        &plan,
        checked.entry.unwrap(),
        &[value],
        &Default::default(),
    );
    assert!(frame.set(capture, V::Unit));
    assert!(result.unwrap_err().contains("cyclic captured frames"));
}

#[test]
fn ingress_contract_preserves_applied_recursive_enum_identity() {
    let checked = checked(
        "module app.main; enum Tree<T> { Leaf(T), Branch(Tree<T>), } flow witness(value: Tree<bool>) -> unit { return; } flow main(value: Tree<i32>) -> unit { return; }",
    );
    let ty = parameter(&checked);
    let leaf = |payload| V::Nominal {
        ty,
        value: V::Variant {
            name: "Leaf".into(),
            fields: vec![payload].into(),
        }
        .into(),
    };
    validate_both(
        &checked,
        &V::Nominal {
            ty,
            value: V::Variant {
                name: "Branch".into(),
                fields: vec![leaf(V::i32(7))].into(),
            }
            .into(),
        },
        true,
    );
    validate_both(&checked, &leaf(V::Bool(true)), false);
}
