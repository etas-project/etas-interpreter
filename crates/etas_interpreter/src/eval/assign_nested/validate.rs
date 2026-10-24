use super::*;

pub(super) fn path(
    mut current: &InterpValue,
    segments: &[LocalPlaceSegment],
    span: Span,
) -> Result<(), ExecutionFault> {
    for (position, segment) in segments.iter().enumerate() {
        while let InterpValue::Nominal { value, .. } = current {
            current = value;
        }
        current = match (segment, current) {
            (LocalPlaceSegment::Field(field), InterpValue::Record(fields)) => {
                fields.get_ref(field).ok_or_else(|| {
                    invalid(
                        span,
                        format!("record field `{field}` does not exist at runtime"),
                    )
                })?
            }
            (LocalPlaceSegment::Field(_), other) => {
                return Err(invalid(
                    span,
                    format!(
                        "field assignment requires a local record value, got {}",
                        other.kind_name()
                    ),
                ));
            }
            (LocalPlaceSegment::Index(index), InterpValue::Array(values)) => {
                values.borrow().get(*index).ok_or_else(|| {
                    invalid(
                        span,
                        format!("array index {index} is out of bounds at runtime"),
                    )
                })?
            }
            (LocalPlaceSegment::Index(index), InterpValue::List(values)) => {
                if *index >= values.len() {
                    return Err(invalid(
                        span,
                        format!("list index {index} is out of bounds at runtime"),
                    ));
                }
                // The last index only needs bounds validation; walk its prefix
                // once, during commit, instead of traversing the list twice.
                if position + 1 == segments.len() {
                    return Ok(());
                }
                values.get(*index).ok_or_else(|| invalidated(span))?
            }
            (LocalPlaceSegment::Index(_), other) => {
                return Err(invalid(
                    span,
                    format!(
                        "indexed assignment requires a local array or list value, got {}",
                        other.kind_name()
                    ),
                ));
            }
            (LocalPlaceSegment::MapKey(key), InterpValue::Map(entries)) => {
                // A final map segment is insertion, not projection of an existing key.
                if position + 1 == segments.len() {
                    return Ok(());
                }
                entries.get_ref(key).ok_or_else(|| {
                    invalid(
                        span,
                        "nested map assignment requires an existing key at runtime",
                    )
                })?
            }
            (LocalPlaceSegment::MapKey(_), other) => {
                return Err(invalid(
                    span,
                    format!(
                        "map assignment requires a local map value, got {}",
                        other.kind_name()
                    ),
                ));
            }
        };
    }
    Ok(())
}

fn invalid(span: Span, message: impl Into<String>) -> ExecutionFault {
    ExecutionFault::new(AnalysisDiagnosticCode::InvalidArguments, span, message)
}
