//! Borrow the live source Map entries in insertion order at runtime boundaries.
use super::{InterpValue, RecordValue};

fn payload(mut value: &InterpValue) -> &InterpValue {
    while let InterpValue::Nominal { value: inner, .. } = value {
        value = inner;
    }
    value
}

fn record(value: &InterpValue) -> Result<&RecordValue, &'static str> {
    match payload(value) {
        InterpValue::Record(record) => Ok(record),
        _ => Err("source map storage is not a record"),
    }
}

pub(crate) fn entries(
    map: &InterpValue,
) -> Result<Vec<(&InterpValue, &InterpValue)>, &'static str> {
    let fields = record(map)?;
    let Some(InterpValue::Array(slots)) = fields.get_ref("slots") else {
        return Err("source map has no checked insertion slots");
    };
    let mut entries = Vec::new();
    for slot in slots.borrow() {
        let entry = match payload(slot) {
            InterpValue::OptionNone => continue,
            InterpValue::OptionSome(entry) => entry.as_ref(),
            InterpValue::Variant { name, fields }
                if name.as_str() == "None" && fields.is_empty() =>
            {
                continue;
            }
            InterpValue::Variant { name, fields }
                if name.as_str() == "Some" && fields.len() == 1 =>
            {
                &fields[0]
            }
            _ => return Err("source map slot has no checked Option entry"),
        };
        let entry = record(entry)?;
        let key = entry.get_ref("key").ok_or("source map entry has no key")?;
        let value = entry
            .get_ref("value")
            .ok_or("source map entry has no value")?;
        entries.push((key, value));
    }
    Ok(entries)
}
