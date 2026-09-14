use super::*;
use crate::{
    intrinsic::dispatch::CheckedPureIntrinsicCall, testing::allocation::measure, value::InterpValue,
};
use etas_std::{StdIntrinsicId, intrinsic::pure};
use etas_types::{NominalTypeRef, PrimitiveType, TrustWrapper, Type, TypeInterner};

#[test]
fn text_piece_iteration_does_not_build_an_intermediate_collection() {
    use etas_builtin::text::transform::{TextOutput, TextTransform};
    for count in [1000, 2000, 4000] {
        let source = "x\n".repeat(count);
        for transform in [TextTransform::Lines, TextTransform::Split] {
            let args = if matches!(transform, TextTransform::Split) {
                vec![source.as_str(), "\n"]
            } else {
                vec![source.as_str()]
            };
            let (output, cost) = measure(|| transform.evaluate(&args).unwrap());
            assert_eq!(cost.count, 0);
            let TextOutput::Array(parts) = output else {
                panic!("parts")
            };
            let (actual, cost) = measure(|| {
                parts.fold((0, 0), |(parts, bytes), part| {
                    (parts + 1, bytes + part.len())
                })
            });
            let expected_count = count + usize::from(matches!(transform, TextTransform::Split));
            assert_eq!(actual, (expected_count, count));
            assert_eq!(cost.count, 0);
            assert_eq!(cost.bytes, 0);
        }
    }
}

#[test]
fn text_and_owned_outputs_reject_cyclic_or_missing_checked_result_wrappers() {
    use etas_types::{TypeId, TypeStore};
    let mut types = TypeStore::new();
    let cycle = types.intern(Type::Trust {
        wrapper: TrustWrapper::Public,
        inner: TypeId(0),
    });
    let string = types.intern(Type::Primitive(PrimitiveType::String));
    let projector = PureAbiProjector::build(&types).unwrap();
    for result_type in [cycle, TypeId(999)] {
        let call = CheckedPureIntrinsicCall {
            intrinsic: StdIntrinsicId(pure::TEXT_TRIM),
            parameter_types: vec![string],
            result_type,
        };
        let error =
            execute_pure_intrinsic(&call, vec![InterpValue::String("x".into())], &projector)
                .unwrap_err();
        assert!(matches!(
            error,
            AdapterError::UnsupportedValue(_) | AdapterError::MissingType(_)
        ));
        let error = abi::output::from_builtin_for_type(
            etas_builtin::BuiltinValue::String("x".into()),
            result_type,
            &projector,
        )
        .unwrap_err();
        assert!(matches!(
            error,
            AdapterError::UnsupportedValue(_) | AdapterError::MissingType(_)
        ));
    }
}

#[test]
fn unchanged_text_transforms_share_backing_without_payload_size_allocations() {
    let mut types = TypeInterner::new();
    let string = types.primitive(PrimitiveType::String);
    let array = types.intern(Type::Array(string));
    let projector = PureAbiProjector::build(&types.into_store()).unwrap();
    for id in [pure::TEXT_TRIM, pure::TEXT_SPLIT, pure::TEXT_LINES] {
        let mut first_cost = None;
        for count in [1000, 2000, 4000] {
            let source = "中😀e\u{301}".repeat(count);
            let pointer = source.as_ptr();
            let value = InterpValue::String(source.into());
            let mut args = vec![value.clone()];
            if id == pure::TEXT_SPLIT {
                args.push(InterpValue::String("absent".into()));
            }
            let call = CheckedPureIntrinsicCall {
                intrinsic: StdIntrinsicId(id),
                parameter_types: vec![string; args.len()],
                result_type: if id == pure::TEXT_TRIM { string } else { array },
            };
            let (output, cost) =
                measure(|| execute_pure_intrinsic(&call, args, &projector).unwrap());
            if id == pure::TEXT_TRIM {
                assert_eq!((cost.count, cost.bytes), (0, 0), "n={count}: {cost:?}");
            }
            match &output {
                InterpValue::String(text) => assert_eq!(text.as_ptr(), pointer),
                InterpValue::Array(parts) => {
                    let parts = parts.borrow();
                    assert_eq!(parts.len(), 1);
                    let InterpValue::String(text) = &parts[0] else {
                        panic!("text")
                    };
                    assert_eq!(text.as_ptr(), pointer);
                }
                _ => panic!("text output"),
            }
            let pair = (cost.count, cost.bytes);
            assert_eq!(
                *first_cost.get_or_insert(pair),
                pair,
                "id={id}, n={count}: {cost:?}"
            );
            if id != pure::TEXT_TRIM {
                assert!(cost.bytes < 1000, "only output slots: {cost:?}");
            }
        }
    }
}

#[test]
fn partial_text_results_copy_only_the_result_and_do_not_retain_large_inputs() {
    let mut types = TypeInterner::new();
    let string = types.primitive(PrimitiveType::String);
    let projector = PureAbiProjector::build(&types.into_store()).unwrap();
    let mut first_cost = None;
    for count in [1000, 2000, 4000] {
        let source = format!("{}x{}", " ".repeat(count), "\u{2003}".repeat(count));
        let inner_pointer = source[count..].as_ptr();
        let value = InterpValue::String(source.into());
        let call = CheckedPureIntrinsicCall {
            intrinsic: StdIntrinsicId(pure::TEXT_TRIM),
            parameter_types: vec![string],
            result_type: string,
        };
        let args = vec![value.clone()];
        let (output, cost) = measure(|| execute_pure_intrinsic(&call, args, &projector).unwrap());
        let InterpValue::String(text) = output else {
            panic!("text")
        };
        assert_eq!(text, "x");
        assert_ne!(
            text.as_ptr(),
            inner_pointer,
            "the substring must have its own allocation"
        );
        assert_eq!(cost.count, 2, "one byte plus shared owner header: {cost:?}");
        assert_eq!(*first_cost.get_or_insert(cost.bytes), cost.bytes);
        assert!(cost.bytes < 128);
        drop(value);
        let (owned, cost) = measure(|| text.into_string());
        assert_eq!(owned, "x");
        assert_eq!(cost.count, 0, "no hidden source/view alias");
    }
}

#[test]
fn case_transforms_allocate_output_without_copying_input() {
    let mut types = TypeInterner::new();
    let string = types.primitive(PrimitiveType::String);
    let projector = PureAbiProjector::build(&types.into_store()).unwrap();
    for (id, input, expected) in [
        (pure::TEXT_UPPERCASE, "a", "A"),
        (pure::TEXT_LOWERCASE, "A", "a"),
    ] {
        for count in [1000, 2000, 4000] {
            let value = InterpValue::String(input.repeat(count).into());
            let args = vec![value.clone()];
            let call = CheckedPureIntrinsicCall {
                intrinsic: StdIntrinsicId(id),
                parameter_types: vec![string],
                result_type: string,
            };
            let (output, cost) =
                measure(|| execute_pure_intrinsic(&call, args, &projector).unwrap());
            assert_eq!(output, InterpValue::String(expected.repeat(count).into()));
            assert_eq!(cost.count, 2);
            assert!(
                cost.bytes >= count && cost.bytes < 2 * count,
                "n={count}: {cost:?}"
            );
        }
    }
}

#[test]
fn split_and_lines_preserve_result_values_and_only_materialize_output_fragments() {
    let mut types = TypeInterner::new();
    let string = types.primitive(PrimitiveType::String);
    let array = types.intern(Type::Array(string));
    let projector = PureAbiProjector::build(&types.into_store()).unwrap();
    for (id, source, separator) in [
        (pure::TEXT_LINES, "中\r\n😀\n\n", None),
        (pure::TEXT_LINES, "", None),
        (pure::TEXT_SPLIT, "中😀e\u{301}", Some("")),
        (pure::TEXT_SPLIT, "aaa", Some("aa")),
        (pure::TEXT_SPLIT, "", Some("")),
    ] {
        let mut values = vec![InterpValue::String(source.into())];
        let mut builtin = vec![etas_builtin::BuiltinValue::String(source.into())];
        if let Some(separator) = separator {
            values.push(InterpValue::String(separator.into()));
            builtin.push(etas_builtin::BuiltinValue::String(separator.into()));
        }
        let call = CheckedPureIntrinsicCall {
            intrinsic: StdIntrinsicId(id),
            parameter_types: vec![string; values.len()],
            result_type: array,
        };
        let expected = etas_builtin::call_pure_intrinsic(call.intrinsic, &builtin).unwrap();
        let expected = abi::output::from_builtin_for_type(expected, array, &projector).unwrap();
        assert_eq!(
            execute_pure_intrinsic(&call, values, &projector).unwrap(),
            expected
        );
    }
}

#[test]
fn text_transforms_preserve_checked_nominal_and_trust_output_identity() {
    let mut types = TypeInterner::new();
    let string = types.primitive(PrimitiveType::String);
    let boolean = types.primitive(PrimitiveType::Bool);
    let left = types.intern(Type::Nominal(NominalTypeRef {
        name: "LeftText".into(),
        representation: Some(string),
        params: vec![],
    }));
    let right = types.intern(Type::Nominal(NominalTypeRef {
        name: "RightText".into(),
        representation: Some(string),
        params: vec![],
    }));
    let input_ty = types.intern(Type::Trust {
        wrapper: TrustWrapper::Trusted,
        inner: left,
    });
    let output_ty = types.intern(Type::Trust {
        wrapper: TrustWrapper::Public,
        inner: right,
    });
    let array = types.intern(Type::Array(output_ty));
    let bad_array = types.intern(Type::Array(boolean));
    let projector = PureAbiProjector::build(&types.into_store()).unwrap();
    let text = crate::value::StringValue::from("unchanged");
    let pointer = text.as_ptr();
    let wrap = |ty, wrapper, text| InterpValue::Trust {
        wrapper,
        value: crate::value::SharedValue::new(InterpValue::Nominal {
            ty,
            value: crate::value::SharedValue::new(text),
        }),
    };
    let valid = wrap(
        left,
        TrustWrapper::Trusted,
        InterpValue::String(text.clone()),
    );
    let call = CheckedPureIntrinsicCall {
        intrinsic: StdIntrinsicId(pure::TEXT_TRIM),
        parameter_types: vec![input_ty],
        result_type: output_ty,
    };
    let expected = wrap(
        right,
        TrustWrapper::Public,
        InterpValue::String(text.clone()),
    );
    let output = execute_pure_intrinsic(&call, vec![valid.clone()], &projector).unwrap();
    assert_eq!(output, expected);
    assert_eq!(
        abi::borrowed::string_for_type(&output, output_ty, &projector)
            .unwrap()
            .as_ptr(),
        pointer
    );
    for value in [
        InterpValue::String(text.clone()),
        wrap(
            right,
            TrustWrapper::Trusted,
            InterpValue::String(text.clone()),
        ),
        wrap(
            left,
            TrustWrapper::Secret,
            InterpValue::String(text.clone()),
        ),
        wrap(left, TrustWrapper::Trusted, InterpValue::Bool(true)),
    ] {
        assert!(execute_pure_intrinsic(&call, vec![value], &projector).is_err());
    }
    assert!(
        execute_pure_intrinsic(
            &CheckedPureIntrinsicCall {
                result_type: boolean,
                ..call.clone()
            },
            vec![valid.clone()],
            &projector
        )
        .is_err()
    );
    let split = CheckedPureIntrinsicCall {
        intrinsic: StdIntrinsicId(pure::TEXT_SPLIT),
        parameter_types: vec![input_ty, string],
        result_type: array,
    };
    let output = execute_pure_intrinsic(
        &split,
        vec![valid.clone(), InterpValue::String("missing".into())],
        &projector,
    )
    .unwrap();
    assert_eq!(output, InterpValue::Array(vec![expected].into()));
    assert!(
        execute_pure_intrinsic(
            &CheckedPureIntrinsicCall {
                result_type: bad_array,
                ..split.clone()
            },
            vec![valid.clone(), InterpValue::String("missing".into())],
            &projector
        )
        .is_err()
    );
    assert!(
        execute_pure_intrinsic(
            &split,
            vec![valid.clone(), InterpValue::Bool(false)],
            &projector
        )
        .is_err()
    );
    assert!(execute_pure_intrinsic(&call, vec![], &projector).is_err());
    let wrong_arity = CheckedPureIntrinsicCall {
        parameter_types: vec![input_ty; 2],
        ..call
    };
    assert!(execute_pure_intrinsic(&wrong_arity, vec![valid.clone(), valid], &projector).is_err());
}
