use std::{cell::OnceCell, fmt, rc::Rc, sync::Arc};

use super::InterpValue;

/// A storage permutation, not a nominal type identity. Slots use lexical field order.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct RecordLayout {
    sorted_to_storage: Box<[usize]>,
}

impl RecordLayout {
    pub(crate) fn from_names(names: &[impl AsRef<str>]) -> Self {
        let mut slots: Vec<_> = (0..names.len()).collect();
        slots.sort_unstable_by(|a, b| names[*a].as_ref().cmp(names[*b].as_ref()).then(a.cmp(b)));
        Self {
            sorted_to_storage: slots.into_boxed_slice(),
        }
    }

    fn from_values(fields: &[(String, InterpValue)]) -> Self {
        let mut slots: Vec<_> = (0..fields.len()).collect();
        slots.sort_unstable_by(|a, b| fields[*a].0.cmp(&fields[*b].0).then(a.cmp(b)));
        Self {
            sorted_to_storage: slots.into_boxed_slice(),
        }
    }
}

#[derive(Clone)]
struct RecordData {
    fields: Vec<(String, InterpValue)>,
    layout: OnceCell<Arc<RecordLayout>>,
}

impl RecordData {
    fn layout(&self) -> &RecordLayout {
        self.layout
            .get_or_init(|| Arc::new(RecordLayout::from_values(&self.fields)))
    }

    fn field_storage(&self, field: &str) -> Option<usize> {
        let slots = &self.layout().sorted_to_storage;
        let slot = slots.partition_point(|slot| self.fields[*slot].0.as_str() < field);
        let storage = *slots.get(slot)?;
        (self.fields[storage].0 == field).then_some(storage)
    }
}

#[derive(Clone)]
pub struct RecordValue(Rc<RecordData>);

impl RecordValue {
    pub(crate) fn shared_capture_identity(&self) -> Option<*const ()> {
        (Rc::strong_count(&self.0) > 1).then(|| Rc::as_ptr(&self.0).cast())
    }

    pub(super) fn into_unique_values(self) -> Option<Vec<(String, InterpValue)>> {
        Rc::try_unwrap(self.0).ok().map(|data| data.fields)
    }

    pub fn new(fields: Vec<(String, InterpValue)>) -> Self {
        Self(Rc::new(RecordData {
            fields,
            layout: OnceCell::new(),
        }))
    }

    pub(crate) fn with_layout(
        fields: Vec<(String, InterpValue)>,
        layout: Arc<RecordLayout>,
    ) -> Self {
        Self(Rc::new(RecordData {
            fields,
            layout: OnceCell::from(layout),
        }))
    }

    pub fn get(&self, field: &str) -> Option<InterpValue> {
        self.get_ref(field).cloned()
    }

    pub(crate) fn get_ref(&self, field: &str) -> Option<&InterpValue> {
        let data = &self.0;
        let storage = data.field_storage(field)?;
        Some(&data.fields[storage].1)
    }

    pub(crate) fn field_mut(&mut self, field: &str) -> Option<&mut InterpValue> {
        let storage = self.0.field_storage(field)?;
        let data = Rc::make_mut(&mut self.0);
        Some(&mut data.fields[storage].1)
    }

    pub(crate) fn borrow_checked_field(
        &self,
        slot: usize,
        expected_name: &str,
        arity: usize,
    ) -> Result<&InterpValue, &'static str> {
        let data = &self.0;
        if data.fields.len() != arity {
            return Err("record field count differs from checked layout");
        }
        let storage = *data
            .layout()
            .sorted_to_storage
            .get(slot)
            .ok_or("record field slot is outside checked layout")?;
        if data
            .fields
            .get(storage)
            .is_none_or(|(name, _)| name != expected_name)
        {
            return Err("record field name differs from checked layout slot");
        }
        Ok(&data.fields[storage].1)
    }

    pub fn borrow(&self) -> &Vec<(String, InterpValue)> {
        &self.0.fields
    }

    pub fn borrow_mut(&mut self) -> &mut Vec<(String, InterpValue)> {
        let data = Rc::make_mut(&mut self.0);
        // Public mutation can change field names/order as well as values.
        data.layout.take();
        &mut data.fields
    }

    pub fn snapshot(&self) -> Vec<(String, InterpValue)> {
        self.borrow().clone()
    }

    pub fn into_values(self) -> Vec<(String, InterpValue)> {
        match Rc::try_unwrap(self.0) {
            Ok(data) => data.fields,
            Err(shared) => shared.fields.clone(),
        }
    }

    pub fn make_unique(&mut self) {
        if Rc::strong_count(&self.0) > 1 {
            self.0 = Rc::new(self.0.as_ref().clone());
        }
    }
}

impl fmt::Debug for RecordValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("RecordValue").field(&self.borrow()).finish()
    }
}

impl PartialEq for RecordValue {
    fn eq(&self, other: &Self) -> bool {
        *self.borrow() == *other.borrow()
    }
}

impl Eq for RecordValue {}

impl From<Vec<(String, InterpValue)>> for RecordValue {
    fn from(fields: Vec<(String, InterpValue)>) -> Self {
        Self::new(fields)
    }
}
