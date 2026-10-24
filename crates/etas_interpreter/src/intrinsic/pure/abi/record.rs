use std::{collections::HashMap, sync::Arc};

use etas_types::{FieldType, TypeId};

use super::AdapterError;
use crate::value::record::RecordLayout;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordAbiLayout {
    fields: Vec<FieldType>,
    slots: HashMap<String, usize>,
    runtime: Arc<RecordLayout>,
}

impl RecordAbiLayout {
    pub(super) fn build(fields: &[FieldType]) -> Result<Self, String> {
        let mut slots = HashMap::with_capacity(fields.len());
        for (slot, field) in fields.iter().enumerate() {
            if slots.insert(field.name.clone(), slot).is_some() {
                return Err(format!(
                    "checked record type has duplicate field `{}`",
                    field.name
                ));
            }
        }
        let names = fields
            .iter()
            .map(|field| field.name.as_str())
            .collect::<Vec<_>>();
        Ok(Self {
            fields: fields.to_vec(),
            slots,
            runtime: Arc::new(RecordLayout::from_names(&names)),
        })
    }

    pub(super) fn fields(&self) -> &[FieldType] {
        &self.fields
    }

    pub(super) fn runtime_layout(&self) -> Arc<RecordLayout> {
        self.runtime.clone()
    }

    pub(super) fn reorder<V>(
        &self,
        mut values: Vec<(String, V)>,
        ty: TypeId,
    ) -> Result<Vec<(String, V)>, AdapterError> {
        if values.len() != self.fields.len() {
            return Err(AdapterError::TypeMismatch {
                expected: ty,
                actual: format!("record with {} field(s)", values.len()),
            });
        }
        for index in 0..values.len() {
            loop {
                #[cfg(test)]
                tests::record_lookup();
                let name = &values[index].0;
                let target =
                    self.slots
                        .get(name)
                        .copied()
                        .ok_or_else(|| AdapterError::TypeMismatch {
                            expected: ty,
                            actual: format!("record with unknown field `{name}`"),
                        })?;
                if target == index {
                    break;
                }
                if values[target].0 == *name {
                    return Err(AdapterError::TypeMismatch {
                        expected: ty,
                        actual: format!("record with duplicate field `{name}`"),
                    });
                }
                // Every swap fixes the destination slot. Detect a duplicate
                // before swapping so malformed input cannot form an endless cycle.
                values.swap(index, target);
            }
        }
        Ok(values)
    }
}

#[cfg(test)]
mod tests;
