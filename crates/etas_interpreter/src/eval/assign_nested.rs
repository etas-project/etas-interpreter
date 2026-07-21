use super::*;
use crate::control::ExecutionFault;

impl<'a> EvalContext<'a> {
    pub(super) fn assign_nested_value(
        &mut self,
        current: &mut InterpValue,
        segments: &[LocalPlaceSegment],
        new_value: InterpValue,
        span: Span,
    ) -> Result<(), ExecutionFault> {
        let Some((head, tail)) = segments.split_first() else {
            *current = new_value;
            return Ok(());
        };
        self.make_value_unique(current);
        if let InterpValue::Nominal { value, .. } = current {
            return self.assign_nested_value(value, segments, new_value, span);
        }
        match head {
            LocalPlaceSegment::Field(field) => match current {
                InterpValue::Record(fields) => {
                    let mut fields = fields.borrow_mut();
                    let Some((_, field_value)) = fields.iter_mut().find(|(name, _)| name == field)
                    else {
                        return Err(ExecutionFault::new(
                            AnalysisDiagnosticCode::InvalidArguments,
                            span,
                            format!("record field `{field}` does not exist at runtime"),
                        ));
                    };
                    self.assign_nested_value(field_value, tail, new_value, span)
                }
                other => Err(ExecutionFault::new(
                    AnalysisDiagnosticCode::InvalidArguments,
                    span,
                    format!(
                        "field assignment requires a local record value, got {:?}",
                        other
                    ),
                )),
            },
            LocalPlaceSegment::Index(index) => match current {
                InterpValue::Array(values) => {
                    let mut values = values.borrow_mut();
                    let Some(slot) = values.get_mut(*index) else {
                        return Err(ExecutionFault::new(
                            AnalysisDiagnosticCode::InvalidArguments,
                            span,
                            format!("array index {index} is out of bounds at runtime"),
                        ));
                    };
                    self.assign_nested_value(slot, tail, new_value, span)
                }
                InterpValue::List(values) => {
                    let mut values = values.borrow_mut();
                    let Some(slot) = values.get_mut(*index) else {
                        return Err(ExecutionFault::new(
                            AnalysisDiagnosticCode::InvalidArguments,
                            span,
                            format!("list index {index} is out of bounds at runtime"),
                        ));
                    };
                    self.assign_nested_value(slot, tail, new_value, span)
                }
                other => Err(ExecutionFault::new(
                    AnalysisDiagnosticCode::InvalidArguments,
                    span,
                    format!(
                        "indexed assignment requires a local array or list value, got {:?}",
                        other
                    ),
                )),
            },
            LocalPlaceSegment::MapKey(key) => match current {
                InterpValue::Map(entries) => {
                    let mut entries = entries.borrow_mut();
                    if tail.is_empty() {
                        if let Some((_, slot)) = entries
                            .iter_mut()
                            .find(|(candidate, _)| candidate == key.as_ref())
                        {
                            *slot = new_value;
                        } else {
                            entries.push(((**key).clone(), new_value));
                        }
                        Ok(())
                    } else {
                        let Some((_, slot)) = entries
                            .iter_mut()
                            .find(|(candidate, _)| candidate == key.as_ref())
                        else {
                            return Err(ExecutionFault::new(
                                AnalysisDiagnosticCode::InvalidArguments,
                                span,
                                "nested map assignment requires an existing key at runtime",
                            ));
                        };
                        self.assign_nested_value(slot, tail, new_value, span)
                    }
                }
                other => Err(ExecutionFault::new(
                    AnalysisDiagnosticCode::InvalidArguments,
                    span,
                    format!("map assignment requires a local map value, got {:?}", other),
                )),
            },
        }
    }

    pub(super) fn make_value_unique(&mut self, value: &mut InterpValue) {
        match value {
            InterpValue::Array(values) => values.make_unique(),
            InterpValue::List(values) => values.make_unique(),
            InterpValue::Map(entries) => entries.make_unique(),
            InterpValue::Deque(values)
            | InterpValue::Queue(values)
            | InterpValue::Stack(values) => values.make_unique(),
            InterpValue::PriorityQueue(entries) | InterpValue::OrderedMap(entries) => {
                entries.make_unique()
            }
            InterpValue::OrderedSet(values) => values.make_unique(),
            InterpValue::Record(fields) => fields.make_unique(),
            InterpValue::Nominal { value, .. } => self.make_value_unique(value),
            _ => {}
        }
    }
}
