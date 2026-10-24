use super::*;
use crate::control::ExecutionFault;
mod validate;

// The caller holds the local slot exclusively. Evaluation/suspension has already
// finished, so a successful borrowed preflight stays valid throughout commit.
pub(super) fn commit(
    mut current: &mut InterpValue,
    mut segments: &[LocalPlaceSegment],
    new_value: InterpValue,
    span: Span,
) -> Result<(), ExecutionFault> {
    validate::path(current, segments, span)?;
    loop {
        let Some((head, tail)) = segments.split_first() else {
            *current = new_value;
            return Ok(());
        };
        if let InterpValue::Nominal { value, .. } = current {
            current = value.make_mut();
            continue;
        }
        current = match (head, current) {
            (LocalPlaceSegment::Field(field), InterpValue::Record(fields)) => {
                fields.field_mut(field).ok_or_else(|| invalidated(span))?
            }
            (LocalPlaceSegment::Index(index), InterpValue::Array(values)) => values
                .borrow_mut()
                .get_mut(*index)
                .ok_or_else(|| invalidated(span))?,
            (LocalPlaceSegment::Index(index), InterpValue::List(values)) => {
                values.get_mut(*index).ok_or_else(|| invalidated(span))?
            }
            (LocalPlaceSegment::MapKey(key), InterpValue::Map(entries)) => {
                if tail.is_empty() {
                    entries.insert((**key).clone(), new_value);
                    return Ok(());
                }
                entries.value_mut(key).ok_or_else(|| invalidated(span))?
            }
            _ => return Err(invalidated(span)),
        };
        segments = tail;
    }
}

fn invalidated(span: Span) -> ExecutionFault {
    ExecutionFault::new(
        AnalysisDiagnosticCode::MissingCheckedFact,
        span,
        "validated local assignment path changed during synchronous commit",
    )
}
