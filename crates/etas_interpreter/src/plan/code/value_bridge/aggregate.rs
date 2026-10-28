use super::{Build, RevisionValueFailure};
use crate::value::{InterpValue, RangeValue, SetValue};

pub(super) fn build(
    kind: Build,
    mut values: Vec<InterpValue>,
) -> Result<InterpValue, RevisionValueFailure> {
    let single = |values: &mut Vec<_>| {
        if values.len() != 1 {
            return Err(RevisionValueFailure::InconsistentLayout);
        }
        values.pop().ok_or(RevisionValueFailure::InconsistentLayout)
    };
    Ok(match kind {
        Build::Nominal(ty) => InterpValue::Nominal {
            ty,
            value: single(&mut values)?.into(),
        },
        Build::Trust(wrapper) => InterpValue::Trust {
            wrapper,
            value: single(&mut values)?.into(),
        },
        Build::Array => InterpValue::Array(values.into()),
        Build::List => InterpValue::List(values.into()),
        Build::Slice => InterpValue::Slice(crate::value::SliceValue::new(values)),
        Build::Set => InterpValue::Set(
            SetValue::from_unique(values).map_err(|_| RevisionValueFailure::InconsistentLayout)?,
        ),
        Build::Tuple => InterpValue::Tuple(values.into()),
        Build::Some => InterpValue::OptionSome(single(&mut values)?.into()),
        Build::Variant(name) => InterpValue::Variant {
            name,
            fields: values.into(),
        },
        Build::Record(names) => {
            if names.len() != values.len() {
                return Err(RevisionValueFailure::InconsistentLayout);
            }
            InterpValue::Record(names.into_iter().zip(values).collect::<Vec<_>>().into())
        }
        Build::Map => {
            if values.len() % 2 != 0 {
                return Err(RevisionValueFailure::InconsistentLayout);
            }
            let mut entries = Vec::with_capacity(values.len() / 2);
            let mut values = values.into_iter();
            while let Some(key) = values.next() {
                let value = values
                    .next()
                    .ok_or(RevisionValueFailure::InconsistentLayout)?;
                entries.push((key, value));
            }
            InterpValue::Map(entries.into())
        }
        Build::Message(mut message) => {
            message.payload = single(&mut values)?.into();
            InterpValue::Message(message)
        }
        Build::Range(bounds) => {
            if values.len() != 2 {
                return Err(RevisionValueFailure::InconsistentLayout);
            }
            let end = values
                .pop()
                .ok_or(RevisionValueFailure::InconsistentLayout)?;
            let start = values
                .pop()
                .ok_or(RevisionValueFailure::InconsistentLayout)?;
            InterpValue::Range(RangeValue {
                start: Box::new(start),
                end: Box::new(end),
                bounds,
            })
        }
    })
}
