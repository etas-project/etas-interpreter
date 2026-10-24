use std::collections::{HashMap, HashSet};

use super::*;

pub(super) fn decode(
    fields: Vec<(String, HostValue)>,
    record: &etas_types::RecordType,
    store: &etas_types::TypeStore,
    substitutions: &crate::eval::host_type_environment::HostTypeEnvironment<'_>,
) -> Result<InterpValue, String> {
    let mut indexed = HashMap::with_capacity(fields.len());
    let known: HashSet<_> = record
        .fields
        .iter()
        .map(|field| field.name.as_str())
        .collect();
    let mut unknown = None;
    for (name, value) in fields {
        if !known.contains(name.as_str()) && unknown.is_none() {
            unknown = Some(name.clone());
        }
        match indexed.entry(name) {
            std::collections::hash_map::Entry::Occupied(entry) => {
                return Err(format!(
                    "host record contains duplicate field `{}`",
                    entry.key()
                ));
            }
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(value);
            }
        }
    }
    if let Some(name) = unknown {
        return Err(format!("host record contains unknown field `{name}`"));
    }
    let mut decoded = Vec::with_capacity(record.fields.len());
    for field in &record.fields {
        let (name, value) = indexed
            .remove_entry(&field.name)
            .ok_or_else(|| format!("host record is missing field `{}`", field.name))?;
        let value =
            host_to_typed_interp_value_with_substitutions(value, field.ty, store, substitutions)
                .map_err(|error| format!("record field `{}`: {error}", field.name))?;
        decoded.push((name, value));
    }
    Ok(InterpValue::Record(RecordValue::new(decoded)))
}

#[cfg(test)]
#[path = "record_tests.rs"]
mod tests;
