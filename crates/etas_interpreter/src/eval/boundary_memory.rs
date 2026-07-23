use super::host_value::{
    host_to_typed_interp_value, host_value_to_json_interp_value, interp_to_host_value,
};
use super::*;
use crate::control::ExecutionFault;
use etas_host::{MemoryEntry, MemoryVersion, host_value_to_json};

impl<'a> EvalContext<'a> {
    pub(crate) fn replayed_memory_result(&self, memory: &PendingMemory) -> Option<InterpValue> {
        if !matches!(
            (&memory.request.operation, memory.decode),
            (MemoryOperation::Get { .. }, _)
                | (MemoryOperation::Scan { .. }, MemoryDecode::KeyList { .. })
                | (MemoryOperation::Scan { .. }, MemoryDecode::JsonEntries)
                | (MemoryOperation::Query { .. }, MemoryDecode::JsonEntries)
        ) {
            return None;
        }
        let key = self.memory_boundary_key(memory);
        self.completed_host_boundary_result("memory", &key)
    }

    pub(crate) fn memory_replay_resources(
        &self,
        memory: &PendingMemory,
        replayed: &InterpValue,
    ) -> Result<Vec<String>, ExecutionFault> {
        match &memory.request.operation {
            MemoryOperation::Get { key } => {
                Ok(vec![memory_resource_key(&memory.request.store, key)])
            }
            MemoryOperation::Scan { .. }
                if matches!(
                    memory.decode,
                    MemoryDecode::KeyList { .. } | MemoryDecode::JsonEntries
                ) =>
            {
                match memory.decode {
                    MemoryDecode::KeyList { .. } => {
                        let InterpValue::List(keys) = replayed else {
                            return Err(ExecutionFault::new(
                                AnalysisDiagnosticCode::UnhandledRuntimeError,
                                memory.span,
                                "checkpoint memory scan replay result is not a key list",
                            ));
                        };
                        let mut resources = Vec::with_capacity(keys.borrow().len());
                        for key in keys.borrow().iter() {
                            let host_key = interp_to_host_value(key).map_err(|error| {
                                ExecutionFault::new(
                                    AnalysisDiagnosticCode::UnhandledRuntimeError,
                                    memory.span,
                                    format!(
                                        "checkpoint memory scan replay key is not host-encodable: {error}"
                                    ),
                                )
                            })?;
                            resources.push(memory_resource_key(&memory.request.store, &host_key));
                        }
                        Ok(resources)
                    }
                    MemoryDecode::JsonEntries => {
                        memory_json_entry_resources(&memory.request.store, replayed, memory.span)
                    }
                    _ => unreachable!(),
                }
            }
            MemoryOperation::Query { .. } if matches!(memory.decode, MemoryDecode::JsonEntries) => {
                memory_json_entry_resources(&memory.request.store, replayed, memory.span)
            }
            _ => Ok(Vec::new()),
        }
    }

    pub(crate) fn memory_resource_for_key(
        &self,
        memory: &PendingMemory,
        key: &HostValue,
    ) -> String {
        memory_resource_key(&memory.request.store, key)
    }

    pub(crate) fn recorded_memory_version(&self, resource: &str) -> Option<&str> {
        self.resource_versions
            .iter()
            .find(|version| version.resource == resource)
            .map(|version| version.version.as_str())
    }

    pub(crate) fn record_memory_result_versions(
        &mut self,
        request: &MemoryRequest,
        result: &MemoryResult,
    ) {
        match (&request.operation, result) {
            (MemoryOperation::Get { key }, MemoryResult::Value { version, .. })
            | (MemoryOperation::Put { key, .. }, MemoryResult::Written { version })
            | (MemoryOperation::Delete { key, .. }, MemoryResult::Deleted { version }) => {
                self.record_resource_version(memory_resource_key(&request.store, key), version);
            }
            (_, MemoryResult::Entries { entries, .. }) => {
                for MemoryEntry { key, version, .. } in entries {
                    self.record_resource_version(memory_resource_key(&request.store, key), version);
                }
            }
            _ => {}
        }
    }

    pub(crate) fn record_resource_version(&mut self, resource: String, version: &MemoryVersion) {
        if let Some(record) = self
            .resource_versions
            .iter_mut()
            .find(|record| record.resource == resource)
        {
            record.version = version.opaque.clone();
            return;
        }
        self.resource_versions.push(ResourceVersionRecord {
            resource,
            version: version.opaque.clone(),
        });
        self.resource_versions
            .sort_by(|left, right| left.resource.cmp(&right.resource));
    }

    pub(crate) fn memory_result_value(
        &self,
        memory: &PendingMemory,
        result: MemoryResult,
    ) -> Result<InterpValue, ExecutionFault> {
        match (memory.decode, result) {
            (MemoryDecode::OptionValue { .. }, MemoryResult::None) => Ok(InterpValue::OptionNone),
            (MemoryDecode::OptionValue { value_type }, MemoryResult::Value { value, .. }) => {
                host_to_typed_interp_value(value, value_type, &self.checked.type_store)
                    .map(|value| InterpValue::OptionSome(Box::new(value)))
                    .map_err(|error| {
                        ExecutionFault::new(
                            AnalysisDiagnosticCode::UnhandledRuntimeError,
                            memory.span,
                            format!(
                                "memory host value does not match the checked value type: {error}"
                            ),
                        )
                    })
            }
            (MemoryDecode::BoolContains, MemoryResult::None) => Ok(InterpValue::Bool(false)),
            (MemoryDecode::BoolContains, MemoryResult::Value { .. }) => Ok(InterpValue::Bool(true)),
            (MemoryDecode::KeyList { key_type }, MemoryResult::Entries { entries, .. }) => entries
                .into_iter()
                .map(|entry| {
                    host_to_typed_interp_value(entry.key, key_type, &self.checked.type_store)
                })
                .collect::<Result<Vec<_>, _>>()
                .map(|values| InterpValue::List(values.into()))
                .map_err(|error| {
                    ExecutionFault::new(
                        AnalysisDiagnosticCode::UnhandledRuntimeError,
                        memory.span,
                        format!("memory host key does not match the checked key type: {error}"),
                    )
                }),
            (MemoryDecode::JsonEntries, MemoryResult::Entries { entries, .. }) => {
                memory_entries_json_value(entries).ok_or_else(|| {
                    ExecutionFault::new(
                        AnalysisDiagnosticCode::UnhandledRuntimeError,
                        memory.span,
                        "memory host entries cannot be encoded as checked JSON values",
                    )
                })
            }
            (MemoryDecode::Unit, MemoryResult::Written { .. })
            | (MemoryDecode::Unit, MemoryResult::Deleted { .. }) => Ok(InterpValue::Unit),
            (_, MemoryResult::Conflict(conflict)) => Err(ExecutionFault::new(
                AnalysisDiagnosticCode::UnhandledRuntimeError,
                memory.span,
                format!(
                    "memory conflict must be routed through the checked Error[MemoryConflict] boundary: expected {:?}, actual {:?}",
                    conflict.expected, conflict.actual
                ),
            )),
            (_, other) => Err(ExecutionFault::new(
                AnalysisDiagnosticCode::UnhandledRuntimeError,
                memory.span,
                format!(
                    "memory host returned a result shape that does not match the checked memory operation: {:?}",
                    other
                ),
            )),
        }
    }

    pub(crate) fn memory_boundary_key(&self, memory: &PendingMemory) -> String {
        let op = match &memory.request.operation {
            MemoryOperation::Get { key } => format!("get:{key:?}"),
            MemoryOperation::Put {
                key,
                value,
                expected,
                mode,
            } => format!("put:{key:?}:{value:?}:expected={expected:?}:mode={mode:?}"),
            MemoryOperation::Delete { key, expected } => {
                format!("delete:{key:?}:expected={expected:?}")
            }
            other => format!("{other:?}"),
        };
        format!(
            "memory:{}:{}:{}",
            memory.request.store.region.stable_id,
            memory.request.store.path.join("."),
            op
        )
    }

    pub(crate) fn memory_conflict_signal(
        &mut self,
        memory: PendingMemory,
        conflict: etas_host::MemoryConflict,
    ) -> ControlSignal {
        let error_type = match self.standard_memory_conflict_type(memory.span) {
            Ok(error_type) => error_type,
            Err(fault) => return ControlSignal::Fault(Box::new(fault)),
        };
        let perform = PendingPerform {
            expr: None,
            action: memory_error_raise_action(memory.span),
            error_type: Some(error_type),
            args: vec![match memory_conflict_value(
                conflict,
                error_type,
                self.known_std_types.memory_version,
            ) {
                Ok(value) => value,
                Err(message) => {
                    return ControlSignal::runtime_fault(message, memory.span);
                }
            }],
            span: memory.span,
            continuation: memory.continuation,
        };
        self.propagate_perform_signal(perform)
    }

    fn standard_memory_conflict_type(
        &self,
        span: Span,
    ) -> Result<etas_types::TypeId, ExecutionFault> {
        self.known_std_types.memory_conflict.ok_or_else(|| {
            ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                span,
                "memory conflict handling requires checked std.memory MemoryConflict type",
            )
        })
    }
}

fn memory_resource_key(store: &StoreRef, key: &HostValue) -> String {
    format!(
        "memory:{}:{}:{}",
        store.region.stable_id,
        store.path.join("."),
        stable_host_key(key)
    )
}

fn memory_resource_key_from_json_key(
    store: &StoreRef,
    key: &crate::value::HostJsonSupportValue,
) -> String {
    format!(
        "memory:{}:{}:{}",
        store.region.stable_id,
        store.path.join("."),
        stable_json_key(key)
    )
}

fn memory_json_entry_resources(
    store: &StoreRef,
    replayed: &InterpValue,
    span: Span,
) -> Result<Vec<String>, ExecutionFault> {
    let InterpValue::Json(crate::value::HostJsonSupportValue::Array(entries)) = replayed else {
        return Err(ExecutionFault::new(
            AnalysisDiagnosticCode::UnhandledRuntimeError,
            span,
            "checkpoint memory replay result is not a JSON entry array",
        ));
    };
    let mut resources = Vec::with_capacity(entries.len());
    for entry in entries {
        let crate::value::HostJsonSupportValue::Object(fields) = entry else {
            return Err(ExecutionFault::new(
                AnalysisDiagnosticCode::UnhandledRuntimeError,
                span,
                "checkpoint memory replay entry is not a JSON object",
            ));
        };
        let Some((_, key)) = fields.iter().find(|(name, _)| name == "key") else {
            return Err(ExecutionFault::new(
                AnalysisDiagnosticCode::UnhandledRuntimeError,
                span,
                "checkpoint memory replay entry is missing its key",
            ));
        };
        resources.push(memory_resource_key_from_json_key(store, key));
    }
    Ok(resources)
}

fn stable_host_key(value: &HostValue) -> String {
    host_value_to_json(value)
        .map(|json| json.to_string())
        .unwrap_or_else(|_| format!("{value:?}"))
}

fn stable_json_key(value: &crate::value::HostJsonSupportValue) -> String {
    match value {
        crate::value::HostJsonSupportValue::Null => "null".to_owned(),
        crate::value::HostJsonSupportValue::Bool(value) => value.to_string(),
        crate::value::HostJsonSupportValue::NumberBits(value) => f64::from_bits(*value).to_string(),
        crate::value::HostJsonSupportValue::String(value) => {
            serde_json::Value::String(value.clone()).to_string()
        }
        crate::value::HostJsonSupportValue::Array(values) => {
            let values = values
                .iter()
                .map(|value| serde_json::from_str::<serde_json::Value>(&stable_json_key(value)))
                .collect::<Result<Vec<_>, _>>();
            values
                .map(serde_json::Value::Array)
                .map(|value| value.to_string())
                .unwrap_or_else(|_| format!("{value:?}"))
        }
        crate::value::HostJsonSupportValue::Object(entries) => {
            let entries = entries
                .iter()
                .map(|(name, value)| {
                    serde_json::from_str::<serde_json::Value>(&stable_json_key(value))
                        .map(|value| (name.clone(), value))
                })
                .collect::<Result<serde_json::Map<_, _>, _>>();
            entries
                .map(serde_json::Value::Object)
                .map(|value| value.to_string())
                .unwrap_or_else(|_| format!("{value:?}"))
        }
    }
}

fn memory_error_raise_action(span: Span) -> ResolvedActionRef {
    ResolvedActionRef {
        effect: etas_hir::HirEffectRef {
            path: etas_hir::unresolved_path_from_segments(&["Error"], span),
            args: Vec::new(),
            span,
        },
        action: "raise".to_owned(),
        action_symbol: ResolveResult::Unresolved,
        span,
    }
}

fn memory_conflict_value(
    conflict: etas_host::MemoryConflict,
    conflict_type: etas_types::TypeId,
    version_type: Option<etas_types::TypeId>,
) -> Result<InterpValue, String> {
    let expected = memory_version_option(conflict.expected, version_type)?;
    let actual = memory_version_option(conflict.actual, version_type)?;
    Ok(InterpValue::Nominal {
        ty: conflict_type,
        value: Box::new(InterpValue::Record(
        vec![
            ("expected".to_owned(), expected),
            ("actual".to_owned(), actual),
            (
                "current_value".to_owned(),
                match conflict.current_value {
                    Some(value) => InterpValue::OptionSome(Box::new(
                        host_value_to_json_interp_value(value).map_err(|error| {
                            format!(
                                "memory conflict current value cannot be represented as std.json.JsonValue: {}",
                                error.message
                            )
                        })?,
                    )),
                    None => InterpValue::OptionNone,
                },
            ),
        ]
        .into(),
        )),
    })
}

fn memory_entries_json_value(entries: Vec<etas_host::MemoryEntry>) -> Option<InterpValue> {
    let entries = entries
        .into_iter()
        .map(|entry| {
            HostValue::Record(vec![
                ("key".to_owned(), entry.key),
                ("value".to_owned(), entry.value),
                (
                    "version".to_owned(),
                    HostValue::String(entry.version.opaque),
                ),
            ])
        })
        .collect::<Vec<_>>();
    host_value_to_json_interp_value(HostValue::List(entries)).ok()
}

fn memory_version_option(
    version: Option<etas_host::MemoryVersion>,
    version_type: Option<etas_types::TypeId>,
) -> Result<InterpValue, String> {
    let Some(version) = version else {
        return Ok(InterpValue::OptionNone);
    };
    let version_type = version_type.ok_or_else(|| {
        "memory conflict contains a version but checked std.memory.MemoryVersion facts are missing"
            .to_owned()
    })?;
    Ok(InterpValue::OptionSome(Box::new(memory_version_value(
        version,
        version_type,
    ))))
}

fn memory_version_value(
    version: etas_host::MemoryVersion,
    version_type: etas_types::TypeId,
) -> InterpValue {
    InterpValue::Nominal {
        ty: version_type,
        value: Box::new(InterpValue::Record(
            vec![("opaque".to_owned(), InterpValue::String(version.opaque))].into(),
        )),
    }
}
