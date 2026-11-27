use super::*;
use crate::control::ExecutionFault;
use std::borrow::Cow;

#[cfg(test)]
mod tests;

impl<'a> EvalContext<'a> {
    pub fn execute_entry_signal(&mut self, item: HirItemId) -> ControlSignal {
        let start = self.step_id();
        self.events.push(WorkflowEvent::StepStarted(start));
        let signal = match self.checked.hir.items.get(item) {
            Some(HirItem::Flow(flow)) => self.execute_flow(item, flow, self.entry_args),
            Some(HirItem::Agent(agent)) => self.execute_agent_call(
                item,
                self.entry_args.to_vec(),
                agent.span,
                Default::default(),
            ),
            _ => ControlSignal::invalid_arguments(
                "interpreter entry point must be a checked flow or agent invocation",
                item_span(self.checked, item),
            ),
        };
        let end = self.step_id();
        self.events.push(WorkflowEvent::StepCompleted(end));
        signal
    }

    pub(super) fn execute_flow(
        &mut self,
        item: HirItemId,
        flow: &HirFlowDecl,
        args: &[InterpValue],
    ) -> ControlSignal {
        self.execute_flow_with_type_bindings(item, flow, args, Default::default())
    }

    pub(super) fn execute_flow_with_type_bindings(
        &mut self,
        item: HirItemId,
        flow: &HirFlowDecl,
        args: &[InterpValue],
        type_bindings: std::collections::HashMap<String, etas_types::TypeId>,
    ) -> ControlSignal {
        self.execute_flow_with_generic_bindings(
            item,
            flow,
            Cow::Borrowed(args),
            crate::control::GenericBindings {
                types: type_bindings,
                parameters: Default::default(),
                rows: Default::default(),
            },
        )
    }

    pub(super) fn execute_flow_with_generic_bindings(
        &mut self,
        item: HirItemId,
        flow: &HirFlowDecl,
        args: Cow<'_, [InterpValue]>,
        bindings: crate::control::GenericBindings,
    ) -> ControlSignal {
        self.execute_flow_with_receiver(item, flow, args, bindings, None)
    }

    pub(super) fn execute_flow_with_receiver(
        &mut self,
        item: HirItemId,
        flow: &HirFlowDecl,
        args: Cow<'_, [InterpValue]>,
        bindings: crate::control::GenericBindings,
        update: Option<crate::control::ReceiverOutcome>,
    ) -> ControlSignal {
        if let etas_hir::HirFlowBody::Runtime { role, span } = flow.body {
            return match role {
                etas_std::StdExecutionRole::ComparatorCapture
                | etas_std::StdExecutionRole::ComparatorSame => {
                    self.execute_comparator_identity(item, role, &args, span)
                }
                etas_std::StdExecutionRole::PayloadConvert => {
                    self.execute_payload_conversion(item, &args, &bindings, span)
                }
                etas_std::StdExecutionRole::MessageContextCapture
                | etas_std::StdExecutionRole::MessageContextRecord => {
                    self.execute_message_context(item, role, &args, span)
                }
                etas_std::StdExecutionRole::ToolReference => ControlSignal::missing_checked_fact(
                    "tool.reference requires its checked declaration operand at the call site",
                    span,
                ),
                etas_std::StdExecutionRole::ParseBody
                | etas_std::StdExecutionRole::AgentWithBody
                | etas_std::StdExecutionRole::CallableWithBody => {
                    self.execute_source_code_primitive(item, role, &args, &bindings, span)
                }
                etas_std::StdExecutionRole::TaskStart => {
                    self.execute_source_task_start(item, args.into_owned(), &bindings, span)
                }
                etas_std::StdExecutionRole::TaskNextCompletion
                | etas_std::StdExecutionRole::TaskCancel => {
                    self.execute_source_scheduler_control(item, role, &args, &bindings, span)
                }
                etas_std::StdExecutionRole::ScopeEnter => {
                    self.execute_runtime_scope(item, args.into_owned(), &bindings, None, span)
                }
                etas_std::StdExecutionRole::ResourceScope => {
                    self.execute_resource_scope(item, args.into_owned(), &bindings, None, span)
                }
                etas_std::StdExecutionRole::ReadScopeEnter => {
                    self.enter_stream_read_scope(item, &args, span)
                }
                etas_std::StdExecutionRole::Abort => match args.as_ref() {
                    [InterpValue::String(message)] => {
                        ControlSignal::execution_aborted(message.to_string(), span)
                    }
                    _ => ControlSignal::missing_checked_fact(
                        "abort execution role requires its checked string argument",
                        span,
                    ),
                },
                etas_std::StdExecutionRole::Checkpoint => {
                    self.execute_checkpoint_callable(args.into_owned(), span)
                }
                etas_std::StdExecutionRole::CachedSchemaFingerprint
                | etas_std::StdExecutionRole::RememberSchemaFingerprint => {
                    self.schema_fingerprint_cache(role, &args, span)
                }
                etas_std::StdExecutionRole::StorageContext
                | etas_std::StdExecutionRole::StorageNonce
                | etas_std::StdExecutionRole::StreamClock
                | etas_std::StdExecutionRole::CurrentSession => {
                    self.execute_runtime_observation(item, role, &args, span)
                }
                etas_std::StdExecutionRole::CaptureData => {
                    self.execute_data_capture(item, &args, &bindings, span)
                }
                etas_std::StdExecutionRole::CaptureStorage
                | etas_std::StdExecutionRole::RestoreStorage
                | etas_std::StdExecutionRole::StorageSchema => {
                    self.execute_storage_transfer(item, role, &args, &bindings, span)
                }
                etas_std::StdExecutionRole::Requeue | etas_std::StdExecutionRole::Park => {
                    self.execute_runtime_transfer(item, role, args.into_owned(), &bindings, span)
                }
            };
        }
        let active_receiver = matches!(args.first(), Some(InterpValue::Agent(reference))
            if self.task.agent_turn.as_ref().is_some_and(|turn| turn.reference == *reference));
        if !active_receiver
            && (flow.receiver_mode == etas_hir::HirReceiverMode::Updating) != update.is_some()
            || update.as_ref().is_some_and(|outcome| outcome.slot != 0)
        {
            return ControlSignal::missing_checked_fact(
                "updating method invocation requires its checked receiver access",
                flow.span,
            );
        }
        if bindings.is_empty()
            && self.execution_limits.max_steps.is_none()
            && let Some(body) = self.plan.prepared_program().arithmetic.get(&item)
            && self.plan.residual.scope(item).is_none()
            && let Some(value) = body.evaluate(&args)
        {
            let span = self.runtime.limit_span;
            return match self.runtime.safe_points.consume_work(
                self.runtime.execution_limits,
                || self.task.host_budget(),
                span,
                body.work(),
            ) {
                Ok(()) => ControlSignal::Value(value),
                Err(fault) => ControlSignal::Fault(Box::new(fault)),
            };
        }
        let mut frame = match self.callable_frame_with_generics(flow.scope, bindings, flow.span) {
            Ok(frame) => frame,
            Err(fault) => return ControlSignal::Fault(Box::new(fault)),
        };
        match args {
            Cow::Borrowed(values) => {
                for (&symbol, value) in flow.params.iter().zip(values) {
                    frame.insert(symbol, value.clone());
                }
            }
            Cow::Owned(values) => {
                for (&symbol, value) in flow.params.iter().zip(values) {
                    frame.insert(symbol, value);
                }
            }
        }
        let Some(body) = flow.body.as_block() else {
            return ControlSignal::missing_checked_fact(
                format!(
                    "flow body block is missing from checked HIR: item {item:?}, declaration {:?}, body {:?}, root {:?}",
                    self.checked.hir.declaration_path(flow.symbol),
                    flow.body,
                    self.plan.root
                ),
                flow.span,
            );
        };
        let signal = self.execute_item_body(item, body, &mut frame);
        let signal = self.drive_restored_handlers(signal, &mut frame);
        match update {
            Some(outcome) => {
                let Some(&symbol) = flow.params.get(outcome.slot as usize) else {
                    return ControlSignal::missing_checked_fact(
                        "updating flow has no receiver parameter",
                        flow.span,
                    );
                };
                self.continue_chain_signal(
                    signal,
                    Continuation::FinishReceiver {
                        outcome,
                        frame,
                        symbol,
                        span: flow.span,
                    },
                )
            }
            None => signal,
        }
    }

    pub(super) fn callable_frame_with_generics(
        &self,
        scope: etas_hir::ScopeId,
        bindings: crate::control::GenericBindings,
        span: Span,
    ) -> Result<Frame, ExecutionFault> {
        let layout = self.plan.frames.get(scope).ok_or_else(|| {
            ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                span,
                format!("callable frame layout is missing for scope {scope:?}"),
            )
        })?;
        Ok(Frame::with_generic_bindings(layout.clone(), bindings))
    }

    pub(crate) fn prepare_source_tool_call(
        &mut self,
        item: HirItemId,
        receiver: Option<InterpValue>,
        args: HostValue,
        span: Span,
    ) -> Result<
        (
            CallTarget,
            Vec<InterpValue>,
            Vec<u32>,
            Vec<etas_types::DefaultArgument>,
        ),
        ExecutionFault,
    > {
        let Some(HirItem::Tool(tool)) = self.checked.hir.items.get(item) else {
            return Err(ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                span,
                "source tool binding does not point at a HIR tool item",
            ));
        };
        let Some(etas_types::ItemSignature::Tool(signature)) =
            self.checked.types.item_signatures.get(&item)
        else {
            return Err(ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                span,
                "source tool is missing checked tool signature facts",
            ));
        };
        let HirToolBody::Source(_) = tool.body else {
            return Err(ExecutionFault::new(
                AnalysisDiagnosticCode::UnhandledRuntimeError,
                span,
                "bodyless external tool requires a configured host tool adapter",
            ));
        };
        let start = usize::from(receiver.is_some());
        let (values, mut slots, mut defaults) = self.decode_source_tool_args(
            &tool.params[start..],
            &signature.params[start..],
            args,
            span,
        )?;
        for slot in &mut slots {
            *slot += start as u32;
        }
        for default in &mut defaults {
            default.slot += start as u32;
        }
        if receiver.is_some() {
            slots.insert(0, 0);
        }
        Ok((
            CallTarget::ToolItem(item),
            receiver.into_iter().chain(values).collect(),
            slots,
            defaults,
        ))
    }

    pub(crate) fn execute_source_tool_call(
        &mut self,
        item: HirItemId,
        args: Vec<InterpValue>,
        span: Span,
    ) -> ControlSignal {
        self.execute_source_tool_call_with_generics(item, args, span, Default::default())
    }

    pub(super) fn execute_source_tool_call_with_generics(
        &mut self,
        item: HirItemId,
        args: Vec<InterpValue>,
        span: Span,
        bindings: crate::control::GenericBindings,
    ) -> ControlSignal {
        let Some(HirItem::Tool(tool)) = self.checked.hir.items.get(item) else {
            return ControlSignal::Fault(Box::new(ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                span,
                "source tool call target does not point at a HIR tool item",
            )));
        };
        let HirToolBody::Source(_) = tool.body else {
            return ControlSignal::Fault(Box::new(ExecutionFault::new(
                AnalysisDiagnosticCode::UnhandledRuntimeError,
                span,
                "bodyless external tool requires a configured host tool adapter",
            )));
        };
        if tool.params.len() != args.len() {
            return ControlSignal::Fault(Box::new(ExecutionFault::new(
                AnalysisDiagnosticCode::InvalidArguments,
                span,
                format!(
                    "source tool expects {} argument(s), got {}",
                    tool.params.len(),
                    args.len()
                ),
            )));
        }
        let mut frame = match self.callable_frame_with_generics(tool.scope, bindings, span) {
            Ok(frame) => frame,
            Err(fault) => return ControlSignal::Fault(Box::new(fault)),
        };
        for (symbol, arg) in tool.params.iter().zip(args) {
            frame.insert(*symbol, arg);
        }
        let Some(body) = self.item_primary_block(item) else {
            return ControlSignal::Fault(Box::new(ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                span,
                "source tool body block is missing from checked HIR structure facts",
            )));
        };
        let signal = self.execute_item_body(item, body, &mut frame);
        let signal = self.drive_restored_handlers(signal, &mut frame);
        match signal {
            ControlSignal::Return(value) => ControlSignal::Value(value),
            other => other,
        }
    }

    pub(in crate::eval) fn bind_source_tool_arguments(
        &mut self,
        item: HirItemId,
        values: Vec<InterpValue>,
        slots: Vec<u32>,
        defaults: Vec<etas_types::DefaultArgument>,
        span: Span,
    ) -> ControlSignal {
        if defaults.is_empty() {
            return match super::arguments::dense_arguments(values.len(), slots, values) {
                Ok(values) => self.execute_call_target(CallTarget::ToolItem(item), values, span),
                Err(reason) => ControlSignal::missing_checked_fact(reason, span),
            };
        }
        let etas_types::ParameterDefaultRef::Source { parameter } = defaults[0].source else {
            unreachable!("source tool bindings belong to their checked declaration");
        };
        let expr = self.checked.hir.parameter_defaults[&parameter];
        let span = self.checked.hir.exprs[expr].span(&self.checked.hir.blocks);
        let Some(HirItem::Tool(tool)) = self.checked.hir.items.get(item) else {
            unreachable!("checked source tool")
        };
        let parameter_count = tool.params.len();
        let frame = match self.callable_frame_with_generics(tool.scope, Default::default(), span) {
            Ok(frame) => frame,
            Err(fault) => return ControlSignal::Fault(Box::new(fault)),
        };
        let previous = self.task.model_policy.clone();
        match self.declaration_model_policy(item) {
            Ok(Some(policy)) => self.task.model_policy = policy,
            Ok(None) => (),
            Err(fault) => return ControlSignal::Fault(Box::new(fault)),
        }
        let signal = self.prepare_default_arguments(super::arguments::DefaultArguments {
            tool: Some(item),
            expr,
            parameter_count,
            slots,
            values,
            defaults,
            next_default: 0,
            destination: super::arguments::ArgumentDestination::Call {
                target: Some(CallTarget::ToolItem(item)),
            },
            span,
            frame,
        });
        compose_signal_continuation(
            signal,
            Continuation::RestoreModelPolicy {
                previous: Box::new(previous),
                inner: Continuation::BlockValue.into(),
            },
        )
    }

    fn decode_source_tool_args(
        &mut self,
        params: &[etas_hir::SymbolId],
        input_types: &[etas_types::TypeId],
        args: HostValue,
        span: Span,
    ) -> Result<(Vec<InterpValue>, Vec<u32>, Vec<etas_types::DefaultArgument>), ExecutionFault>
    {
        if params.len() != input_types.len() {
            return Err(ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                span,
                "source tool HIR parameters do not match checked tool signature facts",
            ));
        }
        let fields = match args {
            HostValue::Record(fields) => fields,
            HostValue::Json(etas_host::HostJsonValue::Object(fields)) => fields
                .into_iter()
                .map(|(name, value)| (name, HostValue::Json(value)))
                .collect(),
            HostValue::Unit if params.is_empty() => Vec::new(),
            other => {
                return Err(ExecutionFault::new(
                    AnalysisDiagnosticCode::InvalidArguments,
                    span,
                    format!("model tool call arguments must be a record, got {other:?}"),
                ));
            }
        };
        let interface = params
            .iter()
            .map(|parameter| etas_types::ParameterInterface {
                name: Some(
                    self.checked
                        .hir
                        .symbols
                        .get(*parameter)
                        .expect("checked tool parameter")
                        .name
                        .clone(),
                ),
                default: self
                    .checked
                    .hir
                    .parameter_defaults
                    .contains_key(parameter)
                    .then_some(etas_types::ParameterDefaultRef::Source {
                        parameter: *parameter,
                    }),
            })
            .collect::<Vec<_>>();
        let names = fields
            .iter()
            .map(|(name, _)| Some(name.clone()))
            .collect::<Vec<_>>();
        let binding = etas_types::ArgumentBinding::bind(&interface, &names).map_err(|message| {
            ExecutionFault::new(AnalysisDiagnosticCode::InvalidArguments, span, message)
        })?;
        let values = fields
            .into_iter()
            .zip(&binding.argument_slots)
            .map(|((name, value), slot)| {
                if self
                    .checked
                    .hir
                    .parameter_injections
                    .contains(&params[*slot as usize])
                {
                    return Err(ExecutionFault::new(
                        AnalysisDiagnosticCode::InvalidArguments,
                        span,
                        format!("injected tool parameter `{name}` is hidden from model arguments"),
                    ));
                }
                super::host_value::decode_checked_host_value(
                    value,
                    input_types[*slot as usize],
                    self.checked,
                )
                .map_err(|message| {
                    ExecutionFault::new(AnalysisDiagnosticCode::InvalidArguments, span, message)
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok((values, binding.argument_slots, binding.defaults))
    }
}
