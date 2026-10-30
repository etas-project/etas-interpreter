use super::*;
use crate::{
    intrinsic::dispatch::CheckedPureIntrinsicCall,
    testing::allocation::measure,
    value::{InterpValue, StringValue},
};
use etas_builtin::text::input::TextInput;
use etas_std::{StdIntrinsicId, intrinsic::pure};
use etas_types::{PrimitiveType, TypeInterner};

fn cold_text(prefix: &str, suffix: &str) -> StringValue {
    let text = StringValue::from(prefix);
    let alias = text.clone();
    let result = text.concat(suffix.into()).unwrap();
    drop(alias);
    assert!(result.contiguous().is_none());
    result
}

fn setup(id: u32) -> (CheckedPureIntrinsicCall, PureAbiProjector) {
    let mut types = TypeInterner::new();
    let string = types.primitive(PrimitiveType::String);
    let projector = PureAbiProjector::build(&types.into_store(), &Default::default()).unwrap();
    (
        CheckedPureIntrinsicCall {
            intrinsic: StdIntrinsicId(id),
            parameter_types: vec![string],
            result_type: string,
        },
        projector,
    )
}

#[test]
fn cold_case_changes_allocate_only_the_result_without_retaining_flat_input() {
    let mut costs = Vec::new();
    for (id, piece, suffix) in [
        (pure::TEXT_LOWERCASE, "AΣ\u{301}Aßİ中😀 ", "AΣ"),
        (pure::TEXT_UPPERCASE, "aßσ中😀 ", "tail"),
    ] {
        let (call, projector) = setup(id);
        for count in [1000, 2000, 4000] {
            let prefix = piece.repeat(count);
            let source = cold_text(&prefix, suffix);
            let flat = format!("{prefix}{suffix}");
            let expected = if id == pure::TEXT_LOWERCASE {
                flat.to_lowercase()
            } else {
                flat.to_uppercase()
            };
            let args = vec![InterpValue::String(source.clone())];
            let (output, cost) =
                measure(|| execute_pure_intrinsic(&call, args, &projector).unwrap());
            let cached = source.contiguous().is_some();
            let InterpValue::String(output) = output else {
                panic!("string")
            };
            assert_eq!(output, expected);
            costs.push((id, count, expected.len(), cost, cached));
            drop(source);
            let (owned, extraction) = measure(|| output.into_string());
            assert_eq!(owned, expected);
            assert_eq!(
                extraction.bytes, 0,
                "result must own its output, not retain input views"
            );
        }
    }
    eprintln!("cold case changed costs: {costs:?}");
    for (id, count, required_output, cost, cached) in costs {
        assert!(
            !cached,
            "case id={id} n={count}: retained input-sized flat cache"
        );
        assert!(
            cost.bytes <= required_output + 256,
            "case id={id} n={count}: {cost:?}, required output={required_output}"
        );
    }
}

#[test]
fn cold_unchanged_cases_share_the_original_backing_without_payload_allocations() {
    let mut costs = Vec::new();
    for (id, piece, suffix) in [
        (pure::TEXT_LOWERCASE, "abc中😀", "tail"),
        (pure::TEXT_UPPERCASE, "ABC中😀", "TAIL"),
    ] {
        let (call, projector) = setup(id);
        for count in [1000, 2000, 4000] {
            let source = cold_text(&piece.repeat(count), suffix);
            let args = vec![InterpValue::String(source.clone())];
            let (output, cost) =
                measure(|| execute_pure_intrinsic(&call, args, &projector).unwrap());
            let cached = source.contiguous().is_some();
            let InterpValue::String(output) = output else {
                panic!("string")
            };
            assert!(output.chunks().eq(source.chunks()));
            costs.push((id, count, cost, cached));
        }
    }
    eprintln!("cold case unchanged costs: {costs:?}");
    for (id, count, cost, cached) in costs {
        assert!(!cached, "case id={id} n={count}: retained flat input");
        assert_eq!(
            (cost.count, cost.bytes),
            (0, 0),
            "case id={id} n={count}: {cost:?}"
        );
    }
}

#[test]
fn lowercase_final_sigma_observes_context_across_chunk_boundaries() {
    let (call, projector) = setup(pure::TEXT_LOWERCASE);
    for (prefix, suffix) in [
        ("A", "Σ"),
        ("AΣ", "\u{301}"),
        ("AΣ\u{301}", "A"),
        ("AΣ.", "A"),
        ("Σ", "AΣ"),
        ("Σ", "Σ"),
        ("AΣ\u{200d}", "A"),
        ("AΣ\u{2b0}", "!"),
        ("AΣ\u{2b0}", "A"),
    ] {
        let source = cold_text(prefix, suffix);
        let output =
            execute_pure_intrinsic(&call, vec![InterpValue::String(source)], &projector).unwrap();
        assert_eq!(
            output,
            InterpValue::String(format!("{prefix}{suffix}").to_lowercase().into())
        );
    }
}

#[test]
fn deep_case_mapping_uses_linear_cursor_scratch_without_an_input_cache() {
    for id in [pure::TEXT_LOWERCASE, pure::TEXT_UPPERCASE] {
        let (call, projector) = setup(id);
        let chunk = format!("{}AΣ\u{301}Aß ", "中".repeat(400));
        for count in [1000, 2000, 4000] {
            let mut source = StringValue::default();
            for _ in 0..count {
                let alias = source.clone();
                source = source.concat(chunk.clone().into()).unwrap();
                drop(alias);
            }
            let expected = if id == pure::TEXT_LOWERCASE {
                chunk.repeat(count).to_lowercase()
            } else {
                chunk.repeat(count).to_uppercase()
            };
            let args = vec![InterpValue::String(source.clone())];
            let (output, cost) =
                measure(|| execute_pure_intrinsic(&call, args, &projector).unwrap());
            assert!(
                source.contiguous().is_none(),
                "id={id} n={count}: input cache"
            );
            assert_eq!(output, InterpValue::String(expected.clone().into()));
            eprintln!(
                "deep case id={id} n={count}: {cost:?}, output bytes={}",
                expected.len()
            );
            assert!(
                cost.bytes <= expected.len() + 256 * count + 4096,
                "id={id} n={count}: {cost:?}"
            );
        }
    }
}

#[test]
fn case_mapping_preserves_checked_nominal_trust_and_invalid_result_rejection() {
    use crate::value::SharedValue;
    use etas_types::{NominalTypeRef, TrustWrapper, Type};
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
    let input = types.intern(Type::Trust {
        wrapper: TrustWrapper::Trusted,
        inner: left,
    });
    let result = types.intern(Type::Trust {
        wrapper: TrustWrapper::Public,
        inner: right,
    });
    let projector = PureAbiProjector::build(&types.into_store(), &Default::default()).unwrap();
    let wrap = |ty, wrapper, value| InterpValue::Trust {
        wrapper,
        value: SharedValue::new(InterpValue::Nominal {
            ty,
            value: SharedValue::new(value),
        }),
    };
    for id in [pure::TEXT_LOWERCASE, pure::TEXT_UPPERCASE] {
        for (prefix, suffix) in [("AΣ", "A"), ("中", "😀")] {
            let source = cold_text(prefix, suffix);
            let valid = wrap(
                left,
                TrustWrapper::Trusted,
                InterpValue::String(source.clone()),
            );
            let call = CheckedPureIntrinsicCall {
                intrinsic: StdIntrinsicId(id),
                parameter_types: vec![input],
                result_type: result,
            };
            let output = execute_pure_intrinsic(&call, vec![valid.clone()], &projector).unwrap();
            let flat = format!("{prefix}{suffix}");
            let expected = if id == pure::TEXT_LOWERCASE {
                flat.to_lowercase()
            } else {
                flat.to_uppercase()
            };
            assert_eq!(
                output,
                wrap(
                    right,
                    TrustWrapper::Public,
                    InterpValue::String(expected.into())
                )
            );
            assert!(source.contiguous().is_none());
            for invalid in [
                InterpValue::String(source.clone()),
                wrap(
                    right,
                    TrustWrapper::Trusted,
                    InterpValue::String(source.clone()),
                ),
                wrap(
                    left,
                    TrustWrapper::Secret,
                    InterpValue::String(source.clone()),
                ),
                wrap(left, TrustWrapper::Trusted, InterpValue::Bool(true)),
            ] {
                assert!(execute_pure_intrinsic(&call, vec![invalid], &projector).is_err());
            }
            assert!(
                execute_pure_intrinsic(
                    &CheckedPureIntrinsicCall {
                        result_type: boolean,
                        ..call
                    },
                    vec![valid],
                    &projector
                )
                .is_err()
            );
        }
    }
}
