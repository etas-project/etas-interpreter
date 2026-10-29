use super::*;
use crate::{
    intrinsic::dispatch::CheckedPureIntrinsicCall,
    testing::allocation::measure,
    value::{InterpValue, StringValue},
};
use etas_builtin::text::input::TextInput;
use etas_std::{StdIntrinsicId, intrinsic::pure};
use etas_types::{PrimitiveType, Type, TypeInterner};

fn cold_text(prefix: &str, suffix: &str) -> StringValue {
    let text = StringValue::from(prefix);
    let alias = text.clone();
    let output = text.concat(suffix.into()).unwrap();
    drop(alias);
    output
}

#[test]
fn cold_unchanged_text_transforms_preserve_backing_without_flattening() {
    let mut types = TypeInterner::new();
    let string = types.primitive(PrimitiveType::String);
    let array = types.intern(Type::Array(string));
    let projector = PureAbiProjector::build(&types.into_store(), &Default::default()).unwrap();
    let mut costs = Vec::new();
    for count in [1000, 2000, 4000] {
        let prefix = "中😀e\u{301}".repeat(count);
        for id in [pure::TEXT_TRIM, pure::TEXT_LINES, pure::TEXT_SPLIT] {
            let source = cold_text(&prefix, "tail");
            let mut args = vec![InterpValue::String(source.clone())];
            if id == pure::TEXT_SPLIT {
                args.push(InterpValue::String(cold_text("abs", "ent")));
            }
            let call = CheckedPureIntrinsicCall {
                intrinsic: StdIntrinsicId(id),
                parameter_types: vec![string; args.len()],
                result_type: if id == pure::TEXT_TRIM { string } else { array },
            };
            let (output, cost) =
                measure(|| execute_pure_intrinsic(&call, args, &projector).unwrap());
            let cached = source.contiguous().is_some();
            let text = match &output {
                InterpValue::String(text) => text,
                InterpValue::Array(parts) => {
                    let [InterpValue::String(text)] = parts.borrow().as_slice() else {
                        panic!("one complete text part")
                    };
                    text
                }
                _ => panic!("text output"),
            };
            assert!(text.chunks().eq(source.chunks()));
            costs.push((id, count, cost, cached));
        }
    }
    eprintln!("cold unchanged transform costs: {costs:?}");
    for (id, count, cost, cached) in costs {
        assert!(!cached, "id={id} n={count}: retained flat input");
        assert!(cost.bytes < 4096, "id={id} n={count}: {cost:?}");
    }
}

#[test]
fn cold_partial_trim_copies_only_the_selected_result() {
    let mut types = TypeInterner::new();
    let string = types.primitive(PrimitiveType::String);
    let projector = PureAbiProjector::build(&types.into_store(), &Default::default()).unwrap();
    let call = CheckedPureIntrinsicCall {
        intrinsic: StdIntrinsicId(pure::TEXT_TRIM),
        parameter_types: vec![string],
        result_type: string,
    };
    let mut costs = Vec::new();
    for count in [1000, 2000, 4000] {
        let source = cold_text(
            &" ".repeat(count),
            &format!("x{}", "\u{2003}".repeat(count)),
        );
        let args = vec![InterpValue::String(source.clone())];
        let (output, cost) = measure(|| execute_pure_intrinsic(&call, args, &projector).unwrap());
        costs.push((count, cost, source.contiguous().is_some()));
        drop(source);
        let InterpValue::String(text) = output else {
            panic!("text output")
        };
        let (owned, release_cost) = measure(|| text.into_string());
        assert_eq!(owned, "x");
        assert_eq!(release_cost.bytes, 0, "small result retains no input alias");
    }
    eprintln!("cold partial trim costs: {costs:?}");
    for (count, cost, cached) in costs {
        assert!(!cached, "n={count}: retained flat input");
        assert!(cost.bytes < 512, "n={count}: {cost:?}");
    }
}

#[test]
fn cold_split_and_lines_materialize_only_output_fragments() {
    let mut types = TypeInterner::new();
    let string = types.primitive(PrimitiveType::String);
    let array = types.intern(Type::Array(string));
    let projector = PureAbiProjector::build(&types.into_store(), &Default::default()).unwrap();
    for count in [1000, 2000, 4000] {
        for (id, prefix, suffix, separator) in [
            (
                pure::TEXT_LINES,
                format!("{}tail\r", "中😀\r\n".repeat(count)),
                "\nend",
                None,
            ),
            (
                pure::TEXT_SPLIT,
                format!("{}tail:", "中😀::".repeat(count)),
                ":end",
                Some("::"),
            ),
        ] {
            let source = cold_text(&prefix, suffix);
            let mut cold = vec![InterpValue::String(source.clone())];
            let mut flat = vec![InterpValue::String(format!("{prefix}{suffix}").into())];
            if let Some(separator) = separator {
                cold.push(InterpValue::String(cold_text(
                    &separator[..1],
                    &separator[1..],
                )));
                flat.push(InterpValue::String(separator.into()));
            }
            let call = CheckedPureIntrinsicCall {
                intrinsic: StdIntrinsicId(id),
                parameter_types: vec![string; cold.len()],
                result_type: array,
            };
            let (reference, reference_cost) =
                measure(|| execute_pure_intrinsic(&call, flat, &projector).unwrap());
            let (output, cost) =
                measure(|| execute_pure_intrinsic(&call, cold, &projector).unwrap());
            assert!(source.contiguous().is_none(), "id={id} n={count}");
            assert_eq!(output, reference);
            eprintln!("cold fragments id={id} n={count}: {cost:?}, flat_output={reference_cost:?}");
            assert!(
                cost.bytes <= reference_cost.bytes + 512,
                "input staging id={id} n={count}: {cost:?}"
            );
            drop(source);
            let InterpValue::Array(parts) = output else {
                panic!("parts")
            };
            for part in parts.into_values() {
                let InterpValue::String(part) = part else {
                    panic!("text")
                };
                let (_, extraction) = measure(|| part.into_string());
                assert_eq!(
                    extraction.bytes, 0,
                    "partial output retains no source alias"
                );
            }
        }
    }
}

#[test]
fn malformed_text_ranges_release_partial_output_without_exposing_input() {
    let mut types = TypeInterner::new();
    let string = types.primitive(PrimitiveType::String);
    let array = types.intern(Type::Array(string));
    let projector = PureAbiProjector::build(&types.into_store(), &Default::default()).unwrap();
    let source = cold_text("ok中", "PRIVATE_PAYLOAD_MARKER");
    let (_, cost) = measure(|| {
        let error =
            abi::text::from_text_ranges([0..2, 2..3].into_iter(), &source, array, &projector)
                .unwrap_err();
        assert!(matches!(error, AdapterError::UnsupportedValue(_)));
    });
    assert_eq!(
        cost.bytes, cost.released_bytes,
        "partial output leaked: {cost:?}"
    );
    let error = abi::text::from_text_ranges([0..2, 2..3].into_iter(), &source, array, &projector)
        .unwrap_err();
    assert!(!format!("{error:?}").contains("PRIVATE_PAYLOAD_MARKER"));
    assert!(source.contiguous().is_none());
}

#[test]
fn deep_fragment_projection_uses_bounded_cursor_metadata_not_repeated_root_walks() {
    let mut types = TypeInterner::new();
    let string = types.primitive(PrimitiveType::String);
    let array = types.intern(Type::Array(string));
    let projector = PureAbiProjector::build(&types.into_store(), &Default::default()).unwrap();
    let call = CheckedPureIntrinsicCall {
        intrinsic: StdIntrinsicId(pure::TEXT_LINES),
        parameter_types: vec![string],
        result_type: array,
    };
    let chunk = format!("{}😀\n", "x".repeat(1024));
    for count in [1000, 2000, 4000] {
        let mut source = StringValue::default();
        for _ in 0..count {
            let alias = source.clone();
            source = source.concat(chunk.clone().into()).unwrap();
            drop(alias);
        }
        let flat = vec![InterpValue::String(chunk.repeat(count).into())];
        let args = vec![InterpValue::String(source.clone())];
        let (reference, reference_cost) =
            measure(|| execute_pure_intrinsic(&call, flat, &projector).unwrap());
        let (output, cost) = measure(|| execute_pure_intrinsic(&call, args, &projector).unwrap());
        assert!(source.contiguous().is_none());
        assert_eq!(output, reference);
        eprintln!("deep fragments n={count}: {cost:?}, required_flat_output={reference_cost:?}");
        assert!(
            cost.bytes <= reference_cost.bytes + 128 * count + 4096,
            "repeated root projection: {cost:?}"
        );
    }
}

#[test]
fn cold_join_allocates_the_output_without_caching_input_payloads() {
    let mut types = TypeInterner::new();
    let string = types.primitive(PrimitiveType::String);
    let array = types.intern(Type::Array(string));
    let projector = PureAbiProjector::build(&types.into_store(), &Default::default()).unwrap();
    let call = CheckedPureIntrinsicCall {
        intrinsic: StdIntrinsicId(pure::TEXT_JOIN),
        parameter_types: vec![array, string],
        result_type: string,
    };
    let mut costs = Vec::new();
    for count in [1000, 2000, 4000] {
        let prefix = "x".repeat(1024);
        let input = crate::value::ArrayValue::new(
            (0..count)
                .map(|_| InterpValue::String(cold_text(&prefix, "😀")))
                .collect(),
        );
        let separator = cold_text(":", "😀");
        let args = vec![
            InterpValue::Array(input.clone()),
            InterpValue::String(separator.clone()),
        ];
        let (output, cost) = measure(|| execute_pure_intrinsic(&call, args, &projector).unwrap());
        let cached_inputs = input
            .borrow()
            .iter()
            .filter(|value| match value {
                InterpValue::String(text) => text.contiguous().is_some(),
                _ => unreachable!(),
            })
            .count();
        let cached_separator = separator.contiguous().is_some();
        let InterpValue::String(text) = output else {
            panic!("joined text")
        };
        let output_bytes = count * (1024 + 4) + (count - 1) * 5;
        assert_eq!(text.len(), output_bytes);
        assert_eq!(
            text.into_string(),
            vec![format!("{prefix}😀"); count].join(":😀")
        );
        costs.push((count, output_bytes, cost, cached_inputs, cached_separator));
    }
    eprintln!("cold segmented join costs: {costs:?}");
    for (count, output_bytes, cost, cached_inputs, cached_separator) in costs {
        assert_eq!(cached_inputs, 0, "n={count}: input payloads flattened");
        assert!(!cached_separator, "n={count}: separator payload flattened");
        assert!(cost.bytes <= output_bytes + 4096, "n={count}: {cost:?}");
    }
}
