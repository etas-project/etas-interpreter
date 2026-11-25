use super::{host_value, memory_page, storage, *};
use crate::control::handler_activation::SourceMemoryPlace;
use crate::value::{InterpValue as V, enum_value};
use etas_host::memory::{
    MemoryMutation, MemoryWriteIntent, MemoryWriteOperation, MemoryWriteRequest,
};
use etas_host::{HostError, MemoryCursor, MemoryQuery};
use etas_types::{TypeId, value_representation};

impl EvalContext<'_> {
    pub(super) fn project_source_memory_store(
        &self,
        ty: TypeId,
        stable_id: &str,
        field: &str,
    ) -> Result<V, String> {
        let descriptor = HostValue::Record(vec![
            ("region".into(), HostValue::String(stable_id.into())),
            (
                "path".into(),
                HostValue::List(vec![HostValue::String(field.into())]),
            ),
            ("schema_fingerprint".into(), memory_page::option_value(None)),
        ]);
        host_value::decode_checked_host_value(
            HostValue::Record(vec![
                ("description".into(), descriptor),
                ("key_format".into(), memory_page::option_value(None)),
                ("value_format".into(), memory_page::option_value(None)),
                ("encoded_schema".into(), memory_page::option_value(None)),
            ]),
            ty,
            self.checked,
        )
    }

    pub(crate) fn source_memory_action(
        &mut self,
        perform: &PendingPerform,
    ) -> Option<ControlSignal> {
        let ("Memory", action) = self.source_default_signature(perform)? else {
            return None;
        };
        let proof = perform
            .checked_action
            .as_ref()
            .expect("checked default action");
        let operation = match action {
            "read" => {
                let [store, request] = perform.args.as_slice() else {
                    return Some(ControlSignal::missing_checked_fact(
                        "Memory action payload",
                        perform.span,
                    ));
                };
                let place = self.source_memory_store(store, proof.parameters[0]);
                place
                    .and_then(|place| {
                        self.require_source_memory_place(proof, &place)?;
                        let request =
                            enum_value::payload(request, proof.parameters[1], self.checked)
                                .map_err(storage::invalid)?;
                        let operation = self.source_memory_read(request)?;
                        let decode = MemoryDecode::SourceRead {
                            result_type: proof.output,
                            selection: request.0 == "Selection",
                        };
                        Ok((place, operation, decode))
                    })
                    .map(|(store, operation, decode)| {
                        ControlSignal::pending_memory(PendingMemory {
                            request: MemoryRequest {
                                id: self.next_host_request_id(),
                                store,
                                operation,
                                authority: self.host_authority(),
                                trace: self.host_trace(),
                                budget: self.host_budget(),
                            },
                            decode,
                            span: perform.span,
                            continuation: perform.continuation.clone(),
                        })
                    })
            }
            "write" | "reconcile" => {
                let action = if action == "write" {
                    "commit"
                } else {
                    "reconcile"
                };
                return Some(match self.source_storage_request(perform, proof, action) {
                    Ok(request) => ControlSignal::pending_host(PendingHostBoundary {
                        request: HostBoundaryRequest::MemoryWrite(request),
                        decode: HostBoundaryDecode::Storage {
                            result_type: proof.output,
                        },
                        span: perform.span,
                        continuation: perform.continuation.clone(),
                    }),
                    Err(error) => self.storage_error_with_continuation(
                        error,
                        perform.span,
                        perform.continuation.clone(),
                    ),
                });
            }
            _ => return None,
        };
        Some(operation.unwrap_or_else(|error| {
            self.storage_error_with_continuation(error, perform.span, perform.continuation.clone())
        }))
    }

    pub(super) fn source_memory_wire(
        request: &MemoryRequest,
        result: MemoryResult,
        selection: bool,
    ) -> Result<HostValue, String> {
        let (name, value) = match (&request.operation, result) {
            (MemoryOperation::Get { .. }, MemoryResult::None) => {
                ("Entry", memory_page::option_value(None))
            }
            (MemoryOperation::Get { key }, MemoryResult::Value { value, version }) => {
                let entry = etas_host::MemoryEntry {
                    key: key.clone(),
                    value,
                    version,
                };
                (
                    "Entry",
                    memory_page::option_value(Some(memory_page::entry_value(entry))),
                )
            }
            (_, MemoryResult::Entries { entries, cursor }) if selection => {
                ("Selected", memory_page::page_value(entries, cursor))
            }
            (MemoryOperation::Scan { .. }, MemoryResult::Entries { entries, cursor }) => {
                ("Page", memory_page::page_value(entries, cursor))
            }
            _ => return Err("memory response does not match the checked source read".into()),
        };
        Ok(HostValue::Variant {
            name: name.into(),
            fields: vec![value],
        })
    }

    pub(super) fn source_memory_store(
        &self,
        value: &V,
        expected: TypeId,
    ) -> Result<StoreRef, HostError> {
        value_representation::source_memory_store_parameters(&self.checked.type_store, expected)
            .ok_or_else(|| storage::invalid("memory action requires a checked source Store"))?;
        let V::Nominal { ty, .. } = value else {
            return Err(storage::invalid("memory action requires a Store value"));
        };
        if *ty != expected {
            return Err(storage::invalid(
                "memory action Store does not match its checked ABI",
            ));
        }
        let descriptor = crate::value::conversation::protocol::field(value, "description")
            .map_err(|error| storage::invalid(error.message))?;
        let fields =
            self.checked_storage_record(descriptor, &["std", "memory", "StoreDescription"])?;
        let stable_id = self.storage_string(storage::field(fields, "region")?)?;
        let path = array(storage::field(fields, "path")?)?
            .iter()
            .map(|part| self.storage_string(part))
            .collect::<Result<Vec<_>, _>>()?;
        let schema = crate::value::conversation::protocol::field(value, "encoded_schema")
            .map_err(|error| storage::invalid(error.message))?;
        let schema_fingerprint = self
            .source_memory_option(schema)?
            .map(|value| self.storage_string(value))
            .transpose()?;
        Ok(StoreRef {
            region: MemoryRegionRef {
                stable_id,
                schema_fingerprint,
            },
            path,
        })
    }

    fn require_source_memory_place(
        &self,
        proof: &crate::control::CheckedActionInvocation,
        store: &StoreRef,
    ) -> Result<(), HostError> {
        let etas_effects::Effect::AppliedAction(instance) = &proof.effect else {
            return Ok(());
        };
        if SourceMemoryPlace::matches_selector(
            self.checked,
            &instance.args,
            &store.region.stable_id,
            &store.path,
        ) == Some(false)
        {
            return Err(HostError::new(
                etas_host::HostErrorCode::AuthorityDenied,
                "Memory action target differs from its checked resource selector",
            ));
        }
        Ok(())
    }

    pub(in crate::eval) fn source_memory_handler_target(
        &self,
        perform: &PendingPerform,
    ) -> Option<SourceMemoryPlace> {
        let proof = perform.checked_action.as_ref()?;
        let etas_effects::Effect::AppliedAction(instance) = &proof.effect else {
            return None;
        };
        if instance.action.tag != etas_effects::MEMORY_TAG
            || instance.args.as_slice() != [etas_types::EffectArgRef::Wildcard]
        {
            return None;
        }
        self.source_memory_store(perform.args.first()?, *proof.parameters.first()?)
            .ok()
            .map(SourceMemoryPlace::from_store)
    }

    fn source_memory_read(
        &self,
        (name, fields): (&str, &[V]),
    ) -> Result<MemoryOperation, HostError> {
        let default_limit = u32::try_from(self.storage_limits.max_page_entries).unwrap_or(u32::MAX);
        match (name, fields) {
            ("Get", [key]) => Ok(MemoryOperation::Get { key: project(key)? }),
            ("Page", [cursor, limit]) => Ok(MemoryOperation::Scan {
                cursor: self.source_memory_cursor(cursor)?,
                limit: Some(
                    self.source_memory_option(limit)?
                        .map(count)
                        .transpose()?
                        .unwrap_or(default_limit),
                ),
            }),
            ("Selection", [selection]) => {
                let ty =
                    super::resolve_std_type(self.checked, &["std", "memory", "SelectionRequest"])
                        .ok_or_else(|| storage::invalid("selection request type is missing"))?;
                match enum_value::payload(selection, ty, self.checked).map_err(storage::invalid)? {
                    ("Query", [language, version, payload, limit]) => Ok(MemoryOperation::Query {
                        query: MemoryQuery::Encoded {
                            language: self.storage_string(language)?,
                            version: count(version)?,
                            payload: match project(payload)? {
                                HostValue::Bytes(bytes) => bytes,
                                _ => {
                                    return Err(storage::invalid(
                                        "memory query payload must be bytes",
                                    ));
                                }
                            },
                        },
                        limit: self.source_memory_option(limit)?.map(count).transpose()?,
                    }),
                    _ => Err(storage::invalid("invalid selection operation")),
                }
            }
            _ => Err(storage::invalid("invalid read operation")),
        }
    }

    fn source_memory_mutation(
        &self,
        (name, fields): (&str, &[V]),
    ) -> Result<MemoryMutation, HostError> {
        let ty = super::resolve_std_type(self.checked, &["std", "memory", "WriteCondition"])
            .ok_or_else(|| storage::invalid("write condition type is missing"))?;
        let condition = |value| {
            self.intent_write_condition(value, ty)
                .map_err(storage::invalid)
        };
        match (name, fields) {
            ("Put", [key, value, expected]) => Ok(MemoryMutation::Put {
                key: project(key)?,
                value: project(value)?,
                condition: condition(expected)?,
            }),
            ("Delete", [key, expected]) => Ok(MemoryMutation::Delete {
                key: project(key)?,
                condition: condition(expected)?,
            }),
            _ => Err(storage::invalid("invalid write operation")),
        }
    }

    fn source_storage_request(
        &mut self,
        perform: &PendingPerform,
        proof: &crate::control::CheckedActionInvocation,
        action: &str,
    ) -> Result<MemoryWriteRequest, HostError> {
        match (action, perform.args.as_slice()) {
            ("commit", [store, reference, request, V::String(wire)]) => {
                let store = self.source_memory_store(store, proof.parameters[0])?;
                self.require_source_memory_place(proof, &store)?;
                let reference = self.checked_operation_ref(reference, proof.parameters.get(1))?;
                let request = enum_value::payload(request, proof.parameters[2], self.checked)
                    .map_err(storage::invalid)?;
                let mutation = self.source_memory_mutation(request)?;
                let intent = MemoryWriteIntent::decode(wire.as_str(), &self.storage_limits)?;
                if intent.store() != &store
                    || intent.mutation() != &mutation
                    || intent.operation_ref() != &reference
                {
                    return Err(storage::invalid(
                        "checked memory mutation differs from its prepared intent",
                    ));
                }
                intent.into_request(
                    self.next_host_request_id(),
                    self.host_authority(),
                    self.host_trace(),
                    self.host_budget(),
                    &self.storage_limits,
                )
            }
            ("reconcile", [store, reference]) => {
                let store = self.source_memory_store(store, proof.parameters[0])?;
                self.require_source_memory_place(proof, &store)?;
                let operation = self.checked_operation_ref(reference, proof.parameters.get(1))?;
                Ok(MemoryWriteRequest {
                    id: self.next_host_request_id(),
                    store,
                    operation: MemoryWriteOperation::Reconcile { operation },
                    authority: self.host_authority(),
                    trace: self.host_trace(),
                    budget: self.host_budget(),
                })
            }
            _ => Err(storage::invalid(
                "invalid checked source storage action arguments",
            )),
        }
    }

    fn source_memory_cursor(&self, value: &V) -> Result<Option<MemoryCursor>, HostError> {
        self.source_memory_option(value)?
            .map(|value| {
                let fields =
                    self.checked_storage_record(value, &["std", "memory", "MemoryCursor"])?;
                let opaque = self.storage_string(storage::field(fields, "opaque")?)?;
                Ok(MemoryCursor { opaque })
            })
            .transpose()
    }

    fn source_memory_option<'a>(&self, value: &'a V) -> Result<Option<&'a V>, HostError> {
        let V::Nominal { ty, .. } = value else {
            return Err(storage::invalid(
                "memory action optional value lacks its checked owner",
            ));
        };
        self.checked_option_payload(value, *ty)
            .ok_or_else(|| storage::invalid("memory action requires a checked Option value"))
    }

    fn storage_string(&self, value: &V) -> Result<String, HostError> {
        match value {
            V::String(value) if value.len() <= self.storage_limits.max_value_bytes => {
                Ok(value.to_string())
            }
            _ => Err(storage::invalid("invalid bounded storage string")),
        }
    }
}

fn array(value: &V) -> Result<&[V], HostError> {
    match value {
        V::Array(items) => Ok(items.borrow()),
        _ => Err(storage::invalid("invalid memory action array")),
    }
}

fn count(value: &V) -> Result<u32, HostError> {
    value
        .as_number()
        .and_then(|number| number.as_u32())
        .ok_or_else(|| storage::invalid("memory action row count must be u32"))
}

fn project(value: &V) -> Result<HostValue, HostError> {
    host_value::interp_to_host_value(value).map_err(storage::invalid)
}
