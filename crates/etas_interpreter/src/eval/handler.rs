use super::*;
use crate::control::ExecutionFault;

impl<'a> EvalContext<'a> {
    pub(super) fn eval_handler_value_expr(
        &mut self,
        expr: HirExprId,
        handlers: &[etas_hir::HirHandlerArmId],
        span: Span,
    ) -> ControlSignal {
        let Some(fact) = self.checked.effects.handler_values.get(&expr) else {
            return ControlSignal::missing_checked_fact(
                "handler literal requires a checked HandlerValueFact",
                span,
            );
        };
        let arm_facts = fact.arms.clone();
        let handlers = match self.active_handler_arms(handlers, &arm_facts, span) {
            Ok(handlers) => handlers,
            Err(fault) => return ControlSignal::Fault(Box::new(fault)),
        };
        ControlSignal::Value(InterpValue::Handler {
            fact_expr: expr,
            handlers,
        })
    }

    pub(super) fn eval_handle_expr(
        &mut self,
        handle_expr: HirExprId,
        body: HirExprId,
        handler: HirExprId,
        span: Span,
        frame: &mut Frame,
    ) -> ControlSignal {
        let Some(_application) = self.checked.effects.handle_applications.get(&handle_expr) else {
            return ControlSignal::missing_checked_fact(
                "handle execution requires a checked HandleApplicationFact",
                span,
            );
        };
        if let Some(path) = self.external_handler_value_path(handler) {
            let message = format!(
                "external handler value `{}` has a checked metadata contract but no runtime handler body in metadata-only package mode",
                path.join(".")
            );
            return ControlSignal::fault(
                AnalysisDiagnosticCode::UnsupportedPhase2RuntimeFeature,
                span,
                message,
            );
        }
        match self.eval_expr(handler, frame) {
            ControlSignal::Value(value) => {
                self.resume_handle_handler(handle_expr, body, handler, span, value, frame)
            }
            signal if is_pending_host_boundary_signal(&signal) => compose_signal_continuation(
                signal,
                Continuation::HandleHandler {
                    handle_expr,
                    body,
                    handler,
                    span,
                    frame: frame.clone(),
                },
            ),
            other => other,
        }
    }

    pub(super) fn resume_handle_handler(
        &mut self,
        handle_expr: HirExprId,
        body: HirExprId,
        handler: HirExprId,
        span: Span,
        value: InterpValue,
        frame: &mut Frame,
    ) -> ControlSignal {
        let Some(application) = self.checked.effects.handle_applications.get(&handle_expr) else {
            return ControlSignal::missing_checked_fact(
                "handle execution requires a checked HandleApplicationFact",
                span,
            );
        };
        match value {
            InterpValue::Handler {
                fact_expr,
                handlers: handler_records,
            } => {
                if !matches!(
                    application.handler,
                    etas_effects::HandlerValueRef::Expr { expr: handler_expr }
                        if handler_expr == handler
                ) || !self.handler_value_fact_matches_application(fact_expr, application)
                {
                    return ControlSignal::missing_checked_fact(
                        "handler value does not match the checked HandleApplicationFact",
                        span,
                    );
                }
                self.eval_handle_with_records(body, handler_records, span, frame)
            }
            other => ControlSignal::invalid_arguments(
                format!("handle expects a handler value, got {other:?}"),
                span,
            ),
        }
    }

    fn external_handler_value_path(&self, expr: HirExprId) -> Option<Vec<String>> {
        let HirExpr::Path(path) = &self.checked.hir.exprs[expr] else {
            return None;
        };
        let ResolveResult::Resolved(symbol) = path.resolution else {
            return None;
        };
        let symbol_data = self.checked.symbols.get(symbol)?;
        let SymbolDef::ImportAlias { path, .. } = &symbol_data.def else {
            return None;
        };
        if self.source_item_for_import_path(path).is_some() {
            return None;
        }
        let Some(etas_types::SymbolTypeFact::TopLevelLet { ty, classification }) =
            self.checked.types.symbol_types.get(&symbol)
        else {
            return None;
        };
        if !matches!(classification, TopLevelLetClassification::Handler)
            && !matches!(
                self.checked.type_store.get(*ty),
                Some(etas_types::Type::Handler(_))
            )
        {
            return None;
        }
        Some(path.clone())
    }

    fn active_handler_arms(
        &mut self,
        handlers: &[etas_hir::HirHandlerArmId],
        facts: &[etas_effects::HandlerArmFact],
        span: Span,
    ) -> Result<Vec<ActiveHandlerArmRecord>, ExecutionFault> {
        if handlers.len() != facts.len() {
            return Err(missing_handler_value_fault(span));
        }
        let mut records = Vec::with_capacity(handlers.len());
        for (handler_id, fact) in handlers.iter().zip(facts) {
            let Some(handler) = self.checked.hir.handler_arms.get(*handler_id) else {
                return Err(missing_handler_value_fault(span));
            };
            if !handler_arm_fact_matches_hir(handler, fact) {
                return Err(missing_handler_value_fault(span));
            }
            let Some(body) = self.handler_arm_body(*handler_id) else {
                return Err(missing_handler_value_fault(span));
            };
            records.push(active_handler_arm(handler, fact, body));
        }
        Ok(records)
    }

    fn handler_value_fact_matches_application(
        &self,
        fact_expr: HirExprId,
        application: &etas_effects::HandleApplicationFact,
    ) -> bool {
        let Some(fact) = self.checked.effects.handler_values.get(&fact_expr) else {
            return false;
        };
        let coverage = etas_effects::EffectCoverage {
            registry: &self.checked.effect_registry,
            types: &self.checked.type_store,
        };
        handler_rows_compatible(&coverage, &application.handled, &fact.handled)
            && coverage.row_covers(&application.produced, &fact.produced)
    }

    fn eval_handle_with_records(
        &mut self,
        body: HirExprId,
        handler_records: Vec<ActiveHandlerArmRecord>,
        span: Span,
        frame: &mut Frame,
    ) -> ControlSignal {
        let scope_id = HandlerScopeId(self.next_handler_scope);
        self.next_handler_scope += 1;
        self.handler_stack.push(ActiveHandlerRecord {
            id: scope_id,
            handled_actions: handler_records
                .iter()
                .map(|handler| format!("{}.{}", handler.effect_segments.join("."), handler.action))
                .collect(),
            handlers: handler_records.clone(),
            span,
        });
        let mut signal = self.eval_expr(body, frame);
        loop {
            match signal {
                ControlSignal::Apply(pending) => {
                    return self.wrap_handler_boundary_signal(
                        ControlSignal::Apply(pending),
                        scope_id,
                        handler_records,
                        span,
                        frame,
                    );
                }
                ControlSignal::Perform(perform) => {
                    signal = match self.dispatch_handler(*perform.clone(), &handler_records, frame)
                    {
                        Some(signal) => signal,
                        None => {
                            if let Err(fault) = self.close_handler_scope(scope_id, span) {
                                return ControlSignal::Fault(Box::new(fault));
                            }
                            return ControlSignal::Perform(perform);
                        }
                    };
                }
                ControlSignal::Block(block) => {
                    return self.wrap_handler_boundary_signal(
                        ControlSignal::Block(block),
                        scope_id,
                        handler_records,
                        span,
                        frame,
                    );
                }
                ControlSignal::Expr(expr) => {
                    return self.wrap_handler_boundary_signal(
                        ControlSignal::Expr(expr),
                        scope_id,
                        handler_records,
                        span,
                        frame,
                    );
                }
                ControlSignal::Call(call) => {
                    return self.wrap_handler_boundary_signal(
                        ControlSignal::Call(call),
                        scope_id,
                        handler_records,
                        span,
                        frame,
                    );
                }
                ControlSignal::Memory(memory) => {
                    return self.wrap_handler_boundary_signal(
                        ControlSignal::Memory(memory),
                        scope_id,
                        handler_records,
                        span,
                        frame,
                    );
                }
                ControlSignal::Session(session) => {
                    return self.wrap_handler_boundary_signal(
                        ControlSignal::Session(session),
                        scope_id,
                        handler_records,
                        span,
                        frame,
                    );
                }
                ControlSignal::Console(console) => {
                    return self.wrap_handler_boundary_signal(
                        ControlSignal::Console(console),
                        scope_id,
                        handler_records,
                        span,
                        frame,
                    );
                }
                ControlSignal::Command(command) => {
                    return self.wrap_handler_boundary_signal(
                        ControlSignal::Command(command),
                        scope_id,
                        handler_records,
                        span,
                        frame,
                    );
                }
                ControlSignal::Model(model) => {
                    return self.wrap_handler_boundary_signal(
                        ControlSignal::Model(model),
                        scope_id,
                        handler_records,
                        span,
                        frame,
                    );
                }
                ControlSignal::Host(host) => {
                    return self.wrap_handler_boundary_signal(
                        ControlSignal::Host(host),
                        scope_id,
                        handler_records,
                        span,
                        frame,
                    );
                }
                other => {
                    if let Err(fault) = self.close_handler_scope(scope_id, span) {
                        return ControlSignal::Fault(Box::new(fault));
                    }
                    return other;
                }
            }
        }
    }

    pub(super) fn drive_restored_handlers(
        &mut self,
        mut signal: ControlSignal,
        frame: &mut Frame,
    ) -> ControlSignal {
        loop {
            let Some(record) = self.handler_stack.last().cloned() else {
                return signal;
            };
            match signal {
                ControlSignal::Perform(perform) => {
                    signal = match self.dispatch_handler(*perform.clone(), &record.handlers, frame)
                    {
                        Some(signal) => signal,
                        None => return ControlSignal::Perform(perform),
                    };
                }
                _ => return signal,
            }
        }
    }

    fn dispatch_handler(
        &mut self,
        perform: PendingPerform,
        handlers: &[ActiveHandlerArmRecord],
        frame: &Frame,
    ) -> Option<ControlSignal> {
        let handler = handlers
            .iter()
            .find(|handler| self.handler_matches_perform(&perform, handler))?;
        let mut handler_frame = frame.clone();
        if let Err(fault) = self.bind_handler_patterns(
            &handler.patterns,
            &perform.args,
            &mut handler_frame,
            handler.span,
        ) {
            return Some(ControlSignal::Fault(Box::new(fault)));
        }
        let outer_continuation = perform.continuation;
        let signal = self.execute_block(handler.body, &mut handler_frame);
        Some(match signal {
            ControlSignal::Apply(mut pending) => {
                pending.continuation = compose_continuation(
                    pending.continuation,
                    Continuation::HandlerDispatch {
                        outer: Box::new(outer_continuation),
                    },
                );
                ControlSignal::Apply(pending)
            }
            ControlSignal::Checkpoint(pending) => ControlSignal::Checkpoint(pending),
            ControlSignal::Resume(value) => self.apply_continuation(outer_continuation, value),
            ControlSignal::Finish(value) => ControlSignal::Value(value),
            ControlSignal::Value(_) => ControlSignal::runtime_fault(
                "handler arm completed without resume, finish, or never control flow",
                handler.span,
            ),
            ControlSignal::Return(value) => ControlSignal::Return(value),
            ControlSignal::Break => ControlSignal::Break,
            ControlSignal::Fault(fault) => ControlSignal::Fault(fault),
            ControlSignal::Cancelled(cause) => ControlSignal::Cancelled(cause),
            ControlSignal::Continue => ControlSignal::Continue,
            ControlSignal::Block(mut nested) => {
                nested.continuation = compose_continuation(
                    nested.continuation,
                    Continuation::HandlerDispatch {
                        outer: Box::new(outer_continuation),
                    },
                );
                ControlSignal::Block(nested)
            }
            ControlSignal::Expr(mut nested) => {
                nested.continuation = compose_continuation(
                    nested.continuation,
                    Continuation::HandlerDispatch {
                        outer: Box::new(outer_continuation),
                    },
                );
                ControlSignal::Expr(nested)
            }
            ControlSignal::Call(mut nested) => {
                nested.continuation = compose_continuation(
                    nested.continuation,
                    Continuation::HandlerDispatch {
                        outer: Box::new(outer_continuation),
                    },
                );
                ControlSignal::Call(nested)
            }
            ControlSignal::Memory(mut nested) => {
                nested.continuation = compose_continuation(
                    nested.continuation,
                    Continuation::HandlerDispatch {
                        outer: Box::new(outer_continuation),
                    },
                );
                ControlSignal::Memory(nested)
            }
            ControlSignal::Session(mut nested) => {
                nested.continuation = compose_continuation(
                    nested.continuation,
                    Continuation::HandlerDispatch {
                        outer: Box::new(outer_continuation),
                    },
                );
                ControlSignal::Session(nested)
            }
            ControlSignal::Perform(mut nested) => {
                nested.continuation = compose_continuation(
                    nested.continuation,
                    Continuation::HandlerDispatch {
                        outer: Box::new(outer_continuation),
                    },
                );
                ControlSignal::Perform(nested)
            }
            ControlSignal::Console(mut nested) => {
                nested.continuation = compose_continuation(
                    nested.continuation,
                    Continuation::HandlerDispatch {
                        outer: Box::new(outer_continuation),
                    },
                );
                ControlSignal::Console(nested)
            }
            ControlSignal::Command(mut nested) => {
                nested.continuation = compose_continuation(
                    nested.continuation,
                    Continuation::HandlerDispatch {
                        outer: Box::new(outer_continuation),
                    },
                );
                ControlSignal::Command(nested)
            }
            ControlSignal::Model(mut nested) => {
                nested.continuation = compose_continuation(
                    nested.continuation,
                    Continuation::HandlerDispatch {
                        outer: Box::new(outer_continuation),
                    },
                );
                ControlSignal::Model(nested)
            }
            ControlSignal::Host(mut nested) => {
                nested.continuation = compose_continuation(
                    nested.continuation,
                    Continuation::HandlerDispatch {
                        outer: Box::new(outer_continuation),
                    },
                );
                ControlSignal::Host(nested)
            }
        })
    }

    pub(super) fn continue_handle_signal(
        &mut self,
        mut signal: ControlSignal,
        scope_id: HandlerScopeId,
        handlers: Vec<ActiveHandlerArmRecord>,
        span: Span,
        frame: &mut Frame,
    ) -> ControlSignal {
        loop {
            match signal {
                ControlSignal::Apply(pending) => {
                    return self.wrap_handler_boundary_signal(
                        ControlSignal::Apply(pending),
                        scope_id,
                        handlers,
                        span,
                        frame,
                    );
                }
                ControlSignal::Perform(perform) => {
                    signal = match self.dispatch_handler(*perform.clone(), &handlers, frame) {
                        Some(signal) => signal,
                        None => {
                            if let Err(fault) = self.close_handler_scope(scope_id, span) {
                                return ControlSignal::Fault(Box::new(fault));
                            }
                            return ControlSignal::Perform(perform);
                        }
                    };
                }
                ControlSignal::Block(block) => {
                    return self.wrap_handler_boundary_signal(
                        ControlSignal::Block(block),
                        scope_id,
                        handlers,
                        span,
                        frame,
                    );
                }
                ControlSignal::Expr(expr) => {
                    return self.wrap_handler_boundary_signal(
                        ControlSignal::Expr(expr),
                        scope_id,
                        handlers,
                        span,
                        frame,
                    );
                }
                ControlSignal::Call(call) => {
                    return self.wrap_handler_boundary_signal(
                        ControlSignal::Call(call),
                        scope_id,
                        handlers,
                        span,
                        frame,
                    );
                }
                ControlSignal::Memory(memory) => {
                    return self.wrap_handler_boundary_signal(
                        ControlSignal::Memory(memory),
                        scope_id,
                        handlers,
                        span,
                        frame,
                    );
                }
                ControlSignal::Session(session) => {
                    return self.wrap_handler_boundary_signal(
                        ControlSignal::Session(session),
                        scope_id,
                        handlers,
                        span,
                        frame,
                    );
                }
                ControlSignal::Console(console) => {
                    return self.wrap_handler_boundary_signal(
                        ControlSignal::Console(console),
                        scope_id,
                        handlers,
                        span,
                        frame,
                    );
                }
                ControlSignal::Command(command) => {
                    return self.wrap_handler_boundary_signal(
                        ControlSignal::Command(command),
                        scope_id,
                        handlers,
                        span,
                        frame,
                    );
                }
                ControlSignal::Model(model) => {
                    return self.wrap_handler_boundary_signal(
                        ControlSignal::Model(model),
                        scope_id,
                        handlers,
                        span,
                        frame,
                    );
                }
                ControlSignal::Host(host) => {
                    return self.wrap_handler_boundary_signal(
                        ControlSignal::Host(host),
                        scope_id,
                        handlers,
                        span,
                        frame,
                    );
                }
                other => {
                    if let Err(fault) = self.close_handler_scope(scope_id, span) {
                        return ControlSignal::Fault(Box::new(fault));
                    }
                    return other;
                }
            }
        }
    }

    fn wrap_handler_boundary_signal(
        &self,
        mut signal: ControlSignal,
        scope_id: HandlerScopeId,
        handlers: Vec<ActiveHandlerArmRecord>,
        span: Span,
        frame: &Frame,
    ) -> ControlSignal {
        let continuation = match &mut signal {
            ControlSignal::Apply(pending) => &mut pending.continuation,
            ControlSignal::Block(pending) => &mut pending.continuation,
            ControlSignal::Expr(pending) => &mut pending.continuation,
            ControlSignal::Call(pending) => &mut pending.continuation,
            ControlSignal::Memory(pending) => &mut pending.continuation,
            ControlSignal::Session(pending) => &mut pending.continuation,
            ControlSignal::Console(pending) => &mut pending.continuation,
            ControlSignal::Command(pending) => &mut pending.continuation,
            ControlSignal::Model(pending) => &mut pending.continuation,
            ControlSignal::Host(pending) => &mut pending.continuation,
            ControlSignal::Perform(pending) => &mut pending.continuation,
            _ => return signal,
        };
        match continuation.handler_scope_occurrences(scope_id) {
            0 => {
                let inner = std::mem::replace(continuation, Continuation::BlockValue);
                *continuation = Continuation::HandleBoundary {
                    scope_id,
                    inner: Box::new(inner),
                    handlers,
                    span,
                    frame: frame.clone(),
                };
                signal
            }
            1 => signal,
            count => ControlSignal::runtime_fault(
                format!(
                    "handler scope {} has {count} continuation boundary owners",
                    scope_id.0
                ),
                span,
            ),
        }
    }

    pub(crate) fn close_handler_scope(
        &mut self,
        scope_id: HandlerScopeId,
        span: Span,
    ) -> Result<(), ExecutionFault> {
        let Some(active) = self.handler_stack.last() else {
            return Err(ExecutionFault::new(
                AnalysisDiagnosticCode::UnhandledRuntimeError,
                span,
                format!(
                    "handler scope stack is empty while closing scope {}",
                    scope_id.0
                ),
            ));
        };
        if active.id != scope_id {
            return Err(ExecutionFault::new(
                AnalysisDiagnosticCode::UnhandledRuntimeError,
                span,
                format!(
                    "handler scope stack mismatch: attempted to close scope {}, but active scope is {}",
                    scope_id.0, active.id.0
                ),
            ));
        }
        self.handler_stack.pop();
        Ok(())
    }

    fn handler_matches_perform(
        &self,
        perform: &PendingPerform,
        handler: &ActiveHandlerArmRecord,
    ) -> bool {
        if is_error_raise_perform(perform) || is_error_raise_handler(handler) {
            return self.error_raise_matches_record(perform, handler);
        }
        action_matches_record(&perform.action, handler)
    }

    fn error_raise_matches_record(
        &self,
        perform: &PendingPerform,
        handler: &ActiveHandlerArmRecord,
    ) -> bool {
        if perform.error_type.is_none() {
            return false;
        }
        if perform.action.action != "raise"
            || perform
                .action
                .effect
                .path
                .segments
                .last()
                .is_none_or(|segment| segment.name != "Error")
        {
            return false;
        }
        if handler.action != "raise"
            || handler
                .effect_segments
                .last()
                .is_none_or(|segment| segment != "Error")
        {
            return false;
        }
        let [handler_error] = handler.effect_type_args.as_slice() else {
            return false;
        };
        let Some(perform_error) = perform.error_type else {
            return false;
        };
        type_ids_match(&self.checked.type_store, perform_error, *handler_error)
    }
}

fn handler_rows_compatible(
    coverage: &etas_effects::EffectCoverage<'_>,
    application: &etas_effects::EffectRow,
    value: &etas_effects::EffectRow,
) -> bool {
    if application.effects.is_empty() {
        return value.effects.is_empty();
    }
    !value.effects.is_empty()
        && (coverage.row_covers(application, value) || coverage.row_covers(value, application))
}

fn active_handler_arm(
    handler: &HirHandlerArm,
    fact: &etas_effects::HandlerArmFact,
    body: HirBlockId,
) -> ActiveHandlerArmRecord {
    ActiveHandlerArmRecord {
        effect_segments: handler
            .action
            .effect
            .path
            .segments
            .iter()
            .map(|segment| segment.name.clone())
            .collect(),
        action: handler.action.action.clone(),
        action_symbol: resolved_symbol(&handler.action.action_symbol),
        type_args: handler
            .generic_args
            .iter()
            .map(|arg| match arg {
                HirGenericArg::Type(ty) => *ty,
                HirGenericArg::EffectRow(_) => {
                    panic!(
                        "effect-row generic argument reached runtime handler record without checked instantiation facts"
                    )
                }
                HirGenericArg::Wildcard { .. } => {
                    panic!(
                        "wildcard generic argument reached runtime handler record without checked instantiation facts"
                    )
                }
            })
            .collect(),
        effect_type_args: fact.action.effect_type_args.clone(),
        patterns: handler.patterns.clone(),
        body,
        scope: handler.scope,
        span: handler.span,
    }
}

fn handler_arm_fact_matches_hir(
    handler: &HirHandlerArm,
    fact: &etas_effects::HandlerArmFact,
) -> bool {
    fact.action.action == handler.action.action
        && fact.action.action_symbol == resolved_symbol(&handler.action.action_symbol)
        && fact.action.effect_segments
            == handler
                .action
                .effect
                .path
                .segments
                .iter()
                .map(|segment| segment.name.clone())
                .collect::<Vec<_>>()
}

fn is_error_raise_perform(perform: &PendingPerform) -> bool {
    perform.action.action == "raise"
        && perform
            .action
            .effect
            .path
            .segments
            .last()
            .is_some_and(|segment| segment.name == "Error")
}

fn is_error_raise_handler(handler: &ActiveHandlerArmRecord) -> bool {
    handler.action == "raise"
        && handler
            .effect_segments
            .last()
            .is_some_and(|segment| segment == "Error")
}

fn action_matches_record(left: &ResolvedActionRef, right: &ActiveHandlerArmRecord) -> bool {
    action_symbols_match(&left.action_symbol, right.action_symbol)
}

fn action_symbols_match(left: &ResolveResult, right: Option<SymbolId>) -> bool {
    resolved_symbol(left)
        .is_some_and(|left_symbol| right.is_some_and(|right_symbol| left_symbol == right_symbol))
}

fn type_ids_match(
    store: &etas_types::TypeStore,
    left: etas_types::TypeId,
    right: etas_types::TypeId,
) -> bool {
    if left == right {
        return true;
    }
    match (store.get(left), store.get(right)) {
        (Some(left), Some(right)) => left == right,
        _ => false,
    }
}

fn resolved_symbol(result: &ResolveResult) -> Option<SymbolId> {
    match result {
        ResolveResult::Resolved(symbol) => Some(*symbol),
        _ => None,
    }
}

fn missing_handler_value_fault(span: Span) -> ExecutionFault {
    ExecutionFault::new(
        AnalysisDiagnosticCode::MissingCheckedFact,
        span,
        "handler value execution requires a checked HandlerValueFact",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handler_action_matching_fails_closed_without_resolved_symbol() {
        assert!(
            !action_symbols_match(&ResolveResult::Unresolved, None),
            "handler dispatch must not fall back to matching effect/action text"
        );
    }
}
