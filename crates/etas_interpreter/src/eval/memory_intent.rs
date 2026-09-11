use super::*;
use crate::intrinsic::dispatch::{CheckedStdIntrinsicCall, MemoryIntentCallable};
use crate::value::MemoryWriteIntentValue;
use etas_host::memory::MemoryWriteIntent;

impl EvalContext<'_> {
    pub(super) fn execute_memory_intent_callable(
        &mut self,
        kind: MemoryIntentCallable,
        checked_call: &CheckedStdIntrinsicCall,
        args: Vec<InterpValue>,
        span: Span,
    ) -> ControlSignal {
        if matches!(
            kind,
            MemoryIntentCallable::Commit | MemoryIntentCallable::Reconcile
        ) {
            return self.execute_memory_commit(kind, checked_call, args, span);
        }
        if kind == MemoryIntentCallable::OperationRef {
            let [InterpValue::MemoryWriteIntent(value)] = args.as_slice() else {
                return ControlSignal::invalid_arguments(
                    "operation_ref requires MemoryWriteIntent",
                    span,
                );
            };
            if let Err(error) = value.validate(self.checked, &self.storage_limits) {
                return ControlSignal::missing_checked_fact(error, span);
            }
            let operation = value.intent().operation_ref();
            let value = HostValue::Record(vec![
                (
                    "key".into(),
                    HostValue::String(operation.key.as_str().into()),
                ),
                (
                    "fingerprint".into(),
                    HostValue::String(operation.request_fingerprint.clone()),
                ),
            ]);
            return match super::host_value::host_to_checked_interp_value(
                value,
                checked_call.result_type,
                self.checked,
                &self.storage_limits.clone(),
            ) {
                Ok(value) => ControlSignal::Value(value),
                Err(error) => ControlSignal::missing_checked_fact(error, span),
            };
        }
        match self.prepare_memory_intent(kind, checked_call.result_type, &args) {
            Ok(value) => ControlSignal::Value(InterpValue::MemoryWriteIntent(Box::new(value))),
            Err(error) => self.storage_error_signal(error, span),
        }
    }

    fn prepare_memory_intent(
        &self,
        kind: MemoryIntentCallable,
        result_type: etas_types::TypeId,
        args: &[InterpValue],
    ) -> Result<MemoryWriteIntentValue, etas_host::HostError> {
        let Some((
            InterpValue::MemoryStore {
                region_stable_id,
                path,
                key_type,
                value_type,
            },
            args,
        )) = args.split_first()
        else {
            return Err(invalid("prepare requires a checked Store<K,V>"));
        };
        let store = StoreRef {
            region: MemoryRegionRef {
                stable_id: region_stable_id.clone(),
                schema_fingerprint: None,
            },
            path: path.clone(),
        };
        let limits = self.storage_limits.clone();
        let intent = match (kind, args) {
            (MemoryIntentCallable::PreparePut, [key, value, condition]) => {
                MemoryWriteIntent::prepare_put(
                    store,
                    super::host_value::interp_to_host_value(key).map_err(invalid)?,
                    super::host_value::interp_to_host_value(value).map_err(invalid)?,
                    self.intent_write_condition(condition).map_err(invalid)?,
                    &limits,
                )
            }
            (MemoryIntentCallable::PrepareDelete, [key, condition]) => {
                MemoryWriteIntent::prepare_delete(
                    store,
                    super::host_value::interp_to_host_value(key).map_err(invalid)?,
                    self.intent_write_condition(condition).map_err(invalid)?,
                    &limits,
                )
            }
            _ => return Err(invalid("invalid memory preparation argument count")),
        }?;
        let value = MemoryWriteIntentValue::new(
            result_type,
            *key_type,
            *value_type,
            intent,
            &self.storage_limits,
        )?;
        value
            .validate(self.checked, &self.storage_limits)
            .map_err(invalid)?;
        Ok(value)
    }

    fn intent_write_condition(&self, value: &InterpValue) -> Result<WriteCondition, String> {
        let InterpValue::Variant { name, fields } = value else {
            return Err("invalid WriteCondition".into());
        };
        match (name.as_str(), fields.as_slice()) {
            ("Any", []) => Ok(WriteCondition::Any),
            ("Missing", []) => Ok(WriteCondition::Missing),
            ("Exists", []) => Ok(WriteCondition::Exists),
            ("Match", [value]) => super::memory_store::memory_version_from_interp(
                value,
                self.known_std_types.memory_version,
            )
            .map(WriteCondition::Match)
            .ok_or_else(|| "invalid checked MemoryVersion".into()),
            _ => Err("invalid WriteCondition constructor".into()),
        }
    }

    pub(super) fn storage_error_signal(
        &mut self,
        error: etas_host::HostError,
        span: Span,
    ) -> ControlSignal {
        self.storage_error_with_continuation(error, span, Continuation::BlockValue)
    }

    pub(crate) fn storage_error_with_continuation(
        &mut self,
        error: etas_host::HostError,
        span: Span,
        continuation: Continuation,
    ) -> ControlSignal {
        let Some(ty) = self.known_std_types.storage_error else {
            return ControlSignal::missing_checked_fact("missing checked StorageError type", span);
        };
        let payload = HostValue::Record(vec![
            ("code".into(), HostValue::String(error.code.as_str().into())),
            ("message".into(), HostValue::String(error.message)),
        ]);
        let value = match super::host_value::host_to_typed_interp_value(
            payload,
            ty,
            &self.checked.type_store,
        ) {
            Ok(value) => value,
            Err(error) => return ControlSignal::missing_checked_fact(error, span),
        };
        self.propagate_perform_signal(PendingPerform {
            expr: None,
            action: ResolvedActionRef {
                effect: etas_hir::HirEffectRef {
                    path: etas_hir::unresolved_path_from_segments(&["Error"], span),
                    args: Vec::new(),
                    span,
                },
                action: "raise".into(),
                action_symbol: ResolveResult::Unresolved,
                span,
            },
            error_type: Some(ty),
            args: vec![value],
            span,
            continuation,
        })
    }
}

fn invalid(message: impl Into<String>) -> etas_host::HostError {
    etas_host::HostError::new(etas_host::HostErrorCode::InvalidRequest, message)
}
