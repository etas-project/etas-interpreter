use super::host_value::interp_to_host_value;
use super::*;

impl<'a> EvalContext<'a> {
    pub(super) fn eval_memory_store_method(
        &mut self,
        eval: MemoryStoreMethodEval<'_>,
        frame: &mut Frame,
    ) -> ControlSignal {
        self.resume_memory_args(MemoryArgsResume::from_eval(eval), frame)
    }

    pub(super) fn finish_memory_store_method(&mut self, args: MemoryStoreArgs) -> ControlSignal {
        let MemoryStoreArgs {
            region_stable_id,
            path,
            key_type,
            value_type,
            method,
            evaluated_args,
            span,
        } = args;
        let request_id = HostRequestId(self.next_host_request);
        self.next_host_request += 1;
        let request = match method.as_str() {
            "get" => {
                let key = match host_argument(&evaluated_args, 0, "memory get key") {
                    Ok(key) => key,
                    Err(error) => return abort_memory_store(span, error),
                };
                MemoryRequest {
                    id: request_id,
                    store: StoreRef {
                        region: MemoryRegionRef {
                            stable_id: region_stable_id.clone(),
                            schema_fingerprint: None,
                        },
                        path: path.clone(),
                    },
                    operation: MemoryOperation::Get { key },
                    authority: self.host_authority(),
                    trace: self.host_trace(),
                    budget: self.host_budget(),
                }
            }
            "contains" => {
                let key = match host_argument(&evaluated_args, 0, "memory contains key") {
                    Ok(key) => key,
                    Err(error) => return abort_memory_store(span, error),
                };
                MemoryRequest {
                    id: request_id,
                    store: StoreRef {
                        region: MemoryRegionRef {
                            stable_id: region_stable_id.clone(),
                            schema_fingerprint: None,
                        },
                        path: path.clone(),
                    },
                    operation: MemoryOperation::Get { key },
                    authority: self.host_authority(),
                    trace: self.host_trace(),
                    budget: self.host_budget(),
                }
            }
            "select" => {
                let Some(predicate) = evaluated_args.first().cloned() else {
                    return abort_memory_store(
                        span,
                        "memory select requires one predicate argument",
                    );
                };
                return ControlSignal::Value(InterpValue::MemorySelection {
                    region_stable_id,
                    path,
                    key_type,
                    value_type,
                    kind: crate::value::MemorySelectionKind::Select,
                    predicate: Some(Box::new(predicate)),
                    limit: None,
                });
            }
            "query" => {
                let Some(predicate) = evaluated_args.first().cloned() else {
                    return abort_memory_store(
                        span,
                        "memory query requires one predicate argument",
                    );
                };
                return ControlSignal::Value(InterpValue::MemorySelection {
                    region_stable_id,
                    path,
                    key_type,
                    value_type,
                    kind: crate::value::MemorySelectionKind::Query,
                    predicate: Some(Box::new(predicate)),
                    limit: None,
                });
            }
            "related_to" => {
                let Some(predicate) = evaluated_args.first().cloned() else {
                    return abort_memory_store(
                        span,
                        "memory related_to requires one predicate argument",
                    );
                };
                return ControlSignal::Value(InterpValue::MemorySelection {
                    region_stable_id,
                    path,
                    key_type,
                    value_type,
                    kind: crate::value::MemorySelectionKind::RelatedTo,
                    predicate: Some(Box::new(predicate)),
                    limit: None,
                });
            }
            "scan" => {
                return ControlSignal::Value(InterpValue::MemorySelection {
                    region_stable_id,
                    path,
                    key_type,
                    value_type,
                    kind: crate::value::MemorySelectionKind::Scan,
                    predicate: None,
                    limit: None,
                });
            }
            "put" => {
                let key = match host_argument(&evaluated_args, 0, "memory put key") {
                    Ok(key) => key,
                    Err(error) => return abort_memory_store(span, error),
                };
                let value = match host_argument(&evaluated_args, 1, "memory put value") {
                    Ok(value) => value,
                    Err(error) => return abort_memory_store(span, error),
                };
                MemoryRequest {
                    id: request_id,
                    store: StoreRef {
                        region: MemoryRegionRef {
                            stable_id: region_stable_id.clone(),
                            schema_fingerprint: None,
                        },
                        path: path.clone(),
                    },
                    operation: MemoryOperation::Put {
                        key,
                        value,
                        expected: None,
                        mode: MemoryWriteMode::Put,
                    },
                    authority: self.host_authority(),
                    trace: self.host_trace(),
                    budget: self.host_budget(),
                }
            }
            "put_versioned" => {
                let key = match host_argument(&evaluated_args, 0, "memory put_versioned key") {
                    Ok(key) => key,
                    Err(error) => return abort_memory_store(span, error),
                };
                let value = match host_argument(&evaluated_args, 1, "memory put_versioned value") {
                    Ok(value) => value,
                    Err(error) => return abort_memory_store(span, error),
                };
                let Some(expected) = evaluated_args.get(2).and_then(|value| {
                    memory_version_from_interp(value, self.known_std_types.memory_version)
                }) else {
                    return abort_memory_store(
                        span,
                        "memory put_versioned expects a MemoryVersion token",
                    );
                };
                MemoryRequest {
                    id: request_id,
                    store: StoreRef {
                        region: MemoryRegionRef {
                            stable_id: region_stable_id.clone(),
                            schema_fingerprint: None,
                        },
                        path: path.clone(),
                    },
                    operation: MemoryOperation::Put {
                        key,
                        value,
                        expected: Some(expected),
                        mode: MemoryWriteMode::Put,
                    },
                    authority: self.host_authority(),
                    trace: self.host_trace(),
                    budget: self.host_budget(),
                }
            }
            "insert" | "update" | "upsert" => {
                let key = match host_argument(&evaluated_args, 0, "memory write key") {
                    Ok(key) => key,
                    Err(error) => return abort_memory_store(span, error),
                };
                let value = match host_argument(&evaluated_args, 1, "memory write value") {
                    Ok(value) => value,
                    Err(error) => return abort_memory_store(span, error),
                };
                MemoryRequest {
                    id: request_id,
                    store: StoreRef {
                        region: MemoryRegionRef {
                            stable_id: region_stable_id.clone(),
                            schema_fingerprint: None,
                        },
                        path: path.clone(),
                    },
                    operation: MemoryOperation::Put {
                        key,
                        value,
                        expected: None,
                        mode: memory_write_mode(&method),
                    },
                    authority: self.host_authority(),
                    trace: self.host_trace(),
                    budget: self.host_budget(),
                }
            }
            "keys" => MemoryRequest {
                id: request_id,
                store: StoreRef {
                    region: MemoryRegionRef {
                        stable_id: region_stable_id.clone(),
                        schema_fingerprint: None,
                    },
                    path: path.clone(),
                },
                operation: MemoryOperation::Scan {
                    cursor: None,
                    limit: None,
                },
                authority: self.host_authority(),
                trace: self.host_trace(),
                budget: self.host_budget(),
            },
            "clear" => {
                let clear_region_stable_id = region_stable_id.clone();
                let clear_path = path.clone();
                return ControlSignal::pending_memory(PendingMemory {
                    request: MemoryRequest {
                        id: request_id,
                        store: StoreRef {
                            region: MemoryRegionRef {
                                stable_id: region_stable_id,
                                schema_fingerprint: None,
                            },
                            path,
                        },
                        operation: MemoryOperation::Scan {
                            cursor: None,
                            limit: None,
                        },
                        authority: self.host_authority(),
                        trace: self.host_trace(),
                        budget: self.host_budget(),
                    },
                    decode: MemoryDecode::KeyList { key_type },
                    span,
                    continuation: Continuation::MemoryClearDeleteAll {
                        region_stable_id: clear_region_stable_id,
                        path: clear_path,
                        span,
                    },
                });
            }
            "delete" => {
                let key = match host_argument(&evaluated_args, 0, "memory delete key") {
                    Ok(key) => key,
                    Err(error) => return abort_memory_store(span, error),
                };
                MemoryRequest {
                    id: request_id,
                    store: StoreRef {
                        region: MemoryRegionRef {
                            stable_id: region_stable_id.clone(),
                            schema_fingerprint: None,
                        },
                        path: path.clone(),
                    },
                    operation: MemoryOperation::Delete {
                        key,
                        expected: None,
                    },
                    authority: self.host_authority(),
                    trace: self.host_trace(),
                    budget: self.host_budget(),
                }
            }
            "delete_versioned" => {
                let key = match host_argument(&evaluated_args, 0, "memory delete_versioned key") {
                    Ok(key) => key,
                    Err(error) => return abort_memory_store(span, error),
                };
                let Some(expected) = evaluated_args.get(1).and_then(|value| {
                    memory_version_from_interp(value, self.known_std_types.memory_version)
                }) else {
                    return abort_memory_store(
                        span,
                        "memory delete_versioned expects a MemoryVersion token",
                    );
                };
                MemoryRequest {
                    id: request_id,
                    store: StoreRef {
                        region: MemoryRegionRef {
                            stable_id: region_stable_id.clone(),
                            schema_fingerprint: None,
                        },
                        path: path.clone(),
                    },
                    operation: MemoryOperation::Delete {
                        key,
                        expected: Some(expected),
                    },
                    authority: self.host_authority(),
                    trace: self.host_trace(),
                    budget: self.host_budget(),
                }
            }
            _ => {
                return ControlSignal::invalid_arguments(
                    format!("unsupported memory store method `{method}`"),
                    span,
                );
            }
        };
        ControlSignal::pending_memory(PendingMemory {
            request,
            decode: match method.as_str() {
                "get" => MemoryDecode::OptionValue { value_type },
                "contains" => MemoryDecode::BoolContains,
                "keys" => MemoryDecode::KeyList { key_type },
                "select" | "query" | "related_to" | "scan" => unreachable!(),
                "put" | "put_versioned" | "insert" | "update" | "upsert" | "delete"
                | "delete_versioned" => MemoryDecode::Unit,
                _ => MemoryDecode::Unit,
            },
            span,
            continuation: Continuation::BlockValue,
        })
    }
}

fn memory_write_mode(method: &str) -> MemoryWriteMode {
    match method {
        "insert" => MemoryWriteMode::Insert,
        "update" => MemoryWriteMode::Update,
        "upsert" => MemoryWriteMode::Upsert,
        _ => MemoryWriteMode::Put,
    }
}

fn host_argument(
    arguments: &[InterpValue],
    index: usize,
    label: &str,
) -> Result<HostValue, String> {
    let value = arguments
        .get(index)
        .ok_or_else(|| format!("{label} argument is missing"))?;
    interp_to_host_value(value).map_err(|error| format!("{label} is not host-encodable: {error}"))
}

fn abort_memory_store(span: Span, message: impl Into<String>) -> ControlSignal {
    ControlSignal::invalid_arguments(message.into(), span)
}

fn memory_version_from_interp(
    value: &InterpValue,
    expected_type: Option<etas_types::TypeId>,
) -> Option<etas_host::MemoryVersion> {
    let InterpValue::Nominal { ty, value } = value else {
        return None;
    };
    if Some(*ty) != expected_type {
        return None;
    }
    let InterpValue::Record(fields) = value.as_ref() else {
        return None;
    };
    let token =
        fields
            .snapshot()
            .into_iter()
            .find_map(|(name, value)| match (name.as_str(), value) {
                ("opaque", InterpValue::String(token)) => Some(token),
                _ => None,
            })?;
    Some(etas_host::MemoryVersion { opaque: token })
}

pub(super) struct MemoryStoreMethodEval<'a> {
    pub region_stable_id: String,
    pub path: Vec<String>,
    pub key_type: etas_types::TypeId,
    pub value_type: etas_types::TypeId,
    pub method: &'a str,
    pub args: &'a [HirArg],
    pub span: Span,
}

pub(super) struct MemoryStoreArgs {
    pub region_stable_id: String,
    pub path: Vec<String>,
    pub key_type: etas_types::TypeId,
    pub value_type: etas_types::TypeId,
    pub method: String,
    pub evaluated_args: Vec<InterpValue>,
    pub span: Span,
}
