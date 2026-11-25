//! The language slicing ABI for the source-owned contiguous array view.
use etas_types::{KnownStdTypes, TypeId, TypeStore};

use super::{ArrayValue, InterpValue, RecordValue};

pub(crate) struct SourceSliceView {
    pub source: ArrayValue,
    pub start: usize,
    pub length: usize,
}

impl SourceSliceView {
    pub fn into_value(self, ty: TypeId) -> InterpValue {
        InterpValue::Nominal {
            ty,
            value: InterpValue::Record(RecordValue::new(vec![
                ("source".into(), InterpValue::Array(self.source)),
                ("start".into(), InterpValue::usize(self.start)),
                ("length".into(), InterpValue::usize(self.length)),
            ]))
            .into(),
        }
    }

    pub fn checked(
        value: &InterpValue,
        known: &KnownStdTypes,
        store: &TypeStore,
    ) -> Result<Option<Self>, String> {
        let InterpValue::Nominal { ty, value } = value else {
            return Ok(None);
        };
        if known.slice_element(store, *ty).is_none() {
            return Ok(None);
        }
        let InterpValue::Record(fields) = value.as_ref() else {
            return Err("source Slice has no checked record representation".into());
        };
        let Some(InterpValue::Array(source)) = fields.get_ref("source") else {
            return Err("source Slice has no checked array backing".into());
        };
        let offset = |field| {
            match fields.get_ref(field) {
                Some(InterpValue::Number(value)) => value.as_usize(),
                _ => None,
            }
            .ok_or_else(|| format!("source Slice has no checked `{field}` offset"))
        };
        Ok(Some(Self {
            source: source.clone(),
            start: offset("start")?,
            length: offset("length")?,
        }))
    }
}
