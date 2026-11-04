use super::*;

mod content;
#[cfg(test)]
mod content_tests;
#[cfg(test)]
mod tests;

impl<'a> EvalContext<'a> {
    pub(in crate::eval) fn finish_prompt_value_method_arg(
        &mut self,
        mut messages: crate::value::PromptValue,
        method: &str,
        role: crate::value::PromptRole,
        allow_plain_system_content: bool,
        value: InterpValue,
        span: Span,
    ) -> ControlSignal {
        if method == "data"
            && let InterpValue::MemorySelection {
                region_stable_id,
                path,
                kind,
                predicate,
                limit,
                ..
            } = value
        {
            return self.prompt_data_memory_selection(PromptMemorySelectionRequest {
                messages,
                method: method.to_owned(),
                role,
                allow_plain_system_content,
                region_stable_id,
                path,
                kind,
                predicate: predicate.map(|value| *value),
                limit,
                span,
            });
        }
        let (text, trust) =
            match self.prompt_channel_content(method, value, span, allow_plain_system_content) {
                Ok(content) => content,
                Err(fault) => return ControlSignal::Fault(Box::new(fault)),
            };
        messages.push(crate::value::PromptMessage { role, text, trust });
        ControlSignal::Value(InterpValue::Prompt(messages))
    }

    pub(super) fn prompt_data_memory_selection(
        &mut self,
        request: PromptMemorySelectionRequest,
    ) -> ControlSignal {
        let PromptMemorySelectionRequest {
            messages,
            method,
            role,
            allow_plain_system_content,
            region_stable_id,
            path,
            kind,
            predicate,
            limit,
            span,
        } = request;
        let operation = match kind {
            crate::value::MemorySelectionKind::Scan => MemoryOperation::Scan {
                cursor: None,
                limit,
            },
            crate::value::MemorySelectionKind::Select
            | crate::value::MemorySelectionKind::Query => {
                let Some(predicate) = predicate else {
                    let message = "Prompt.data memory selection query requires a predicate";
                    return ControlSignal::invalid_arguments(message.to_owned(), span);
                };
                let predicate = match super::host_value::interp_to_host_value(&predicate) {
                    Ok(predicate) => predicate,
                    Err(error) => {
                        let message = format!(
                            "Prompt.data memory selection predicate must be host-encodable: {error}"
                        );
                        return ControlSignal::invalid_arguments(message, span);
                    }
                };
                MemoryOperation::Query {
                    query: etas_host::MemoryQuery {
                        predicate: Some(predicate),
                        order_by: Vec::new(),
                    },
                    limit,
                }
            }
            crate::value::MemorySelectionKind::RelatedTo => {
                let Some(predicate) = predicate else {
                    let message =
                        "Prompt.data related_to memory selection requires an embedding argument";
                    return ControlSignal::invalid_arguments(message.to_owned(), span);
                };
                let predicate = match super::host_value::interp_to_host_value(&predicate) {
                    Ok(predicate) => predicate,
                    Err(error) => {
                        let message = format!(
                            "Prompt.data related_to embedding must be host-encodable: {error}"
                        );
                        return ControlSignal::invalid_arguments(message, span);
                    }
                };
                let Some(embedding) = host_value_to_embedding(&predicate) else {
                    let message = "Prompt.data related_to requires a non-empty numeric embedding";
                    return ControlSignal::invalid_arguments(message.to_owned(), span);
                };
                MemoryOperation::VectorSearch {
                    embedding,
                    limit: limit.unwrap_or(100).max(1),
                    filter: None,
                }
            }
        };
        let request_id = HostRequestId(self.next_host_request);
        self.next_host_request += 1;
        let pending = PendingMemory {
            request: MemoryRequest {
                id: request_id,
                store: StoreRef {
                    region: MemoryRegionRef {
                        stable_id: region_stable_id,
                        schema_fingerprint: None,
                    },
                    path,
                },
                operation,
                authority: self.host_authority(),
                trace: self.host_trace(),
                budget: self.host_budget(),
            },
            decode: MemoryDecode::JsonEntries,
            span,
            continuation: Continuation::PromptValueMethodArg {
                messages,
                method,
                role,
                allow_plain_system_content,
                span,
            },
        };
        ControlSignal::pending_memory(pending)
    }

    pub(in crate::eval) fn prompt_channel_content(
        &self,
        method: &str,
        value: InterpValue,
        span: Span,
        allow_plain_system_content: bool,
    ) -> Result<(crate::value::StringValue, Option<etas_types::TrustWrapper>), ExecutionFault> {
        match value {
            value if method == "data" => {
                if prompt_data_contains_secret(&value) {
                    return Err(ExecutionFault::new(
                        AnalysisDiagnosticCode::InvalidArguments,
                        span,
                        "Prompt.data cannot encode secret values",
                    ));
                }
                let json =
                    super::host_value::interp_to_host_json_string(&value).map_err(|error| {
                        ExecutionFault::new(
                            AnalysisDiagnosticCode::InvalidArguments,
                            span,
                            format!(
                                "Prompt.data cannot encode {} values: {}",
                                prompt_data_kind(&value),
                                error.message
                            ),
                        )
                    })?;
                Ok((json.into(), None))
            }
            value => content::text(value, method, span, allow_plain_system_content),
        }
    }
}
