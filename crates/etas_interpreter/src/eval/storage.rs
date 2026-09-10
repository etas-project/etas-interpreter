use super::{EvalContext, InterpValue};
use etas_host::{
    HostError, HostErrorCode, HostRequestId, StorageLimits, StorageOperationKey,
    StorageOperationRef,
};
use std::cell::Ref;

impl EvalContext<'_> {
    pub(super) fn checked_storage_record<'a>(
        &self,
        value: &'a InterpValue,
        path: &[&str],
    ) -> Result<Ref<'a, Vec<(String, InterpValue)>>, HostError> {
        let expected = super::resolve_std_type(self.checked, path)
            .ok_or_else(|| invalid("missing checked storage type"))?;
        let InterpValue::Nominal { ty, value } = value else {
            return Err(invalid("storage value requires nominal identity"));
        };
        if *ty != expected {
            return Err(invalid("storage value has foreign nominal identity"));
        }
        let InterpValue::Record(fields) = value.as_ref() else {
            return Err(invalid("invalid storage record representation"));
        };
        Ok(fields.borrow())
    }

    pub(super) fn checked_operation_ref(
        &self,
        value: &InterpValue,
        expected: Option<&etas_types::TypeId>,
    ) -> Result<StorageOperationRef, HostError> {
        if !matches!(value, InterpValue::Nominal { ty, .. } if Some(ty) == expected) {
            return Err(invalid(
                "operation reference does not match checked call ABI",
            ));
        }
        let fields =
            self.checked_storage_record(value, &["std", "memory", "StorageOperationRef"])?;
        if fields.len() != 2 {
            return Err(invalid("invalid operation reference fields"));
        }
        let operation = StorageOperationRef {
            key: StorageOperationKey::parse(text(&fields, "key", &self.storage_limits)?)?,
            request_fingerprint: text(&fields, "fingerprint", &self.storage_limits)?.into(),
        };
        operation.validate()?;
        Ok(operation)
    }

    pub(crate) fn bind_storage_operation(
        &mut self,
        request: HostRequestId,
        operation: &StorageOperationRef,
    ) -> Result<(), HostError> {
        operation.validate()?;
        if self.storage_operations.contains_key(&request.0) {
            return Err(HostError::new(
                HostErrorCode::InvalidRequest,
                "storage request occurrence is already bound",
            ));
        }
        if self.storage_operations.len() >= self.storage_limits.max_receipts {
            return Err(HostError::new(
                HostErrorCode::BudgetExceeded,
                "run storage operation ledger is full",
            ));
        }
        self.storage_operations.insert(request.0, operation.clone());
        Ok(())
    }
}

pub(super) fn field<'a>(
    fields: &'a [(String, InterpValue)],
    name: &str,
) -> Result<&'a InterpValue, HostError> {
    let mut found = fields.iter().filter(|(key, _)| key == name);
    match (found.next(), found.next()) {
        (Some((_, value)), None) => Ok(value),
        _ => Err(invalid("missing or duplicate storage field")),
    }
}

pub(super) fn text<'a>(
    fields: &'a [(String, InterpValue)],
    name: &str,
    limits: &StorageLimits,
) -> Result<&'a str, HostError> {
    match field(fields, name)? {
        InterpValue::String(value) if value.len() <= limits.max_value_bytes => Ok(value),
        _ => Err(invalid("invalid bounded storage string")),
    }
}

pub(super) fn invalid(message: impl Into<String>) -> HostError {
    HostError::new(HostErrorCode::InvalidRequest, message)
}
