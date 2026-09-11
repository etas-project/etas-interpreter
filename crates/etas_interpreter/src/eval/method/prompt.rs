use super::*;

impl<'a> EvalContext<'a> {
    pub(in crate::eval) fn eval_prompt_value_method(
        &mut self,
        messages: Vec<crate::value::PromptMessage>,
        method: &str,
        args: &[HirArg],
        span: Span,
        frame: &mut Frame,
    ) -> ControlSignal {
        let role = match method {
            "system" => crate::value::PromptRole::System,
            "user" => crate::value::PromptRole::User,
            "assistant" => crate::value::PromptRole::Assistant,
            "data" => crate::value::PromptRole::Data,
            other => {
                return unsupported_method(span, "Prompt value", other);
            }
        };
        let [arg] = args else {
            return ControlSignal::invalid_arguments(
                format!("Prompt.{method} expects exactly one argument"),
                span,
            );
        };
        let expr = match arg {
            HirArg::Positional(expr) | HirArg::Named { value: expr, .. } => *expr,
        };
        let allow_plain_system_content = method != "system"
            || matches!(
                self.checked.hir.exprs.get(expr),
                Some(HirExpr::Literal(HirLiteral::String { .. }))
            );
        match self.eval_expr(expr, frame) {
            ControlSignal::Value(value) => self.finish_prompt_value_method_arg(
                messages,
                method,
                role,
                allow_plain_system_content,
                value,
                span,
            ),
            ControlSignal::Apply(pending) => attach_prompt_value_method_arg_continuation(
                ControlSignal::Apply(pending),
                messages,
                method,
                role,
                allow_plain_system_content,
                span,
            ),
            ControlSignal::Checkpoint(pending) => ControlSignal::Checkpoint(pending),
            ControlSignal::Block(pending) => attach_prompt_value_method_arg_continuation(
                ControlSignal::Block(pending),
                messages,
                method,
                role,
                allow_plain_system_content,
                span,
            ),
            ControlSignal::Expr(pending) => attach_prompt_value_method_arg_continuation(
                ControlSignal::Expr(pending),
                messages,
                method,
                role,
                allow_plain_system_content,
                span,
            ),
            ControlSignal::Call(call) => attach_prompt_value_method_arg_continuation(
                ControlSignal::Call(call),
                messages,
                method,
                role,
                allow_plain_system_content,
                span,
            ),
            ControlSignal::Perform(perform) => attach_prompt_value_method_arg_continuation(
                ControlSignal::Perform(perform),
                messages,
                method,
                role,
                allow_plain_system_content,
                span,
            ),
            ControlSignal::Memory(memory) => attach_prompt_value_method_arg_continuation(
                ControlSignal::Memory(memory),
                messages,
                method,
                role,
                allow_plain_system_content,
                span,
            ),
            ControlSignal::Session(session) => attach_prompt_value_method_arg_continuation(
                ControlSignal::Session(session),
                messages,
                method,
                role,
                allow_plain_system_content,
                span,
            ),
            ControlSignal::Console(console) => attach_prompt_value_method_arg_continuation(
                ControlSignal::Console(console),
                messages,
                method,
                role,
                allow_plain_system_content,
                span,
            ),
            ControlSignal::Command(command) => attach_prompt_value_method_arg_continuation(
                ControlSignal::Command(command),
                messages,
                method,
                role,
                allow_plain_system_content,
                span,
            ),
            ControlSignal::Model(model) => attach_prompt_value_method_arg_continuation(
                ControlSignal::Model(model),
                messages,
                method,
                role,
                allow_plain_system_content,
                span,
            ),
            ControlSignal::Host(host) => attach_prompt_value_method_arg_continuation(
                ControlSignal::Host(host),
                messages,
                method,
                role,
                allow_plain_system_content,
                span,
            ),
            ControlSignal::Return(value) => ControlSignal::Return(value),
            ControlSignal::Resume(value) => ControlSignal::Resume(value),
            ControlSignal::Finish(value) => ControlSignal::Finish(value),
            ControlSignal::Break => ControlSignal::Break,
            ControlSignal::Fault(fault) => ControlSignal::Fault(fault),
            ControlSignal::Cancelled(cause) => ControlSignal::Cancelled(cause),
            ControlSignal::Continue => ControlSignal::Continue,
        }
    }

    pub(in crate::eval) fn finish_prompt_value_method_arg(
        &mut self,
        mut messages: Vec<crate::value::PromptMessage>,
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
    ) -> Result<(String, Option<etas_types::TrustWrapper>), ExecutionFault> {
        match value {
            value if method == "data" => {
                if prompt_data_contains_secret(&value) {
                    return Err(ExecutionFault::new(
                        AnalysisDiagnosticCode::InvalidArguments,
                        span,
                        "Prompt.data cannot encode secret values",
                    ));
                }
                let host_value =
                    super::host_value::interp_to_host_value(&value).map_err(|error| {
                        ExecutionFault::new(
                            AnalysisDiagnosticCode::InvalidArguments,
                            span,
                            format!(
                                "Prompt.data cannot encode {} values: {error}",
                                prompt_data_kind(&value),
                            ),
                        )
                    })?;
                let json = etas_host::host_value_to_json(&host_value).map_err(|error| {
                    ExecutionFault::new(
                        AnalysisDiagnosticCode::InvalidArguments,
                        span,
                        format!("Prompt.data could not encode host value: {}", error.message),
                    )
                })?;
                Ok((json.to_string(), None))
            }
            InterpValue::String(text) => {
                if method == "system" && !allow_plain_system_content {
                    return Err(ExecutionFault::new(
                        AnalysisDiagnosticCode::InvalidArguments,
                        span,
                        "Prompt.system requires Trusted[T] content or a checked static string literal",
                    ));
                }
                Ok((text, None))
            }
            InterpValue::Prompt(parts) => {
                if method == "system" && !allow_plain_system_content {
                    return Err(ExecutionFault::new(
                        AnalysisDiagnosticCode::InvalidArguments,
                        span,
                        "Prompt.system requires Trusted[T] content or a checked static string literal",
                    ));
                }
                Ok((
                    parts
                        .into_iter()
                        .map(|message| message.text)
                        .collect::<Vec<_>>()
                        .join("\n"),
                    None,
                ))
            }
            InterpValue::Trust { wrapper, value } => {
                if wrapper == etas_types::TrustWrapper::Secret {
                    return Err(ExecutionFault::new(
                        AnalysisDiagnosticCode::InvalidArguments,
                        span,
                        "Secret[T] values are not prompt-encodable by default",
                    ));
                }
                if method == "system" && wrapper != etas_types::TrustWrapper::Trusted {
                    return Err(ExecutionFault::new(
                        AnalysisDiagnosticCode::InvalidArguments,
                        span,
                        "Prompt.system requires Trusted[T] content or a checked static string literal",
                    ));
                }
                let (text, _) = self.prompt_channel_content(
                    method,
                    *value,
                    span,
                    wrapper == etas_types::TrustWrapper::Trusted || allow_plain_system_content,
                )?;
                Ok((text, Some(wrapper)))
            }
            InterpValue::Message(message) => self.prompt_channel_content(
                method,
                *message.payload,
                span,
                allow_plain_system_content,
            ),
            other => Err(ExecutionFault::new(
                AnalysisDiagnosticCode::InvalidArguments,
                span,
                format!("Prompt.{method} expects a string-compatible argument, got {other:?}"),
            )),
        }
    }
}
