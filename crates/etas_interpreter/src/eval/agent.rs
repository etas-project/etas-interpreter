use super::*;
use crate::control::ExecutionFault;
use etas_host::{
    HostFieldSchema, HostSchema, HostVariantSchema, ModelToolChoice, ToolRef, ToolSchema,
};
use etas_std::{StdDecl, StdLimitKind};
use etas_types::{PrimitiveType, SymbolTypeFact, ToolSignature, Type, TypeId};
use serde_json::Value;

impl<'a> EvalContext<'a> {
    pub(super) fn execute_agent_call(
        &mut self,
        item: HirItemId,
        call_args: Vec<InterpValue>,
        span: Span,
    ) -> ControlSignal {
        let Some(HirItem::Agent(agent)) = self.checked.hir.items.get(item) else {
            return ControlSignal::fault(
                AnalysisDiagnosticCode::InvalidArguments,
                span,
                "callee HIR item is not an executable agent",
            );
        };
        if !matches!(agent.body, etas_hir::HirAgentBody::Source { .. }) {
            return ControlSignal::fault(
                AnalysisDiagnosticCode::MissingCheckedFact,
                span,
                "bodyless agent declarations are not executable without runtime metadata",
            );
        }
        if agent.params.len() != call_args.len() {
            return ControlSignal::fault(
                AnalysisDiagnosticCode::InvalidArguments,
                span,
                format!(
                    "agent expects {} argument(s), got {}",
                    agent.params.len(),
                    call_args.len()
                ),
            );
        }

        self.record_agent_message_handoffs(item, &call_args);

        let mut frame = Frame::new(self.plan.slots.clone());
        for (symbol, arg) in agent.params.iter().zip(call_args.into_iter()) {
            frame.insert(*symbol, arg);
        }
        let body = self
            .item_primary_block(item)
            .expect("agent body block should be indexed by HirTreeView");
        let signal = self.execute_block(body, &mut frame);
        self.finish_agent_prompt_body(item, signal, span)
    }

    fn record_agent_message_handoffs(&mut self, target: HirItemId, args: &[InterpValue]) {
        for arg in args {
            let InterpValue::Message(message) = arg else {
                continue;
            };
            self.events
                .push(crate::orchestration::WorkflowEvent::MessageHandoff {
                    id: message.id.clone(),
                    from: message.from.clone(),
                    to: message.to.clone(),
                    session: message.session.clone(),
                    target_item: target.0,
                });
        }
    }

    pub(crate) fn finish_agent_prompt_body(
        &mut self,
        item: HirItemId,
        signal: ControlSignal,
        span: Span,
    ) -> ControlSignal {
        let prompt = match signal {
            ControlSignal::Return(InterpValue::Prompt(prompt))
            | ControlSignal::Value(InterpValue::Prompt(prompt)) => prompt,
            ControlSignal::Return(other) | ControlSignal::Value(other) => {
                return ControlSignal::fault(
                    AnalysisDiagnosticCode::InvalidArguments,
                    span,
                    format!("agent body must evaluate to Prompt, got {other:?}"),
                );
            }
            signal @ (ControlSignal::Apply(_)
            | ControlSignal::Checkpoint(_)
            | ControlSignal::Block(_)
            | ControlSignal::Expr(_)
            | ControlSignal::Call(_)
            | ControlSignal::Perform(_)
            | ControlSignal::Memory(_)
            | ControlSignal::Session(_)
            | ControlSignal::Console(_)
            | ControlSignal::Command(_)
            | ControlSignal::Model(_)
            | ControlSignal::Host(_)) => {
                return compose_signal_continuation(
                    signal,
                    self.agent_prompt_body_continuation(item, span),
                );
            }
            ControlSignal::Resume(value) => return ControlSignal::Resume(value),
            ControlSignal::Finish(value) => return ControlSignal::Finish(value),
            ControlSignal::Break => return ControlSignal::Break,
            ControlSignal::Fault(fault) => return ControlSignal::Fault(fault),
            ControlSignal::Continue => return ControlSignal::Continue,
        };

        let Some(HirItem::Agent(agent)) = self.checked.hir.items.get(item) else {
            return ControlSignal::fault(
                AnalysisDiagnosticCode::UnhandledRuntimeError,
                span,
                "agent prompt continuation target is not an executable agent",
            );
        };
        let mut model_policy = self.model_policy.clone();
        let mut source_tools = Vec::new();
        let mut trace_plan = Vec::new();
        if let Err(fault) = self.apply_agent_annotations(
            item,
            &mut model_policy,
            &mut source_tools,
            &mut trace_plan,
        ) {
            return ControlSignal::Fault(Box::new(fault));
        }
        if !trace_plan.is_empty() {
            self.events
                .push(crate::orchestration::WorkflowEvent::AgentTracePlan {
                    item: item.0,
                    trace: trace_plan,
                });
        }

        let Some(output_ref) = agent.output_type else {
            return ControlSignal::fault(
                AnalysisDiagnosticCode::MissingCheckedFact,
                agent.span,
                "agent output type is missing from the declaration",
            );
        };
        let Some(output_type) = self.checked.types.type_refs.get(&output_ref).copied() else {
            return ControlSignal::fault(
                AnalysisDiagnosticCode::MissingCheckedFact,
                agent.span,
                "agent output type is missing checked type facts",
            );
        };
        let decode = self.agent_model_decode(output_type);
        let response_schema = match decode {
            ModelDecode::Typed(expected) => {
                let schema = match self.host_schema_for_type(expected, span, 0) {
                    Ok(schema) => schema,
                    Err(fault) => return ControlSignal::Fault(Box::new(fault)),
                };
                Some(schema)
            }
            ModelDecode::String | ModelDecode::ModelResponse => None,
        };
        if let Err(fault) =
            self.model_provider_supports_request(&model_policy, response_schema.is_some(), span)
        {
            return ControlSignal::Fault(Box::new(fault));
        }

        let request_id = HostRequestId(self.next_host_request);
        self.next_host_request += 1;
        ControlSignal::pending_model(PendingModel {
            request: ModelRequest {
                id: request_id,
                provider: model_policy.provider.clone(),
                model: model_policy.model.clone(),
                messages: prompt
                    .into_iter()
                    .map(|message| ModelMessage {
                        role: match message.role {
                            crate::value::PromptRole::System => ModelRole::System,
                            crate::value::PromptRole::User | crate::value::PromptRole::Data => {
                                ModelRole::User
                            }
                            crate::value::PromptRole::Assistant => ModelRole::Assistant,
                        },
                        content: vec![ModelContent::Text(message.text)],
                        tool_call_id: None,
                        tool_calls: Vec::new(),
                    })
                    .collect(),
                tools: model_policy.tools.clone(),
                tool_choice: model_policy.tool_choice.clone(),
                response_schema,
                policy_ref: model_policy.policy_ref.clone(),
                options: model_policy.options.clone(),
                authority: self.host_authority(),
                trace: self.host_trace(),
                budget: model_policy
                    .budget
                    .clone()
                    .map(|limits| self.host_budget().with_limits(limits))
                    .unwrap_or_else(|| self.host_budget()),
            },
            decode,
            max_tool_rounds: model_policy.max_tool_rounds,
            source_tools,
            span,
            continuation: Continuation::BlockValue,
        })
    }

    fn agent_prompt_body_continuation(&self, item: HirItemId, span: Span) -> Continuation {
        Continuation::AgentPromptBody {
            item,
            span,
            model_policy: Some(Box::new(self.model_policy.clone())),
        }
    }

    fn model_provider_supports_request(
        &self,
        policy: &crate::api::ModelExecutionPolicy,
        has_response_schema: bool,
        span: Span,
    ) -> Result<(), ExecutionFault> {
        let Some(capabilities) = policy.provider_capabilities else {
            if has_response_schema || !policy.tools.is_empty() {
                return Err(ExecutionFault::new(
                    AnalysisDiagnosticCode::InvalidArguments,
                    span,
                    "configured model provider did not report capabilities for typed output or tool-call execution",
                ));
            }
            return Ok(());
        };
        if has_response_schema
            && !capabilities.supports_forced_tool_output
            && !capabilities.supports_json_schema_response_format
            && !capabilities.supports_plain_json_text_instruction
        {
            return Err(ExecutionFault::new(
                AnalysisDiagnosticCode::InvalidArguments,
                span,
                "configured model provider does not support typed output requests",
            ));
        }
        if !policy.tools.is_empty() && !capabilities.supports_tool_call_loop {
            return Err(ExecutionFault::new(
                AnalysisDiagnosticCode::InvalidArguments,
                span,
                "configured model provider does not support model tool-call loops",
            ));
        }
        if !matches!(policy.tool_choice, ModelToolChoice::Auto)
            && !capabilities.supports_required_tool_choice
        {
            return Err(ExecutionFault::new(
                AnalysisDiagnosticCode::InvalidArguments,
                span,
                "configured model provider does not support required model tool choice",
            ));
        }
        Ok(())
    }

    fn apply_agent_annotations(
        &mut self,
        item: HirItemId,
        policy: &mut crate::api::ModelExecutionPolicy,
        source_tools: &mut Vec<SourceToolBinding>,
        trace_plan: &mut Vec<String>,
    ) -> Result<(), ExecutionFault> {
        let annotations = self
            .checked
            .hir
            .item_annotations
            .get(&item)
            .cloned()
            .unwrap_or_default();
        for annotation in annotations {
            match annotation_name(&annotation).as_str() {
                "model" => {
                    self.apply_agent_model_annotation(&annotation, policy)?;
                }
                "tools" => {
                    let (tools, bindings, tool_choice) =
                        self.agent_tools_annotation(&annotation)?;
                    policy.tools = tools;
                    policy.tool_choice = tool_choice;
                    *source_tools = bindings;
                }
                "limits" => {
                    self.apply_agent_limits_annotation(&annotation, policy)?;
                }
                "trace" => {
                    let entries = self.agent_trace_annotation(&annotation)?;
                    trace_plan.extend(entries);
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn apply_agent_model_annotation(
        &mut self,
        annotation: &etas_hir::HirAnnotation,
        policy: &mut crate::api::ModelExecutionPolicy,
    ) -> Result<(), ExecutionFault> {
        for arg in &annotation.args {
            let (name, value, span) = match arg {
                etas_hir::HirAnnotationArg::Positional { value, span } => ("model", *value, *span),
                etas_hir::HirAnnotationArg::Named {
                    name, value, span, ..
                } => (name.as_str(), *value, *span),
            };
            match name {
                "adapter" | "provider" => {
                    let Some(value) = self.const_string_expr(value) else {
                        return Err(ExecutionFault::new(
                            AnalysisDiagnosticCode::InvalidArguments,
                            span,
                            "`@model` provider/adapter must be a string literal",
                        ));
                    };
                    policy.provider = Some(etas_host::ModelProviderId(value));
                }
                "model" => {
                    let Some(value) = self.const_string_expr(value) else {
                        return Err(ExecutionFault::new(
                            AnalysisDiagnosticCode::InvalidArguments,
                            span,
                            "`@model` model must be a string literal",
                        ));
                    };
                    if !policy.model_locked {
                        policy.model = etas_host::ModelName(value);
                    }
                }
                other => {
                    return Err(ExecutionFault::new(
                        AnalysisDiagnosticCode::InvalidArguments,
                        span,
                        format!("unsupported `@model` argument `{other}`"),
                    ));
                }
            }
        }
        Ok(())
    }

    fn agent_tools_annotation(
        &mut self,
        annotation: &etas_hir::HirAnnotation,
    ) -> Result<(Vec<ToolSchema>, Vec<SourceToolBinding>, ModelToolChoice), ExecutionFault> {
        let mut tools = Vec::new();
        let mut bindings = Vec::new();
        let mut required_choice = false;
        for arg in &annotation.args {
            let value = match arg {
                etas_hir::HirAnnotationArg::Positional { value, .. } => *value,
                etas_hir::HirAnnotationArg::Named {
                    name, value, span, ..
                } if name == "choice" || name == "mode" => {
                    let Some(choice) = self.const_string_expr(*value) else {
                        return Err(ExecutionFault::new(
                            AnalysisDiagnosticCode::InvalidArguments,
                            *span,
                            "`@tools` choice must be a string literal",
                        ));
                    };
                    match choice.as_str() {
                        "auto" => required_choice = false,
                        "required" => required_choice = true,
                        other => {
                            return Err(ExecutionFault::new(
                                AnalysisDiagnosticCode::InvalidArguments,
                                *span,
                                format!(
                                    "unsupported `@tools` choice `{other}`; expected `auto` or `required`"
                                ),
                            ));
                        }
                    }
                    continue;
                }
                etas_hir::HirAnnotationArg::Named { name, value, .. }
                    if name == "tools" || name == "items" =>
                {
                    *value
                }
                etas_hir::HirAnnotationArg::Named { name, span, .. } => {
                    return Err(ExecutionFault::new(
                        AnalysisDiagnosticCode::InvalidArguments,
                        *span,
                        format!("unsupported `@tools` argument `{name}`"),
                    ));
                }
            };
            let (arg_tools, arg_bindings) = self.agent_tool_schemas(value, annotation.span)?;
            tools.extend(arg_tools);
            bindings.extend(arg_bindings);
        }
        let tool_choice = if required_choice {
            match tools.as_slice() {
                [] => {
                    return Err(ExecutionFault::new(
                        AnalysisDiagnosticCode::InvalidArguments,
                        annotation.span,
                        "`@tools` choice `required` needs at least one tool",
                    ));
                }
                [tool] => ModelToolChoice::RequiredTool(tool.tool.name.clone()),
                _ => ModelToolChoice::RequiredAny,
            }
        } else {
            ModelToolChoice::Auto
        };
        Ok((tools, bindings, tool_choice))
    }

    fn apply_agent_limits_annotation(
        &mut self,
        annotation: &etas_hir::HirAnnotation,
        policy: &mut crate::api::ModelExecutionPolicy,
    ) -> Result<(), ExecutionFault> {
        for arg in &annotation.args {
            let value = match arg {
                etas_hir::HirAnnotationArg::Positional { value, .. }
                | etas_hir::HirAnnotationArg::Named { value, .. } => *value,
            };
            let limits = match self.checked.hir.exprs.get(value) {
                Some(HirExpr::Array { elems, .. }) | Some(HirExpr::List { elems, .. }) => {
                    elems.clone()
                }
                _ => vec![value],
            };
            for limit in limits {
                let limit = self.resolve_limit_expr(limit)?;
                match limit.kind {
                    StdLimitKind::Attempts | StdLimitKind::Iterations => {
                        return Err(ExecutionFault::new(
                            AnalysisDiagnosticCode::InvalidArguments,
                            limit.span,
                            "`@limits` on agent only supports model budget limits; use Attempts/Iterations on retry, loop, or pipeline stages",
                        ));
                    }
                    StdLimitKind::Tokens
                    | StdLimitKind::ContextTokens
                    | StdLimitKind::WallTime
                    | StdLimitKind::Cost => {
                        self.apply_runtime_limit_to_model_policy(&limit, policy)?;
                    }
                }
            }
        }
        Ok(())
    }

    fn agent_trace_annotation(
        &mut self,
        annotation: &etas_hir::HirAnnotation,
    ) -> Result<Vec<String>, ExecutionFault> {
        let mut entries = Vec::new();
        for arg in &annotation.args {
            let value = match arg {
                etas_hir::HirAnnotationArg::Positional { value, .. }
                | etas_hir::HirAnnotationArg::Named { value, .. } => *value,
            };
            self.collect_agent_trace_entries(value, annotation.span, &mut entries)?;
        }
        Ok(entries)
    }

    fn collect_agent_trace_entries(
        &mut self,
        expr: HirExprId,
        fallback_span: Span,
        entries: &mut Vec<String>,
    ) -> Result<(), ExecutionFault> {
        match self.checked.hir.exprs.get(expr) {
            Some(HirExpr::Array { elems, .. }) | Some(HirExpr::List { elems, .. }) => {
                for elem in elems.clone() {
                    self.collect_agent_trace_entries(elem, fallback_span, entries)?;
                }
                Ok(())
            }
            _ => {
                entries.push(self.agent_trace_entry(expr, fallback_span)?);
                Ok(())
            }
        }
    }

    fn agent_trace_entry(
        &mut self,
        expr: HirExprId,
        fallback_span: Span,
    ) -> Result<String, ExecutionFault> {
        let Some(expr_data) = self.checked.hir.exprs.get(expr) else {
            return Err(ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                fallback_span,
                "`@trace` annotation expression is missing from checked HIR",
            ));
        };
        match expr_data {
            HirExpr::Literal(HirLiteral::String { value, .. }) => Ok(value.clone()),
            HirExpr::Literal(HirLiteral::Bool { value, .. }) => Ok(value.to_string()),
            HirExpr::Literal(HirLiteral::Int { text, .. })
            | HirExpr::Literal(HirLiteral::Float { text, .. }) => Ok(text.clone()),
            HirExpr::Literal(HirLiteral::Char { value, .. }) => Ok(value.to_string()),
            HirExpr::Path(path) => match path.resolution {
                ResolveResult::Resolved(_) => Ok(path
                    .segments
                    .iter()
                    .map(|segment| segment.name.as_str())
                    .collect::<Vec<_>>()
                    .join(".")),
                ResolveResult::PartiallyResolved(_)
                | ResolveResult::Unresolved
                | ResolveResult::Ambiguous(_) => Err(ExecutionFault::new(
                    AnalysisDiagnosticCode::MissingCheckedFact,
                    expr_data.span(&self.checked.hir.blocks),
                    "`@trace` path must resolve to a checked static symbol",
                )),
            },
            HirExpr::Tuple { elems, .. } => {
                let mut parts = Vec::with_capacity(elems.len());
                for elem in elems.clone() {
                    parts.push(self.agent_trace_entry(elem, fallback_span)?);
                }
                Ok(format!("({})", parts.join(", ")))
            }
            HirExpr::Array { elems, .. } | HirExpr::List { elems, .. } => {
                let mut parts = Vec::with_capacity(elems.len());
                for elem in elems.clone() {
                    parts.push(self.agent_trace_entry(elem, fallback_span)?);
                }
                Ok(format!("[{}]", parts.join(", ")))
            }
            HirExpr::Set { elems, .. } => {
                let mut parts = Vec::with_capacity(elems.len());
                for elem in elems.clone() {
                    parts.push(self.agent_trace_entry(elem, fallback_span)?);
                }
                Ok(format!("{{{}}}", parts.join(", ")))
            }
            HirExpr::Call {
                callee,
                generic_args,
                args,
                ..
            } if generic_args.is_empty() => {
                let name = self.agent_trace_constructor_name(
                    *callee,
                    expr_data.span(&self.checked.hir.blocks),
                )?;
                let mut parts = Vec::with_capacity(args.len());
                for arg in args.clone() {
                    match arg {
                        HirArg::Positional(value) => {
                            parts.push(self.agent_trace_entry(value, fallback_span)?);
                        }
                        HirArg::Named { name, value, .. } => {
                            parts.push(format!(
                                "{}={}",
                                name,
                                self.agent_trace_entry(value, fallback_span)?
                            ));
                        }
                    }
                }
                Ok(format!("{name}({})", parts.join(", ")))
            }
            HirExpr::Record(record) if record.path.is_none() && record.generic_args.is_empty() => {
                let mut fields = Vec::with_capacity(record.fields.len());
                for field in record.fields.clone() {
                    match field {
                        etas_hir::HirFieldInit::Named { name, value, .. } => {
                            fields.push(format!(
                                "{}={}",
                                name,
                                self.agent_trace_entry(value, fallback_span)?
                            ));
                        }
                        etas_hir::HirFieldInit::Shorthand { name, .. } => {
                            fields.push(name);
                        }
                    }
                }
                Ok(format!("{{{}}}", fields.join(", ")))
            }
            HirExpr::EmptyRecordOrMap { .. } => Ok("{}".to_owned()),
            HirExpr::EmptySequence { .. } => Ok("[]".to_owned()),
            _ => Err(ExecutionFault::new(
                AnalysisDiagnosticCode::InvalidArguments,
                expr_data.span(&self.checked.hir.blocks),
                "`@trace` annotation must contain static literals, paths, arrays, records, or compile-time constants",
            )),
        }
    }

    fn agent_trace_constructor_name(
        &self,
        callee: HirExprId,
        span: Span,
    ) -> Result<String, ExecutionFault> {
        let Some(HirExpr::Path(path)) = self.checked.hir.exprs.get(callee) else {
            return Err(ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                span,
                "`@trace` constructor callee is missing from checked HIR",
            ));
        };
        let ResolveResult::Resolved(symbol) = path.resolution else {
            return Err(ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                span,
                "`@trace` constructor callee must resolve to std.runtime.trace",
            ));
        };
        let Some(symbol) = self.checked.hir.symbols.get(symbol) else {
            return Err(ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                span,
                "`@trace` constructor symbol is missing from checked HIR",
            ));
        };
        let SymbolDef::ImportAlias { path, .. } = &symbol.def else {
            return Err(ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                span,
                "`@trace` constructor must be a std.runtime.trace import",
            ));
        };
        let Some(std_symbol) = self.checked.std_registry.lookup_qualified(path) else {
            return Err(ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                span,
                "`@trace` constructor is not registered in std",
            ));
        };
        if std_symbol.qualified_path.as_slice()
            != ["std", "runtime", "trace", std_symbol.name.as_str()]
        {
            return Err(ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                span,
                "`@trace` constructor must be in std.runtime.trace",
            ));
        }
        let StdDecl::Flow(flow) = &std_symbol.decl else {
            return Err(ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                span,
                "`@trace` constructor must be a pure std trace flow",
            ));
        };
        if !flow.public_effects.is_empty() || !flow.requested_actions.is_empty() {
            return Err(ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                span,
                "`@trace` constructor must be pure",
            ));
        }
        Ok(flow.name.clone())
    }

    fn agent_tool_schemas(
        &mut self,
        expr: HirExprId,
        span: Span,
    ) -> Result<(Vec<ToolSchema>, Vec<SourceToolBinding>), ExecutionFault> {
        let Some(expr_data) = self.checked.hir.exprs.get(expr) else {
            return Err(ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                span,
                "`@tools` annotation expression is missing from checked HIR",
            ));
        };
        let elems = match expr_data {
            HirExpr::Array { elems, .. } | HirExpr::List { elems, .. } => elems,
            _ => {
                return Err(ExecutionFault::new(
                    AnalysisDiagnosticCode::InvalidArguments,
                    span,
                    "`@tools` must be an array of resolved tool declarations",
                ));
            }
        };

        let mut tools = Vec::with_capacity(elems.len());
        let mut bindings = Vec::with_capacity(elems.len());
        for elem in elems {
            let (schema, binding) = self.agent_tool_schema(*elem, span)?;
            tools.push(schema);
            if let Some(binding) = binding {
                bindings.push(binding);
            }
        }
        Ok((tools, bindings))
    }

    fn agent_tool_schema(
        &mut self,
        expr: HirExprId,
        fallback_span: Span,
    ) -> Result<(ToolSchema, Option<SourceToolBinding>), ExecutionFault> {
        let Some(expr_data) = self.checked.hir.exprs.get(expr) else {
            return Err(ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                fallback_span,
                "agent tool expression is missing from checked HIR",
            ));
        };
        let span = expr_data.span(&self.checked.hir.blocks);
        let HirExpr::Path(path) = expr_data else {
            return Err(ExecutionFault::new(
                AnalysisDiagnosticCode::InvalidArguments,
                span,
                "agent tool entries must be resolved tool paths",
            ));
        };
        let ResolveResult::Resolved(symbol) = path.resolution else {
            return Err(ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                span,
                "agent tool path is not resolved in checked HIR",
            ));
        };
        let Some(symbol_data) = self.checked.hir.symbols.get(symbol) else {
            return Err(ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                span,
                "agent tool symbol is missing from checked HIR",
            ));
        };
        let Some(signature) = self.tool_signature_for_symbol(symbol) else {
            return Err(ExecutionFault::new(
                AnalysisDiagnosticCode::InvalidArguments,
                span,
                format!(
                    "agent tool entry `{}` is not a tool declaration",
                    symbol_data.name
                ),
            ));
        };
        let (input, binding, tool) = if let Some(tool_item) =
            self.source_tool_item_for_symbol(symbol)
        {
            let Some(qualified_name) = self.source_tool_qualified_name(tool_item, symbol) else {
                return Err(ExecutionFault::new(
                    AnalysisDiagnosticCode::MissingCheckedFact,
                    span,
                    format!(
                        "source tool `{}` is missing a canonical module identity",
                        symbol_data.name
                    ),
                ));
            };
            (
                self.tool_input_schema(symbol, tool_item, &signature, span)?,
                Some(SourceToolBinding {
                    name: symbol_data.name.clone(),
                    qualified_name: Some(qualified_name.clone()),
                    item: tool_item,
                }),
                ToolRef::source(symbol_data.name.clone(), qualified_name),
            )
        } else {
            let Some(qualified_name) = self.external_tool_qualified_name(symbol) else {
                return Err(ExecutionFault::new(
                    AnalysisDiagnosticCode::MissingCheckedFact,
                    span,
                    format!(
                        "external package tool `{}` is missing a canonical package identity",
                        symbol_data.name
                    ),
                ));
            };
            let schema = self.external_tool_input_schema_for_symbol(symbol, span)?;
            (
                schema,
                None,
                ToolRef::external(symbol_data.name.clone(), qualified_name),
            )
        };
        let output = if self.type_is_unit(signature.output) {
            None
        } else {
            Some(self.host_schema_for_type(signature.output, span, 0)?)
        };
        Ok((
            ToolSchema {
                tool,
                input,
                output,
            },
            binding,
        ))
    }

    fn external_tool_qualified_name(&self, symbol: SymbolId) -> Option<String> {
        let symbol_data = self.checked.hir.symbols.get(symbol)?;
        match &symbol_data.def {
            SymbolDef::ImportAlias {
                path,
                origin: etas_hir::ImportAliasOrigin::SourceImport,
            } => Some(path.join(".")),
            _ => None,
        }
    }

    fn source_tool_qualified_name(&self, item: HirItemId, symbol: SymbolId) -> Option<String> {
        let item_name = self.checked.hir.symbols.get(symbol)?.name.as_str();
        for module_id in &self.checked.hir.modules {
            let module = self.checked.hir.modules_arena.get(*module_id)?;
            if !module.items.contains(&item) {
                continue;
            }
            let module_path = module.name.as_ref()?;
            let mut segments = module_path
                .segments
                .iter()
                .map(|segment| segment.name.clone())
                .collect::<Vec<_>>();
            segments.push(item_name.to_owned());
            return Some(segments.join("."));
        }
        None
    }

    fn tool_signature_for_symbol(&self, symbol: SymbolId) -> Option<ToolSignature> {
        match self.checked.types.symbol_types.get(&symbol) {
            Some(SymbolTypeFact::Tool { signature }) => Some(signature.clone()),
            _ => {
                let symbol_data = self.checked.hir.symbols.get(symbol)?;
                let SymbolDef::Item { item } = symbol_data.def else {
                    return None;
                };
                match self.checked.types.item_signatures.get(&item) {
                    Some(etas_types::ItemSignature::Tool(signature)) => Some(signature.clone()),
                    _ => None,
                }
            }
        }
    }

    fn source_tool_item_for_symbol(&self, symbol: SymbolId) -> Option<HirItemId> {
        let symbol_data = self.checked.hir.symbols.get(symbol)?;
        match &symbol_data.def {
            SymbolDef::Item { item } => Some(*item),
            SymbolDef::ImportAlias {
                path,
                origin: etas_hir::ImportAliasOrigin::SourceImport,
            } => self
                .source_item_for_import_path(path)
                .and_then(|ast_item| self.hir_item_for_ast_item(&ast_item)),
            _ => None,
        }
    }

    fn external_tool_input_schema_for_symbol(
        &self,
        symbol: SymbolId,
        span: Span,
    ) -> Result<HostSchema, ExecutionFault> {
        let Some(symbol_data) = self.checked.hir.symbols.get(symbol) else {
            return Err(ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                span,
                "external tool symbol is missing from checked HIR",
            ));
        };
        let SymbolDef::ImportAlias {
            path,
            origin: etas_hir::ImportAliasOrigin::SourceImport,
        } = &symbol_data.def
        else {
            return Err(ExecutionFault::new(
                AnalysisDiagnosticCode::InvalidArguments,
                span,
                "agent tool entry is not backed by an external package import",
            ));
        };
        let Some(schema) = self
            .checked
            .external_tool_schemas
            .iter()
            .find(|schema| schema.path == *path)
        else {
            return Err(ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                span,
                format!(
                    "external tool `{}` is missing package tool schema metadata",
                    path.join(".")
                ),
            ));
        };
        let value = match serde_json::from_str::<Value>(&schema.schema_json) {
            Ok(value) => value,
            Err(error) => {
                return Err(ExecutionFault::new(
                    AnalysisDiagnosticCode::MissingCheckedFact,
                    span,
                    format!(
                        "external tool `{}` has invalid package tool schema JSON: {error}",
                        path.join(".")
                    ),
                ));
            }
        };
        match host_schema_from_json_schema(&value) {
            Ok(schema) => Ok(schema),
            Err(message) => Err(ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                span,
                format!(
                    "external tool `{}` has unsupported package tool schema: {message}",
                    path.join(".")
                ),
            )),
        }
    }

    fn tool_input_schema(
        &mut self,
        symbol: SymbolId,
        tool_item: HirItemId,
        signature: &ToolSignature,
        span: Span,
    ) -> Result<HostSchema, ExecutionFault> {
        let Some(symbol_data) = self.checked.hir.symbols.get(symbol) else {
            return Err(ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                span,
                "agent tool symbol is missing from checked HIR",
            ));
        };
        let Some(HirItem::Tool(tool)) = self.checked.hir.items.get(tool_item) else {
            return Err(ExecutionFault::new(
                AnalysisDiagnosticCode::InvalidArguments,
                span,
                format!(
                    "agent tool entry `{}` is not backed by a source tool item",
                    symbol_data.name
                ),
            ));
        };
        if tool.params.len() != signature.params.len() {
            return Err(ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                span,
                format!(
                    "tool `{}` signature parameter facts do not match HIR parameters",
                    symbol_data.name
                ),
            ));
        }

        let mut fields = Vec::with_capacity(tool.params.len());
        for (param, ty) in tool.params.iter().zip(signature.params.iter()) {
            let Some(param_symbol) = self.checked.hir.symbols.get(*param) else {
                return Err(ExecutionFault::new(
                    AnalysisDiagnosticCode::MissingCheckedFact,
                    span,
                    format!(
                        "tool `{}` parameter is missing from checked HIR",
                        symbol_data.name
                    ),
                ));
            };
            fields.push(HostFieldSchema {
                name: param_symbol.name.clone(),
                schema: self.host_schema_for_type(*ty, span, 0)?,
                optional: false,
            });
        }
        Ok(HostSchema::Record(fields))
    }

    fn host_schema_for_type(
        &self,
        ty: TypeId,
        span: Span,
        depth: usize,
    ) -> Result<HostSchema, ExecutionFault> {
        if depth > 16 {
            return Err(ExecutionFault::new(
                AnalysisDiagnosticCode::InvalidArguments,
                span,
                "tool schema type nesting exceeds interpreter host schema limit",
            ));
        }
        let Some(ty_data) = self.checked.type_store.get(ty).cloned() else {
            return Err(ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                span,
                "tool schema type is missing checked type facts",
            ));
        };
        match ty_data {
            Type::Primitive(primitive) => Ok(Self::host_schema_for_primitive(primitive)),
            Type::IntegerLiteral { .. } => Ok(HostSchema::Int),
            Type::Array(inner) | Type::List(inner) | Type::Set(inner) | Type::Slice(inner) => {
                Ok(HostSchema::List(Box::new(self.host_schema_for_type(
                    inner,
                    span,
                    depth + 1,
                )?)))
            }
            Type::Map { key, value } => Ok(HostSchema::Map {
                key: Box::new(self.host_schema_for_type(key, span, depth + 1)?),
                value: Box::new(self.host_schema_for_type(value, span, depth + 1)?),
            }),
            Type::Option(inner) => Ok(HostSchema::Variant(vec![
                HostVariantSchema {
                    name: "None".to_owned(),
                    fields: Vec::new(),
                },
                HostVariantSchema {
                    name: "Some".to_owned(),
                    fields: vec![self.host_schema_for_type(inner, span, depth + 1)?],
                },
            ])),
            Type::Result { ok, err } => Ok(HostSchema::Variant(vec![
                HostVariantSchema {
                    name: "Ok".to_owned(),
                    fields: vec![self.host_schema_for_type(ok, span, depth + 1)?],
                },
                HostVariantSchema {
                    name: "Err".to_owned(),
                    fields: vec![self.host_schema_for_type(err, span, depth + 1)?],
                },
            ])),
            Type::Record(record) => {
                let mut fields = Vec::with_capacity(record.fields.len());
                for field in record.fields {
                    fields.push(HostFieldSchema {
                        name: field.name,
                        schema: self.host_schema_for_type(field.ty, span, depth + 1)?,
                        optional: false,
                    });
                }
                Ok(HostSchema::Record(fields))
            }
            Type::Tuple(elems) => {
                let mut fields = Vec::with_capacity(elems.len());
                for (index, elem) in elems.into_iter().enumerate() {
                    fields.push(HostFieldSchema {
                        name: index.to_string(),
                        schema: self.host_schema_for_type(elem, span, depth + 1)?,
                        optional: false,
                    });
                }
                Ok(HostSchema::Record(fields))
            }
            Type::Trust { inner, .. }
            | Type::Schema(inner)
            | Type::Message(inner)
            | Type::MemorySelection(inner)
            | Type::MemoryRegion(inner)
            | Type::Refined { base: inner, .. } => {
                self.host_schema_for_type(inner, span, depth + 1)
            }
            Type::Nominal(nominal) => {
                let Some(representation) = nominal.representation else {
                    return Err(ExecutionFault::new(
                        AnalysisDiagnosticCode::InvalidArguments,
                        span,
                        format!(
                            "tool schema type `{}` has no representation and cannot be lowered to host schema",
                            nominal.name
                        ),
                    ));
                };
                self.host_schema_for_type(representation, span, depth + 1)
            }
            Type::Prompt | Type::PromptPart => Ok(HostSchema::String),
            Type::Named(named) if named.name.rsplit('.').next() == Some("JsonValue") => {
                Ok(HostSchema::Json)
            }
            unsupported => Err(ExecutionFault::new(
                AnalysisDiagnosticCode::InvalidArguments,
                span,
                format!("tool schema type `{unsupported:?}` cannot be lowered to host schema"),
            )),
        }
    }

    fn host_schema_for_primitive(primitive: PrimitiveType) -> HostSchema {
        match primitive {
            PrimitiveType::Bool => HostSchema::Bool,
            PrimitiveType::I8
            | PrimitiveType::I16
            | PrimitiveType::I32
            | PrimitiveType::I64
            | PrimitiveType::I128
            | PrimitiveType::ISize => HostSchema::Int,
            PrimitiveType::U8
            | PrimitiveType::U16
            | PrimitiveType::U32
            | PrimitiveType::U64
            | PrimitiveType::U128
            | PrimitiveType::USize => HostSchema::UInt,
            PrimitiveType::F32 | PrimitiveType::F64 => HostSchema::Float,
            PrimitiveType::String | PrimitiveType::Char => HostSchema::String,
            PrimitiveType::Bytes => HostSchema::Bytes,
            PrimitiveType::Unit | PrimitiveType::Never => HostSchema::Unit,
        }
    }

    fn const_string_expr(&self, expr: HirExprId) -> Option<String> {
        match self.checked.hir.exprs.get(expr)? {
            HirExpr::Literal(HirLiteral::String { value, .. }) => Some(value.clone()),
            _ => None,
        }
    }

    fn agent_model_decode(&self, output_type: TypeId) -> ModelDecode {
        match self.model_policy.response_decode {
            crate::api::ModelResponseDecodePolicy::ModelResponse => ModelDecode::ModelResponse,
            crate::api::ModelResponseDecodePolicy::String => {
                if self.type_is_string(output_type) {
                    ModelDecode::String
                } else if self.type_is_model_response(output_type) {
                    ModelDecode::ModelResponse
                } else {
                    ModelDecode::Typed(output_type)
                }
            }
        }
    }

    fn type_is_string(&self, ty: TypeId) -> bool {
        matches!(
            self.checked.type_store.get(ty),
            Some(Type::Primitive(PrimitiveType::String))
        )
    }

    fn type_is_model_response(&self, ty: TypeId) -> bool {
        matches!(
            self.checked.type_store.get(ty),
            Some(Type::Named(named)) if named.name == "ModelResponse"
        )
    }

    fn type_is_unit(&self, ty: TypeId) -> bool {
        matches!(
            self.checked.type_store.get(ty),
            Some(Type::Primitive(PrimitiveType::Unit))
        )
    }
}

fn host_schema_from_json_schema(value: &Value) -> Result<HostSchema, String> {
    let Some(object) = value.as_object() else {
        return Err("schema must be a JSON object".to_owned());
    };
    if object.is_empty() {
        return Ok(HostSchema::Json);
    }
    if let Some(any_of) = object.get("anyOf") {
        return option_schema_from_any_of(any_of);
    }
    if let Some(one_of) = object.get("oneOf") {
        return variant_schema_from_one_of(one_of);
    }
    match object.get("type").and_then(Value::as_str) {
        Some("null") => Ok(HostSchema::Unit),
        Some("boolean") => Ok(HostSchema::Bool),
        Some("integer") => {
            if object
                .get("minimum")
                .and_then(Value::as_i64)
                .is_some_and(|minimum| minimum >= 0)
            {
                Ok(HostSchema::UInt)
            } else {
                Ok(HostSchema::Int)
            }
        }
        Some("number") => Ok(HostSchema::Float),
        Some("string") => Ok(HostSchema::String),
        Some("array") => array_schema_from_object(object),
        Some("object") => object_schema_from_object(object),
        Some(other) => Err(format!("unsupported JSON schema type `{other}`")),
        None => Err("schema is missing `type`, `anyOf`, or `oneOf`".to_owned()),
    }
}

fn object_schema_from_object(
    object: &serde_json::Map<String, Value>,
) -> Result<HostSchema, String> {
    if let Some(properties) = object.get("properties") {
        let properties = properties
            .as_object()
            .ok_or_else(|| "`properties` must be an object".to_owned())?;
        let required = object
            .get("required")
            .map(required_field_names)
            .transpose()?
            .unwrap_or_default();
        let mut fields = Vec::with_capacity(properties.len());
        for (name, schema) in properties {
            fields.push(HostFieldSchema {
                name: name.clone(),
                schema: host_schema_from_json_schema(schema)?,
                optional: !required.iter().any(|required| required == name),
            });
        }
        fields.sort_by(|left, right| left.name.cmp(&right.name));
        return Ok(HostSchema::Record(fields));
    }
    if let Some(additional) = object.get("additionalProperties") {
        if additional == &Value::Bool(false) {
            return Ok(HostSchema::Record(Vec::new()));
        }
        return Ok(HostSchema::Map {
            key: Box::new(HostSchema::String),
            value: Box::new(host_schema_from_json_schema(additional)?),
        });
    }
    Err("object schema must contain `properties` or `additionalProperties`".to_owned())
}

fn array_schema_from_object(object: &serde_json::Map<String, Value>) -> Result<HostSchema, String> {
    if let Some(prefix_items) = object.get("prefixItems") {
        let prefix_items = prefix_items
            .as_array()
            .ok_or_else(|| "`prefixItems` must be an array".to_owned())?;
        let mut fields = Vec::with_capacity(prefix_items.len());
        for (index, item) in prefix_items.iter().enumerate() {
            fields.push(HostFieldSchema {
                name: index.to_string(),
                schema: host_schema_from_json_schema(item)?,
                optional: false,
            });
        }
        return Ok(HostSchema::Record(fields));
    }
    let items = object
        .get("items")
        .ok_or_else(|| "array schema is missing `items`".to_owned())?;
    Ok(HostSchema::List(Box::new(host_schema_from_json_schema(
        items,
    )?)))
}

fn option_schema_from_any_of(value: &Value) -> Result<HostSchema, String> {
    let items = value
        .as_array()
        .ok_or_else(|| "`anyOf` must be an array".to_owned())?;
    if items.len() != 2 {
        return Err("only two-arm Option anyOf schemas are supported".to_owned());
    }
    let mut non_null = None;
    for item in items {
        if item
            .as_object()
            .and_then(|object| object.get("type"))
            .and_then(Value::as_str)
            == Some("null")
        {
            continue;
        }
        if non_null.is_some() {
            return Err("Option anyOf contains more than one non-null schema".to_owned());
        }
        non_null = Some(host_schema_from_json_schema(item)?);
    }
    let Some(inner) = non_null else {
        return Err("Option anyOf is missing a non-null schema".to_owned());
    };
    Ok(HostSchema::Variant(vec![
        HostVariantSchema {
            name: "None".to_owned(),
            fields: Vec::new(),
        },
        HostVariantSchema {
            name: "Some".to_owned(),
            fields: vec![inner],
        },
    ]))
}

fn variant_schema_from_one_of(value: &Value) -> Result<HostSchema, String> {
    let items = value
        .as_array()
        .ok_or_else(|| "`oneOf` must be an array".to_owned())?;
    let mut variants = Vec::with_capacity(items.len());
    for item in items {
        let object = item
            .as_object()
            .ok_or_else(|| "variant schema arm must be an object".to_owned())?;
        let properties = object
            .get("properties")
            .and_then(Value::as_object)
            .ok_or_else(|| "variant schema arm is missing object `properties`".to_owned())?;
        let name = properties
            .get("name")
            .and_then(Value::as_object)
            .and_then(|name| name.get("const"))
            .and_then(Value::as_str)
            .ok_or_else(|| "variant schema arm is missing `name.const`".to_owned())?;
        let field_items = properties
            .get("fields")
            .and_then(Value::as_object)
            .and_then(|fields| fields.get("prefixItems"))
            .and_then(Value::as_array)
            .ok_or_else(|| "variant schema arm is missing `fields.prefixItems`".to_owned())?;
        variants.push(HostVariantSchema {
            name: name.to_owned(),
            fields: field_items
                .iter()
                .map(host_schema_from_json_schema)
                .collect::<Result<Vec<_>, _>>()?,
        });
    }
    Ok(HostSchema::Variant(variants))
}

fn required_field_names(value: &Value) -> Result<Vec<String>, String> {
    value
        .as_array()
        .ok_or_else(|| "`required` must be an array".to_owned())?
        .iter()
        .map(|item| {
            item.as_str()
                .map(str::to_owned)
                .ok_or_else(|| "`required` entries must be strings".to_owned())
        })
        .collect()
}

fn annotation_name(annotation: &etas_hir::HirAnnotation) -> String {
    annotation
        .path
        .segments
        .iter()
        .map(|segment| segment.name.as_str())
        .collect::<Vec<_>>()
        .join(".")
}
