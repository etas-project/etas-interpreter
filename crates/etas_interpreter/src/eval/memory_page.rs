use crate::value::InterpValue;
use etas_host::{HostValue, MemoryCursor, MemoryEntry, MemoryVersion};

pub(super) fn page_arguments(
    args: &[InterpValue],
    cursor_type: Option<etas_types::TypeId>,
    limits: &etas_host::StorageLimits,
) -> Result<(Option<MemoryCursor>, u32), String> {
    let [cursor, InterpValue::Number(limit)] = args else {
        return Err("memory page expects an optional cursor and a u32 limit".into());
    };
    let limit = limit
        .as_u32()
        .filter(|limit| *limit > 0)
        .ok_or("memory page limit must be a positive u32")?;
    let cursor = match cursor {
        InterpValue::OptionNone => None,
        InterpValue::OptionSome(value) => {
            let InterpValue::Nominal { ty, value } = value.as_ref() else {
                return Err("memory page cursor lacks checked nominal identity".into());
            };
            if Some(*ty) != cursor_type {
                return Err(
                    "memory page cursor does not match the checked MemoryCursor type".into(),
                );
            }
            let InterpValue::Record(fields) = value.as_ref() else {
                return Err("invalid memory cursor representation".into());
            };
            let fields = fields.borrow();
            let [(name, InterpValue::String(opaque))] = fields.as_slice() else {
                return Err("invalid memory cursor fields".into());
            };
            if name != "opaque" || opaque.len() > limits.max_value_bytes {
                return Err("invalid memory cursor token".into());
            }
            Some(MemoryCursor {
                opaque: opaque.clone(),
            })
        }
        _ => return Err("memory page expects Option<MemoryCursor>".into()),
    };
    Ok((cursor, limit))
}

pub(super) fn page_value(entries: Vec<MemoryEntry>, cursor: Option<MemoryCursor>) -> HostValue {
    HostValue::Record(vec![
        (
            "entries".into(),
            HostValue::List(entries.into_iter().map(entry_value).collect()),
        ),
        (
            "cursor".into(),
            option_value(cursor.map(|cursor| opaque_value(cursor.opaque))),
        ),
    ])
}

pub(super) fn entry_value(entry: MemoryEntry) -> HostValue {
    HostValue::Record(vec![
        ("key".into(), entry.key),
        ("value".into(), entry.value),
        ("version".into(), version_value(entry.version)),
    ])
}

pub(super) fn option_value(value: Option<HostValue>) -> HostValue {
    match value {
        Some(value) => HostValue::Variant {
            name: "Some".into(),
            fields: vec![value],
        },
        None => HostValue::Variant {
            name: "None".into(),
            fields: vec![],
        },
    }
}

fn version_value(version: MemoryVersion) -> HostValue {
    opaque_value(version.as_token().to_owned())
}

fn opaque_value(opaque: String) -> HostValue {
    HostValue::Record(vec![("opaque".into(), HostValue::String(opaque))])
}
