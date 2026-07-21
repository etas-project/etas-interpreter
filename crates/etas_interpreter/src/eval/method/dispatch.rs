use super::*;

impl<'a> EvalContext<'a> {
    pub(in crate::eval) fn eval_method_call(
        &mut self,
        dispatch: MethodDispatch<'_>,
        receiver: HirExprId,
        frame: &mut Frame,
    ) -> ControlSignal {
        let MethodDispatch {
            expr,
            method,
            type_args,
            args,
            span,
        } = dispatch;
        if self.expr_is_std_prompt_type(receiver) {
            return self.resume_static_method_args(
                StaticMethodArgsState {
                    expr,
                    kind: StaticMethodKind::Prompt,
                    method: method.to_owned(),
                    type_args: type_args.to_vec(),
                    args: args.to_vec(),
                    start_arg_index: 0,
                    evaluated_args: Vec::new(),
                    span,
                },
                frame,
            );
        }
        if self.expr_is_std_message_type(receiver) {
            return self.resume_static_method_args(
                StaticMethodArgsState {
                    expr,
                    kind: StaticMethodKind::Message,
                    method: method.to_owned(),
                    type_args: type_args.to_vec(),
                    args: args.to_vec(),
                    start_arg_index: 0,
                    evaluated_args: Vec::new(),
                    span,
                },
                frame,
            );
        }
        if self.expr_is_std_session_config_type(receiver) {
            return self.resume_static_method_args(
                StaticMethodArgsState {
                    expr,
                    kind: StaticMethodKind::SessionConfig,
                    method: method.to_owned(),
                    type_args: type_args.to_vec(),
                    args: args.to_vec(),
                    start_arg_index: 0,
                    evaluated_args: Vec::new(),
                    span,
                },
                frame,
            );
        }
        if self.expr_is_std_conversation_type(receiver) {
            return self.resume_static_method_args(
                StaticMethodArgsState {
                    expr,
                    kind: StaticMethodKind::Conversation,
                    method: method.to_owned(),
                    type_args: type_args.to_vec(),
                    args: args.to_vec(),
                    start_arg_index: 0,
                    evaluated_args: Vec::new(),
                    span,
                },
                frame,
            );
        }
        if self.expr_is_std_range_type(receiver) {
            return self.resume_static_method_args(
                StaticMethodArgsState {
                    expr,
                    kind: StaticMethodKind::Range,
                    method: method.to_owned(),
                    type_args: type_args.to_vec(),
                    args: args.to_vec(),
                    start_arg_index: 0,
                    evaluated_args: Vec::new(),
                    span,
                },
                frame,
            );
        }
        if let Some(collection) = self.std_advanced_collection_type(receiver) {
            return self.resume_static_method_args(
                StaticMethodArgsState {
                    expr,
                    kind: StaticMethodKind::AdvancedCollection(collection.to_owned()),
                    method: method.to_owned(),
                    type_args: type_args.to_vec(),
                    args: args.to_vec(),
                    start_arg_index: 0,
                    evaluated_args: Vec::new(),
                    span,
                },
                frame,
            );
        }
        if method == "run"
            && let Some(CallTarget::AgentItem(item)) =
                self.resolve_static_call_target(receiver, frame)
        {
            return self.resume_call_args(
                CallTarget::AgentItem(item),
                args.to_vec(),
                0,
                Vec::new(),
                span,
                frame,
            );
        }
        let receiver = match self.eval_expr(receiver, frame) {
            ControlSignal::Value(value) => value,
            ControlSignal::Apply(pending) => {
                return attach_method_receiver_continuation(
                    ControlSignal::Apply(pending),
                    expr,
                    method,
                    type_args,
                    args,
                    span,
                    frame,
                );
            }
            ControlSignal::Checkpoint(pending) => {
                return ControlSignal::Checkpoint(pending);
            }
            ControlSignal::Block(pending) => {
                return attach_method_receiver_continuation(
                    ControlSignal::Block(pending),
                    expr,
                    method,
                    type_args,
                    args,
                    span,
                    frame,
                );
            }
            ControlSignal::Expr(pending) => {
                return attach_method_receiver_continuation(
                    ControlSignal::Expr(pending),
                    expr,
                    method,
                    type_args,
                    args,
                    span,
                    frame,
                );
            }
            ControlSignal::Call(call) => {
                return attach_method_receiver_continuation(
                    ControlSignal::Call(call),
                    expr,
                    method,
                    type_args,
                    args,
                    span,
                    frame,
                );
            }
            ControlSignal::Perform(perform) => {
                return attach_method_receiver_continuation(
                    ControlSignal::Perform(perform),
                    expr,
                    method,
                    type_args,
                    args,
                    span,
                    frame,
                );
            }
            ControlSignal::Memory(memory) => {
                return attach_method_receiver_continuation(
                    ControlSignal::Memory(memory),
                    expr,
                    method,
                    type_args,
                    args,
                    span,
                    frame,
                );
            }
            ControlSignal::Session(session) => {
                return attach_method_receiver_continuation(
                    ControlSignal::Session(session),
                    expr,
                    method,
                    type_args,
                    args,
                    span,
                    frame,
                );
            }
            ControlSignal::Console(console) => {
                return attach_method_receiver_continuation(
                    ControlSignal::Console(console),
                    expr,
                    method,
                    type_args,
                    args,
                    span,
                    frame,
                );
            }
            ControlSignal::Command(command) => {
                return attach_method_receiver_continuation(
                    ControlSignal::Command(command),
                    expr,
                    method,
                    type_args,
                    args,
                    span,
                    frame,
                );
            }
            ControlSignal::Model(model) => {
                return attach_method_receiver_continuation(
                    ControlSignal::Model(model),
                    expr,
                    method,
                    type_args,
                    args,
                    span,
                    frame,
                );
            }
            ControlSignal::Host(host) => {
                return attach_method_receiver_continuation(
                    ControlSignal::Host(host),
                    expr,
                    method,
                    type_args,
                    args,
                    span,
                    frame,
                );
            }
            ControlSignal::Return(value) => return ControlSignal::Return(value),
            ControlSignal::Resume(value) => return ControlSignal::Resume(value),
            ControlSignal::Finish(value) => return ControlSignal::Finish(value),
            ControlSignal::Break => return ControlSignal::Break,
            ControlSignal::Fault(fault) => return ControlSignal::Fault(fault),
            ControlSignal::Continue => return ControlSignal::Continue,
        };
        self.eval_method_on_receiver(
            MethodDispatch {
                expr,
                method,
                type_args,
                args,
                span,
            },
            receiver,
            frame,
        )
    }

    pub(in crate::eval) fn eval_method_on_receiver(
        &mut self,
        dispatch: MethodDispatch<'_>,
        receiver: InterpValue,
        frame: &mut Frame,
    ) -> ControlSignal {
        let MethodDispatch {
            expr,
            method,
            type_args,
            args,
            span,
        } = dispatch;
        if local_value_method_expected_arg_count(&receiver, method).is_some() {
            return self.resume_local_method_args(
                LocalMethodArgsState {
                    expr,
                    receiver,
                    method: method.to_owned(),
                    type_args: type_args.to_vec(),
                    args: args.to_vec(),
                    start_arg_index: 0,
                    evaluated_args: Vec::new(),
                    span,
                },
                frame,
            );
        }
        match receiver {
            InterpValue::Prompt(messages) => {
                self.eval_prompt_value_method(messages, method, args, span, frame)
            }
            InterpValue::Message(message) => {
                self.eval_message_value_method(message, method, type_args, args, span)
            }
            InterpValue::Array(values) => {
                self.eval_array_method(expr, values, method, args, span, frame)
            }
            InterpValue::List(values) => self.eval_list_method(values, method, args, span, frame),
            InterpValue::Slice(values) => {
                self.eval_slice_method(expr, values, method, args, span, frame)
            }
            InterpValue::Map(entries) => self.eval_map_method(entries, method, args, span, frame),
            InterpValue::Deque(values) => self.eval_deque_method(values, method, args, span, frame),
            InterpValue::Queue(values) => self.eval_queue_method(values, method, args, span, frame),
            InterpValue::Stack(values) => self.eval_stack_method(values, method, args, span, frame),
            InterpValue::PriorityQueue(entries) => {
                self.eval_priority_queue_method(entries, method, args, span, frame)
            }
            InterpValue::OrderedMap(entries) => {
                self.eval_ordered_map_method(entries, method, args, span, frame)
            }
            InterpValue::OrderedSet(values) => {
                self.eval_ordered_set_method(values, method, args, span, frame)
            }
            InterpValue::MemoryStore {
                region_stable_id,
                path,
                key_type,
                value_type,
            } => self.eval_memory_store_method(
                MemoryStoreMethodEval {
                    region_stable_id,
                    path,
                    key_type,
                    value_type,
                    method,
                    args,
                    span,
                },
                frame,
            ),
            InterpValue::MemorySelection {
                region_stable_id,
                path,
                key_type,
                value_type,
                kind,
                predicate,
                limit,
            } => self.eval_memory_selection_method(
                MemorySelectionMethodEval {
                    region_stable_id,
                    path,
                    key_type,
                    value_type,
                    kind,
                    predicate: predicate.map(|value| *value),
                    limit,
                    method,
                    args,
                    span,
                },
                frame,
            ),
            _ => {
                let message = format!(
                    "method `{method}` is not supported for runtime value {:?}",
                    receiver
                );
                ControlSignal::invalid_arguments(message, span)
            }
        }
    }

    pub(in crate::eval) fn resume_static_method_args(
        &mut self,
        state: StaticMethodArgsState,
        frame: &mut Frame,
    ) -> ControlSignal {
        let StaticMethodArgsState {
            expr: call_expr,
            kind,
            method,
            type_args,
            args,
            start_arg_index,
            mut evaluated_args,
            span,
        } = state;
        for (index, arg) in args.iter().enumerate().skip(start_arg_index) {
            let arg_expr = match arg {
                HirArg::Positional(expr) | HirArg::Named { value: expr, .. } => *expr,
            };
            match self.eval_expr(arg_expr, frame) {
                ControlSignal::Value(value) => evaluated_args.push(value),
                signal if is_pending_host_boundary_signal(&signal) => {
                    return compose_signal_continuation(
                        signal,
                        Continuation::StaticMethodArgs {
                            expr: call_expr,
                            kind,
                            method,
                            type_args,
                            args,
                            next_arg_index: index + 1,
                            evaluated_args,
                            span,
                            frame: frame.clone(),
                        },
                    );
                }
                other => return other,
            }
        }
        self.eval_static_method_values(call_expr, kind, &method, &type_args, &evaluated_args, span)
    }

    pub(in crate::eval) fn eval_static_method_values(
        &mut self,
        expr: HirExprId,
        kind: StaticMethodKind,
        method: &str,
        type_args: &[etas_hir::HirTypeId],
        args: &[InterpValue],
        span: Span,
    ) -> ControlSignal {
        match kind {
            StaticMethodKind::Prompt => {
                if method != "new" || !args.is_empty() {
                    return unsupported_method(span, "Prompt type", method);
                }
                ControlSignal::Value(InterpValue::Prompt(Vec::new()))
            }
            StaticMethodKind::Message => self.eval_message_type_method_values(method, args, span),
            StaticMethodKind::SessionConfig => {
                self.eval_session_config_type_method_values(method, args, span)
            }
            StaticMethodKind::Conversation => {
                self.eval_conversation_type_method_values(expr, method, args, span)
            }
            StaticMethodKind::Range => self.eval_range_type_method_values(method, args, span),
            StaticMethodKind::AdvancedCollection(collection) => {
                self.eval_advanced_collection_type_method(&collection, method, type_args, &[], span)
            }
        }
    }

    pub(in crate::eval) fn expr_is_std_prompt_type(&self, expr: HirExprId) -> bool {
        let HirExpr::Path(path) = &self.checked.hir.exprs[expr] else {
            return false;
        };
        let ResolveResult::Resolved(symbol) = path.resolution else {
            return false;
        };
        let Some(symbol_data) = self.checked.symbols.get(symbol) else {
            return false;
        };
        matches!(
            &symbol_data.def,
            SymbolDef::ImportAlias { path, .. }
                if path == &["std", "agent", "prompt", "Prompt"].map(str::to_owned)
        )
    }

    pub(in crate::eval) fn expr_is_std_range_type(&self, expr: HirExprId) -> bool {
        self.expr_is_std_type(expr, &["std", "collections", "Range"])
    }

    pub(in crate::eval) fn expr_is_std_message_type(&self, expr: HirExprId) -> bool {
        self.expr_is_std_type(expr, &["std", "agent", "message", "Message"])
    }

    pub(in crate::eval) fn expr_is_std_session_config_type(&self, expr: HirExprId) -> bool {
        self.expr_is_std_type(expr, &["std", "agent", "session", "SessionConfig"])
    }

    pub(in crate::eval) fn expr_is_std_conversation_type(&self, expr: HirExprId) -> bool {
        self.expr_is_std_type(expr, &["std", "agent", "session", "Conversation"])
    }

    pub(in crate::eval) fn std_advanced_collection_type(
        &self,
        expr: HirExprId,
    ) -> Option<&'static str> {
        for (name, path) in [
            ("Deque", &["std", "collections", "Deque"][..]),
            ("Queue", &["std", "collections", "Queue"][..]),
            ("Stack", &["std", "collections", "Stack"][..]),
            (
                "PriorityQueue",
                &["std", "collections", "PriorityQueue"][..],
            ),
            ("OrderedMap", &["std", "collections", "OrderedMap"][..]),
            ("OrderedSet", &["std", "collections", "OrderedSet"][..]),
        ] {
            if self.expr_is_std_type(expr, path) {
                return Some(name);
            }
        }
        None
    }

    pub(in crate::eval) fn expr_is_std_type(&self, expr: HirExprId, expected: &[&str]) -> bool {
        let HirExpr::Path(path) = &self.checked.hir.exprs[expr] else {
            return false;
        };
        let ResolveResult::Resolved(symbol) = path.resolution else {
            return false;
        };
        let Some(symbol_data) = self.checked.symbols.get(symbol) else {
            return false;
        };
        matches!(
            &symbol_data.def,
            SymbolDef::ImportAlias { path, .. }
                if path.iter().map(String::as_str).eq(expected.iter().copied())
        )
    }
}
