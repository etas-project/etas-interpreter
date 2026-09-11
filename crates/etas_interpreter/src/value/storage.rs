use etas_host::{StorageLimits, memory::MemoryWriteIntent};
use etas_types::TypeId;

#[derive(Clone)]
pub struct MemoryWriteIntentValue {
    pub(crate) ty: TypeId,
    pub(crate) key_type: TypeId,
    pub(crate) value_type: TypeId,
    encoded: String,
    intent: MemoryWriteIntent,
}

impl MemoryWriteIntentValue {
    pub(crate) fn new(
        ty: TypeId,
        key_type: TypeId,
        value_type: TypeId,
        intent: MemoryWriteIntent,
        limits: &StorageLimits,
    ) -> Result<Self, etas_host::HostError> {
        let encoded = intent.encode(limits)?;
        Ok(Self {
            ty,
            key_type,
            value_type,
            intent,
            encoded,
        })
    }
    pub(crate) fn restore(
        ty: TypeId,
        key_type: TypeId,
        value_type: TypeId,
        encoded: &str,
        limits: &StorageLimits,
    ) -> Result<Self, String> {
        let intent = MemoryWriteIntent::decode(encoded, limits).map_err(|e| e.message)?;
        Self::new(ty, key_type, value_type, intent, limits).map_err(|error| error.message)
    }
    pub(crate) fn intent(&self) -> &MemoryWriteIntent {
        &self.intent
    }
    pub(crate) fn encoded(&self) -> &str {
        &self.encoded
    }

    pub(crate) fn validate(
        &self,
        checked: &etas_frontend::CheckedProject,
        limits: &StorageLimits,
    ) -> Result<(), String> {
        self.intent.validate(limits).map_err(|e| e.message)?;
        let constructor =
            crate::eval::resolve_std_type(checked, &["std", "memory", "MemoryWriteIntent"])
                .ok_or("missing checked MemoryWriteIntent type")?;
        let Some(etas_types::Type::Applied {
            constructor: actual,
            args,
        }) = checked.type_store.get(self.ty)
        else {
            return Err("memory intent requires a checked generic nominal type".into());
        };
        if actual.0 != constructor.0 || args.as_slice() != [self.key_type, self.value_type] {
            return Err("memory intent generic identity does not match its payload types".into());
        }
        let (key, value) = match self.intent.mutation() {
            etas_host::memory::MemoryMutation::Put { key, value, .. } => (key, Some(value)),
            etas_host::memory::MemoryMutation::Delete { key, .. } => (key, None),
        };
        crate::eval::host_to_typed_interp_value(key.clone(), self.key_type, &checked.type_store)?;
        if let Some(value) = value {
            crate::eval::host_to_typed_interp_value(
                value.clone(),
                self.value_type,
                &checked.type_store,
            )?;
        }
        Ok(())
    }
}

impl PartialEq for MemoryWriteIntentValue {
    fn eq(&self, other: &Self) -> bool {
        self.ty == other.ty
            && self.key_type == other.key_type
            && self.value_type == other.value_type
            && self.encoded == other.encoded
    }
}
impl Eq for MemoryWriteIntentValue {}
impl std::fmt::Debug for MemoryWriteIntentValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemoryWriteIntentValue")
            .field("ty", &self.ty)
            .finish_non_exhaustive()
    }
}
