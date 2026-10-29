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
    let prefix = StringValue::from(prefix);
    let alias = prefix.clone();
    let text = prefix.concat(suffix.into()).unwrap();
    drop(alias);
    text
}

#[test]
fn cold_composite_text_queries_do_not_materialize_payload_sized_buffers() {
    let mut types = TypeInterner::new();
    let string = types.primitive(PrimitiveType::String);
    let boolean = types.primitive(PrimitiveType::Bool);
    let size = types.primitive(PrimitiveType::USize);
    let projector = PureAbiProjector::build(&types.into_store(), &Default::default()).unwrap();
    let mut costs = Vec::new();
    for count in [1000, 2000, 4000] {
        let prefix = format!("start{}", "中😀e\u{301}".repeat(count));
        for (id, needle, result_type, expected) in [
            (
                pure::TEXT_CONTAINS,
                Some(("e\u{301}", "end")),
                boolean,
                InterpValue::Bool(true),
            ),
            (
                pure::TEXT_CONTAINS,
                Some(("abs", "ent")),
                boolean,
                InterpValue::Bool(false),
            ),
            (
                pure::TEXT_STARTS_WITH,
                Some(("sta", "rt中")),
                boolean,
                InterpValue::Bool(true),
            ),
            (
                pure::TEXT_ENDS_WITH,
                Some(("e\u{301}", "end")),
                boolean,
                InterpValue::Bool(true),
            ),
            (
                pure::TEXT_LEN,
                None,
                size,
                InterpValue::usize(count * 4 + 8),
            ),
            (
                pure::LIST_LEN,
                None,
                size,
                InterpValue::usize(count * 4 + 8),
            ),
            (pure::LIST_IS_EMPTY, None, boolean, InterpValue::Bool(false)),
        ] {
            // Every query gets fresh cold source AND pattern backing.
            let text = cold_text(&prefix, "end");
            let pattern = needle.map(|(a, b)| cold_text(a, b));
            let args = std::iter::once(InterpValue::String(text.clone()))
                .chain(pattern.clone().map(InterpValue::String))
                .collect::<Vec<_>>();
            let call = CheckedPureIntrinsicCall {
                intrinsic: StdIntrinsicId(id),
                parameter_types: vec![string; args.len()],
                result_type,
            };
            let (actual, cost) =
                measure(|| execute_pure_intrinsic(&call, args, &projector).unwrap());
            assert_eq!(actual, expected, "id={id} n={count}");
            assert_eq!(text.len(), prefix.len() + 3);
            assert!(text.contiguous().is_none());
            assert!(
                pattern
                    .as_ref()
                    .is_none_or(|text| text.contiguous().is_none())
            );
            costs.push((id, count, cost));
        }
    }
    eprintln!("cold composite query allocation costs: {costs:?}");
    for (id, count, cost) in costs {
        assert!(cost.bytes < 4096, "id={id} n={count}: {cost:?}");
    }
}

#[test]
fn cold_composite_integer_parsing_keeps_results_without_payload_staging() {
    let mut types = TypeInterner::new();
    let string = types.primitive(PrimitiveType::String);
    let integer = types.primitive(PrimitiveType::I32);
    let result = types.intern(etas_types::Type::Result {
        ok: integer,
        err: string,
    });
    let projector = PureAbiProjector::build(&types.into_store(), &Default::default()).unwrap();
    let call = CheckedPureIntrinsicCall {
        intrinsic: StdIntrinsicId(pure::TEXT_PARSE_I32),
        parameter_types: vec![string],
        result_type: result,
    };
    for count in [1000, 2000, 4000] {
        for (sign, suffix) in [
            ("", "2147483647"),
            ("-", "2147483648"),
            ("", "2147483648x"),
            ("", "中"),
        ] {
            let prefix = format!("{sign}{}", "0".repeat(count));
            let expected = match format!("{prefix}{suffix}").parse::<i32>() {
                Ok(value) => InterpValue::Variant {
                    name: "Ok".into(),
                    fields: vec![InterpValue::i32(value)].into(),
                },
                Err(error) => InterpValue::Variant {
                    name: "Err".into(),
                    fields: vec![InterpValue::String(error.to_string().into())].into(),
                },
            };
            let text = cold_text(&prefix, suffix);
            let args = vec![InterpValue::String(text.clone())];
            let (actual, cost) =
                measure(|| execute_pure_intrinsic(&call, args, &projector).unwrap());
            assert_eq!(actual, expected);
            assert!(text.contiguous().is_none());
            assert!(cost.bytes < 4096, "integer query n={count}: {cost:?}");
        }
    }
}
