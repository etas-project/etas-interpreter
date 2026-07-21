use super::*;
use crate::control::ExecutionFault;

impl<'a> EvalContext<'a> {
    pub fn execute_entry_signal(&mut self, item: HirItemId) -> ControlSignal {
        let start = self.step_id();
        self.events.push(WorkflowEvent::StepStarted(start));
        let signal = match self.checked.hir.items.get(item) {
            Some(HirItem::Flow(flow)) => self.execute_flow(item, flow, self.entry_args),
            _ => ControlSignal::invalid_arguments(
                "interpreter entry point must be a flow item",
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
        let (signal, _) = self.execute_flow_with_frame(item, flow, args);
        signal
    }

    pub(super) fn execute_flow_with_frame(
        &mut self,
        item: HirItemId,
        flow: &HirFlowDecl,
        args: &[InterpValue],
    ) -> (ControlSignal, Frame) {
        let mut frame = Frame::new(self.plan.slots.clone());
        for (symbol, arg) in flow.params.iter().zip(args.iter().cloned()) {
            frame.insert(*symbol, arg);
        }
        let body = self
            .item_primary_block(item)
            .expect("flow body block should be indexed by HirTreeView");
        let signal = self.execute_block(body, &mut frame);
        let signal = self.drive_restored_handlers(signal, &mut frame);
        (signal, frame)
    }

    pub(crate) fn prepare_source_tool_call(
        &mut self,
        item: HirItemId,
        args: HostValue,
        span: Span,
    ) -> Result<(CallTarget, Vec<InterpValue>), ExecutionFault> {
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
        let values = self.decode_source_tool_args(tool, &signature.params, args, span)?;
        Ok((CallTarget::ToolItem(item), values))
    }

    pub(crate) fn execute_source_tool_call(
        &mut self,
        item: HirItemId,
        args: Vec<InterpValue>,
        span: Span,
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
        let mut frame = Frame::new(self.plan.slots.clone());
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
        let signal = self.execute_block(body, &mut frame);
        let signal = self.drive_restored_handlers(signal, &mut frame);
        match signal {
            ControlSignal::Return(value) => ControlSignal::Value(value),
            other => other,
        }
    }

    fn decode_source_tool_args(
        &mut self,
        tool: &etas_hir::HirToolDecl,
        input_types: &[etas_types::TypeId],
        args: HostValue,
        span: Span,
    ) -> Result<Vec<InterpValue>, ExecutionFault> {
        if tool.params.len() != input_types.len() {
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
            HostValue::Unit if tool.params.is_empty() => Vec::new(),
            other => {
                return Err(ExecutionFault::new(
                    AnalysisDiagnosticCode::InvalidArguments,
                    span,
                    format!("model tool call arguments must be a record, got {other:?}"),
                ));
            }
        };
        tool.params
            .iter()
            .zip(input_types.iter().copied())
            .map(|(param, ty)| {
                let Some(param_symbol) = self.checked.hir.symbols.get(*param) else {
                    return Err(ExecutionFault::new(
                        AnalysisDiagnosticCode::MissingCheckedFact,
                        span,
                        "source tool parameter is missing from checked HIR",
                    ));
                };
                let Some((_, value)) = fields.iter().find(|(name, _)| name == &param_symbol.name)
                else {
                    return Err(ExecutionFault::new(
                        AnalysisDiagnosticCode::InvalidArguments,
                        span,
                        format!(
                            "model tool call for source tool is missing argument `{}`",
                            param_symbol.name
                        ),
                    ));
                };
                super::host_value::host_to_typed_interp_value(
                    value.clone(),
                    ty,
                    &self.checked.type_store,
                )
                .map_err(|error| {
                    ExecutionFault::new(
                        AnalysisDiagnosticCode::InvalidArguments,
                        span,
                        format!(
                            "model tool call argument `{}` did not match the source tool parameter type: {error}",
                            param_symbol.name,
                        ),
                    )
                })
            })
            .collect()
    }
}
