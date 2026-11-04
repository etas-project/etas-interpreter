use super::host_value::interp_to_host_value;
use super::*;

impl<'a> EvalContext<'a> {
    pub(super) fn decode_memory_selection_limit(
        &self,
        value: &InterpValue,
        span: Span,
    ) -> Result<u32, crate::control::ExecutionFault> {
        let invalid = |reason: &str| {
            crate::control::ExecutionFault::new(
                AnalysisDiagnosticCode::InvalidArguments,
                span,
                reason,
            )
        };
        let ty = self.runtime_type(etas_std::RuntimeTypeRole::Limit, span)?;
        let (name, fields) = crate::value::enum_value::payload(value, ty, self.checked)
            .map_err(|reason| invalid(&reason))?;
        if !matches!(name, "Iterations" | "Tokens" | "ContextTokens" | "Attempts") {
            return Err(invalid("memory selection limit requires a count limit"));
        }
        let [InterpValue::Number(value)] = fields else {
            return Err(invalid(
                "memory selection limit requires a count limit constructor with one integer field",
            ));
        };
        value
            .as_u32()
            .ok_or_else(|| invalid("memory selection limit exceeds the maximum supported count"))
    }

    pub(super) fn resume_memory_clear_delete_keys(
        &mut self,
        region_stable_id: String,
        path: Vec<String>,
        remaining_keys: Vec<InterpValue>,
        next_index: usize,
        span: Span,
    ) -> ControlSignal {
        if next_index >= remaining_keys.len() {
            return ControlSignal::Value(InterpValue::Unit);
        }
        let key = match interp_to_host_value(&remaining_keys[next_index]) {
            Ok(key) => key,
            Err(error) => {
                return abort_memory_selection(
                    span,
                    format!("memory clear encountered a non-host-encodable key: {error}"),
                );
            }
        };
        let request_id = HostRequestId(self.next_host_request);
        self.next_host_request += 1;
        let store = StoreRef {
            region: MemoryRegionRef {
                stable_id: region_stable_id.clone(),
                schema_fingerprint: None,
            },
            path: path.clone(),
        };
        // Finish the fenced scan before mutating. Delete only captured versions,
        // never a concurrent replacement; clear is not an atomic transaction.
        let resource = super::boundary_memory::memory_resource_key(&store, &key);
        let Some(version) = self
            .recorded_memory_version(&resource)
            .and_then(|version| etas_host::MemoryVersion::parse(version).ok())
        else {
            return ControlSignal::runtime_fault(
                "memory clear is missing its captured key version",
                span,
            );
        };
        ControlSignal::pending_memory(PendingMemory {
            request: MemoryRequest {
                id: request_id,
                store,
                operation: MemoryOperation::Delete {
                    key,
                    condition: WriteCondition::Match(version),
                },
                authority: self.host_authority(),
                trace: self.host_trace(),
                budget: self.host_budget(),
            },
            decode: MemoryDecode::Unit,
            span,
            continuation: Continuation::MemoryClearDeleteNext {
                region_stable_id,
                path,
                remaining_keys,
                next_index: next_index + 1,
                span,
            },
        })
    }
}

fn abort_memory_selection(span: Span, message: impl Into<String>) -> ControlSignal {
    ControlSignal::invalid_arguments(message.into(), span)
}
