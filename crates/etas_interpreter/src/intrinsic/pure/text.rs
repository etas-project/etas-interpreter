use etas_builtin::text::{
    iteration::{TextSequenceKind, TextSequencePlan},
    ranges::{split_ranges, trim_range},
    transform::TextTransform,
};

use crate::{intrinsic::dispatch::CheckedPureIntrinsicCall, value::InterpValue};

use super::abi::{
    AdapterError, PureAbiProjector, borrowed,
    text::{from_text_range, from_text_ranges},
};

pub(super) fn execute_integer_parse(
    call: &CheckedPureIntrinsicCall,
    args: &[InterpValue],
    projector: &PureAbiProjector,
) -> Result<Option<InterpValue>, AdapterError> {
    use etas_builtin::text::parse;
    use etas_std::intrinsic::pure;

    if !matches!(
        call.intrinsic.0,
        pure::TEXT_PARSE_I8
            | pure::TEXT_PARSE_I16
            | pure::TEXT_PARSE_I32
            | pure::TEXT_PARSE_I64
            | pure::TEXT_PARSE_I128
            | pure::TEXT_PARSE_ISIZE
            | pure::TEXT_PARSE_U8
            | pure::TEXT_PARSE_U16
            | pure::TEXT_PARSE_U32
            | pure::TEXT_PARSE_U64
            | pure::TEXT_PARSE_U128
            | pure::TEXT_PARSE_USIZE
    ) {
        return Ok(None);
    }
    let [input] = args else {
        return Err(AdapterError::Arity {
            expected: 1,
            actual: args.len(),
        });
    };
    let input = borrowed::shared_string_for_type(input, call.parameter_types[0], projector)?;
    let parsed = match call.intrinsic.0 {
        pure::TEXT_PARSE_I8 => parse::parse_i8_input(input),
        pure::TEXT_PARSE_I16 => parse::parse_i16_input(input),
        pure::TEXT_PARSE_I32 => parse::parse_i32_input(input),
        pure::TEXT_PARSE_I64 => parse::parse_i64_input(input),
        pure::TEXT_PARSE_I128 => parse::parse_i128_input(input),
        pure::TEXT_PARSE_ISIZE => parse::parse_isize_input(input),
        pure::TEXT_PARSE_U8 => parse::parse_u8_input(input),
        pure::TEXT_PARSE_U16 => parse::parse_u16_input(input),
        pure::TEXT_PARSE_U32 => parse::parse_u32_input(input),
        pure::TEXT_PARSE_U64 => parse::parse_u64_input(input),
        pure::TEXT_PARSE_U128 => parse::parse_u128_input(input),
        pure::TEXT_PARSE_USIZE => parse::parse_usize_input(input),
        _ => unreachable!(),
    };
    let value = parse::result(parsed);
    super::abi::output::from_builtin_for_type(value, call.result_type, projector).map(Some)
}

pub(super) fn execute_join(
    call: &CheckedPureIntrinsicCall,
    args: &[InterpValue],
    projector: &PureAbiProjector,
) -> Result<InterpValue, AdapterError> {
    use super::abi::{AbiShape, input::type_mismatch, output::from_builtin_for_type};
    use etas_builtin::text::join::{JoinError, join_text_projected};
    let [parts, separator] = args else {
        return Err(AdapterError::Arity {
            expected: 2,
            actual: args.len(),
        });
    };
    let (parts, ty) = borrowed::representation_for_type(parts, call.parameter_types[0], projector)?;
    let (Some(AbiShape::Array(inner)), InterpValue::Array(parts)) = (projector.shape(ty), parts)
    else {
        return Err(type_mismatch(ty, parts));
    };
    let parts = parts.borrow();
    let separator =
        match borrowed::shared_string_for_type(separator, call.parameter_types[1], projector) {
            Ok(separator) => separator,
            Err(error) => {
                // Input projection historically reports the first argument's error
                // first. Preserve that ordering without copying its elements.
                for part in parts.iter() {
                    borrowed::shared_string_for_type(part, *inner, projector)?;
                }
                return Err(error);
            }
        };
    let output = join_text_projected(
        parts
            .iter()
            .map(|part| borrowed::shared_string_for_type(part, *inner, projector)),
        separator,
    )
    .map_err(|error| match error {
        JoinError::Projection(error) => error,
        JoinError::LengthOverflow => AdapterError::Builtin(etas_builtin::BuiltinError::Text(
            etas_builtin::TextFailure::LengthOverflow,
        )),
        JoinError::AllocationFailed => {
            AdapterError::Builtin(etas_builtin::BuiltinError::ResourceExhausted)
        }
    })?;
    from_builtin_for_type(
        etas_builtin::BuiltinValue::String(output),
        call.result_type,
        projector,
    )
}

pub(super) fn execute_lazy_sequence(
    call: &CheckedPureIntrinsicCall,
    args: &[InterpValue],
    projector: &PureAbiProjector,
) -> Result<Option<InterpValue>, AdapterError> {
    use super::abi::{AbiShape, input::type_mismatch};
    use etas_std::intrinsic::pure;

    let kind = match call.intrinsic.0 {
        pure::TEXT_CHARS => TextSequenceKind::Chars,
        pure::TEXT_CHAR_INDICES => TextSequenceKind::CharIndices,
        pure::TEXT_LINES => TextSequenceKind::Lines,
        pure::TEXT_SPLIT_ITER => TextSequenceKind::Split {
            separator: borrowed::shared_string_for_type(
                &args[1],
                call.parameter_types[1],
                projector,
            )?
            .as_str()
            .to_owned(),
        },
        pure::TEXT_SPLIT_WHITESPACE => TextSequenceKind::Whitespace,
        pure::TEXT_FIND_ALL => TextSequenceKind::Matches {
            pattern: borrowed::shared_string_for_type(
                &args[1],
                call.parameter_types[1],
                projector,
            )?
            .as_str()
            .to_owned(),
        },
        pure::TEXT_GRAPHEMES => TextSequenceKind::Graphemes,
        pure::TEXT_GRAPHEME_INDICES => TextSequenceKind::GraphemeIndices,
        _ => return Ok(None),
    };
    let source =
        borrowed::shared_string_for_type(&args[0], call.parameter_types[0], projector)?.clone();
    let Some(AbiShape::Iterable(item_type)) = projector.shape(call.result_type) else {
        return Err(type_mismatch(
            call.result_type,
            &InterpValue::String(source),
        ));
    };
    let sequence = crate::value::iteration::TextSequenceValue::new(
        source,
        TextSequencePlan::new(kind),
        *item_type,
    )
    .map_err(AdapterError::UnsupportedValue)?;
    Ok(Some(InterpValue::TextSequence(sequence)))
}

pub(super) fn execute_transform(
    transform: TextTransform,
    call: &CheckedPureIntrinsicCall,
    args: &[InterpValue],
    projector: &PureAbiProjector,
) -> Result<InterpValue, AdapterError> {
    if args.len() != transform.arity() {
        return Err(AdapterError::Arity {
            expected: transform.arity(),
            actual: args.len(),
        });
    }
    let source = borrowed::shared_string_for_type(&args[0], call.parameter_types[0], projector)?;
    match transform {
        TextTransform::Trim => from_text_range(
            trim_range(source),
            &mut crate::value::TextRangeProjector::new(source),
            call.result_type,
            projector,
        ),
        TextTransform::Split => {
            let separator =
                borrowed::shared_string_for_type(&args[1], call.parameter_types[1], projector)?;
            let ranges = split_ranges(source, separator).map_err(AdapterError::Builtin)?;
            from_text_ranges(ranges, source, call.result_type, projector)
        }
        TextTransform::Lowercase | TextTransform::Uppercase => {
            use etas_builtin::text::case::{CaseMapping, CaseOutput, map_case};
            let mapping = if matches!(transform, TextTransform::Lowercase) {
                CaseMapping::Lowercase
            } else {
                CaseMapping::Uppercase
            };
            match map_case(source, mapping).map_err(AdapterError::Builtin)? {
                CaseOutput::Unchanged => from_text_range(
                    0..source.len(),
                    &mut crate::value::TextRangeProjector::new(source),
                    call.result_type,
                    projector,
                ),
                CaseOutput::Changed(output) => super::abi::output::from_builtin_for_type(
                    etas_builtin::BuiltinValue::String(output),
                    call.result_type,
                    projector,
                ),
            }
        }
    }
}
