//! Borrow the live source Map entries in insertion order at runtime boundaries.
use super::{InterpValue, NumericValue, RecordValue};
use etas_types::{Type, TypeId, TypeStore};

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

/// Keep host entries in insertion order. The Etas Map builds its hash index
/// on the first mutation, using the actual key type's checked Hash method.
pub(crate) fn from_entries(
    store: &TypeStore,
    ty: TypeId,
    entries: impl ExactSizeIterator<Item = (InterpValue, InterpValue)>,
) -> Result<InterpValue, String> {
    let representation = etas_types::ty::applied_representation_in_store(store, ty)
        .map_err(|error| error.to_string())?
        .ok_or("source Map has no checked representation")?;
    let Some(Type::Record(layout)) = store.get(representation) else {
        return Err("source Map representation is not a record".into());
    };
    let field = |name| {
        layout
            .fields
            .iter()
            .find(|f| f.name == name)
            .map(|f| f.ty)
            .ok_or_else(|| format!("source Map has no checked {name} field"))
    };
    let Some(Type::Array(slot)) = store.get(field("slots")?) else {
        return Err("source Map has no checked slot array".into());
    };
    let (entry, source_option) = match store.get(*slot) {
        Some(Type::Option(entry)) => (*entry, false),
        Some(Type::Applied { args, .. }) if args.len() == 1 => (args[0], true),
        _ => return Err("source Map has no checked optional entry type".into()),
    };
    let length = entries.len();
    let slots = entries
        .map(|(key, value)| {
            let entry = nominal_record(
                entry,
                vec![
                    ("key".into(), key),
                    ("value".into(), value),
                    ("hash".into(), InterpValue::Number(NumericValue::U64(0))),
                ],
            );
            if source_option {
                InterpValue::Nominal {
                    ty: *slot,
                    value: InterpValue::Variant {
                        name: "Some".into(),
                        fields: vec![entry].into(),
                    }
                    .into(),
                }
            } else {
                InterpValue::OptionSome(entry.into())
            }
        })
        .collect::<Vec<_>>();
    Ok(nominal_record(
        ty,
        vec![
            ("buckets".into(), InterpValue::Array(Vec::new().into())),
            ("slots".into(), InterpValue::Array(slots.into())),
            (
                "length".into(),
                InterpValue::Number(NumericValue::usize(length)),
            ),
            (
                "base".into(),
                nominal_record(
                    field("base")?,
                    vec![("state".into(), InterpValue::Number(NumericValue::U64(0)))],
                ),
            ),
        ],
    ))
}

fn nominal_record(ty: TypeId, fields: Vec<(String, InterpValue)>) -> InterpValue {
    InterpValue::Nominal {
        ty,
        value: InterpValue::Record(fields.into()).into(),
    }
}
