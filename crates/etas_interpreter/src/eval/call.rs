use super::*;

#[cfg(test)]
#[path = "call/tests.rs"]
mod tests;

pub(super) struct CallArgsState {
    pub call: HirExprId,
    pub target: CallTarget,
    pub args: std::sync::Arc<[HirArg]>,
    pub start_arg_index: usize,
    pub evaluated_args: Vec<InterpValue>,
    pub span: Span,
}

impl<'a> EvalContext<'a> {
    pub(super) fn eval_call(
        &mut self,
        call: HirExprId,
        callee: HirExprId,
        _type_args: &[etas_hir::HirTypeId],
        args: &[HirArg],
        span: Span,
        frame: &mut Frame,
    ) -> ControlSignal {
        let Some(descriptors) = self.plan.arguments.get(call) else {
            return ControlSignal::missing_checked_fact(
                "call is missing its checked argument descriptors",
                span,
            );
        };
        let nominal_target =
            match self.resolve_nominal_constructor_call_target(call, callee, frame, span) {
                Ok(target) => target,
                Err(fault) => return ControlSignal::Fault(Box::new(fault)),
            };
        let static_target =
            match self.resolve_static_call_target_for_call(call, callee, args, frame, span) {
                Ok(target) => target,
                Err(fault) => return ControlSignal::Fault(Box::new(fault)),
            };
        let member_receiver = match self.checked_agent_member_receiver(call, callee, span) {
            Ok(receiver) => receiver,
            Err(fault) => return ControlSignal::Fault(Box::new(fault)),
        };
        let selected_target = if let Some(receiver) = member_receiver {
            let receiver =
                match self.eval_path_signal(ResolveResult::Resolved(receiver), span, frame) {
                    ControlSignal::Value(value) => value,
                    other => return other,
                };
            match self.checked_agent_method_target(call, receiver, span) {
                Ok(target) => Some(target),
                Err(fault) => return ControlSignal::Fault(Box::new(fault)),
            }
        } else {
            None
        };
        let target = if let Some(target) = selected_target.or(nominal_target).or(static_target) {
            target
        } else {
            match self.eval_expr(callee, frame) {
                ControlSignal::Value(InterpValue::Callable(target)) => target,
                ControlSignal::Value(other) => {
                    let message = format!("callee is not a callable runtime value: {:?}", other);
                    return ControlSignal::invalid_arguments(message, span);
                }
                signal @ (ControlSignal::Apply(_)
                | ControlSignal::Checkpoint(_)
                | ControlSignal::Block(_)
                | ControlSignal::Expr(_)
                | ControlSignal::Call(_)
                | ControlSignal::Native(_)
                | ControlSignal::Perform(_)
                | ControlSignal::Memory(_)
                | ControlSignal::Session(_)
                | ControlSignal::Console(_)
                | ControlSignal::Command(_)
                | ControlSignal::Model(_)
                | ControlSignal::Host(_)) => {
                    return compose_signal_continuation(
                        signal,
                        Continuation::CalleeEval {
                            call,
                            args: descriptors,
                            span,
                            frame: frame.clone(),
                        },
                    );
                }
                ControlSignal::Return(value) => return ControlSignal::Return(value),
                ControlSignal::Resume(value) => return ControlSignal::Resume(value),
                ControlSignal::Finish(value) => return ControlSignal::Finish(value),
                ControlSignal::Break => return ControlSignal::Break,
                ControlSignal::Fault(fault) => return ControlSignal::Fault(fault),
                ControlSignal::Cancelled(cause) => return ControlSignal::Cancelled(cause),
                ControlSignal::Continue => return ControlSignal::Continue,
            }
        };
        let target = match self.specialize_call_target_for_call(call, target, frame, span) {
            Ok(target) => target,
            Err(fault) => return ControlSignal::Fault(Box::new(fault)),
        };
        self.resume_call_args(
            CallArgsState {
                call,
                target,
                args: descriptors,
                start_arg_index: 0,
                evaluated_args: Vec::new(),
                span,
            },
            frame,
        )
    }

    pub(super) fn resume_call_args(
        &mut self,
        state: CallArgsState,
        frame: &mut Frame,
    ) -> ControlSignal {
        let CallArgsState {
            call,
            target,
            args,
            start_arg_index,
            mut evaluated_args,
            span,
        } = state;
        evaluated_args.reserve(args.len().saturating_sub(evaluated_args.len()));
        for (index, arg) in args.iter().enumerate().skip(start_arg_index) {
            let expr = match arg {
                HirArg::Positional(value) | HirArg::Named { value, .. } => *value,
            };
            match self.eval_expr(expr, frame) {
                ControlSignal::Value(value) => evaluated_args.push(value),
                signal @ (ControlSignal::Apply(_)
                | ControlSignal::Checkpoint(_)
                | ControlSignal::Block(_)
                | ControlSignal::Expr(_)
                | ControlSignal::Call(_)
                | ControlSignal::Native(_)
                | ControlSignal::Perform(_)
                | ControlSignal::Memory(_)
                | ControlSignal::Session(_)
                | ControlSignal::Console(_)
                | ControlSignal::Command(_)
                | ControlSignal::Model(_)
                | ControlSignal::Host(_)) => {
                    return compose_signal_continuation(
                        signal,
                        Continuation::CallArgs {
                            call,
                            target,
                            args,
                            next_arg_index: index + 1,
                            evaluated_args,
                            span,
                            frame: frame.clone(),
                        },
                    );
                }
                ControlSignal::Return(value) => return ControlSignal::Return(value),
                ControlSignal::Resume(value) => return ControlSignal::Resume(value),
                ControlSignal::Finish(value) => return ControlSignal::Finish(value),
                ControlSignal::Break => return ControlSignal::Break,
                ControlSignal::Fault(fault) => return ControlSignal::Fault(fault),
                ControlSignal::Cancelled(cause) => return ControlSignal::Cancelled(cause),
                ControlSignal::Continue => return ControlSignal::Continue,
            }
        }
        if let Some(method) = self.plan.dispatch.method(call) {
            let mut source = evaluated_args.into_iter();
            let mut ordered = vec![None; method.signature.params.len()];
            if let etas_types::method::MethodReceiver::Value { slot } = method.receiver {
                ordered[slot as usize] = source.next();
            }
            for slot in &method.argument_slots {
                ordered[*slot as usize] = source.next();
            }
            evaluated_args = match ordered.into_iter().collect::<Option<Vec<_>>>() {
                Some(values) if source.next().is_none() => values,
                _ => {
                    return ControlSignal::missing_checked_fact(
                        "method argument progress differs from its checked ABI",
                        span,
                    );
                }
            };
            if matches!(
                method.target,
                etas_types::method::MethodTarget::BoundSpec { .. }
            ) && let etas_types::method::MethodReceiver::Value { slot } = method.receiver
            {
                let receiver = &mut evaluated_args[slot as usize];
                while let InterpValue::Existential(_) = receiver {
                    let InterpValue::Existential(object) =
                        std::mem::replace(receiver, InterpValue::Unit)
                    else {
                        unreachable!()
                    };
                    *receiver = object.value.into_value();
                }
            }
        }
        let site = Some(crate::control::CallSite {
            kind: crate::control::CallSiteKind::Expr { expr: call },
            frame: frame.clone(),
            composition: None,
        });
        self.execute_call_target_at(target, evaluated_args, span, site)
    }

    pub(super) fn execute_flow_call(
        &mut self,
        item: HirItemId,
        call_args: Vec<InterpValue>,
        span: Span,
    ) -> ControlSignal {
        match self.checked.hir.items.get(item) {
            Some(HirItem::Flow(flow)) => match self.execute_flow(item, flow, &call_args) {
                ControlSignal::Value(value) | ControlSignal::Return(value) => {
                    ControlSignal::Value(value)
                }
                signal @ (ControlSignal::Apply(_)
                | ControlSignal::Checkpoint(_)
                | ControlSignal::Block(_)
                | ControlSignal::Expr(_)
                | ControlSignal::Call(_)
                | ControlSignal::Native(_)
                | ControlSignal::Perform(_)
                | ControlSignal::Memory(_)
                | ControlSignal::Session(_)
                | ControlSignal::Console(_)
                | ControlSignal::Command(_)
                | ControlSignal::Model(_)
                | ControlSignal::Host(_)) => {
                    compose_signal_continuation(signal, Continuation::BlockValue)
                }
                ControlSignal::Resume(value) => ControlSignal::Resume(value),
                ControlSignal::Finish(value) => ControlSignal::Finish(value),
                ControlSignal::Break => ControlSignal::Break,
                ControlSignal::Fault(fault) => ControlSignal::Fault(fault),
                ControlSignal::Cancelled(cause) => ControlSignal::Cancelled(cause),
                ControlSignal::Continue => ControlSignal::Continue,
            },
            _ => ControlSignal::invalid_arguments(
                "callee HIR item is not an executable flow".to_owned(),
                span,
            ),
        }
    }

    fn execute_lambda_call(
        &mut self,
        expr: HirExprId,
        captured: Frame,
        call_args: Vec<InterpValue>,
        span: Span,
    ) -> ControlSignal {
        match self
            .checked
            .checked_callable_effects(etas_frontend::CheckedCallableRoot::Lambda(expr))
        {
            Ok(summary) if summary.residual_checks.is_empty() => {}
            Ok(_) => {
                return ControlSignal::missing_checked_fact(
                    "callable requires residual checks but no verified residual plan and installation are available",
                    span,
                );
            }
            Err(error) => return ControlSignal::missing_checked_fact(error.to_string(), span),
        }
        let Some(HirExpr::Lambda { params, body, .. }) = self.checked.hir.exprs.get(expr) else {
            return ControlSignal::missing_checked_fact(
                "lambda callable does not point at a lambda expression",
                span,
            );
        };
        let Some(layout) = self.plan.closures.get(expr) else {
            return ControlSignal::missing_checked_fact(
                "lambda call has no checked capture layout",
                span,
            );
        };
        let mut captured = match captured.capture(layout.slots.clone(), &layout.captures) {
            Ok(frame) => frame,
            Err(message) => return ControlSignal::missing_checked_fact(message, span),
        };
        if params.len() != call_args.len() {
            return ControlSignal::fault(
                AnalysisDiagnosticCode::InvalidArguments,
                span,
                format!(
                    "lambda expects {} argument(s), got {}",
                    params.len(),
                    call_args.len()
                ),
            );
        }
        for (symbol, arg) in params.iter().zip(call_args.into_iter()) {
            captured.insert(*symbol, arg);
        }
        match body {
            etas_hir::HirLambdaBody::Expr(expr) => match self.eval_expr(*expr, &mut captured) {
                ControlSignal::Return(value) => ControlSignal::Value(value),
                other => other,
            },
            etas_hir::HirLambdaBody::Block(block) => {
                match self.execute_block(*block, &mut captured) {
                    ControlSignal::Return(value) => ControlSignal::Value(value),
                    other => other,
                }
            }
        }
    }

    pub(super) fn execute_call_target(
        &mut self,
        target: CallTarget,
        call_args: Vec<InterpValue>,
        span: Span,
    ) -> ControlSignal {
        self.execute_call_target_at(target, call_args, span, None)
    }

    pub(super) fn execute_call_target_at(
        &mut self,
        target: CallTarget,
        call_args: Vec<InterpValue>,
        span: Span,
        site: Option<crate::control::CallSite>,
    ) -> ControlSignal {
        ControlSignal::pending_call(PendingCall {
            purity: crate::control::CallPurity::Ordinary,
            site,
            target,
            args: call_args,
            span,
            continuation: Continuation::BlockValue,
        })
    }

    pub(crate) fn execute_call_target_frame(
        &mut self,
        target: CallTarget,
        call_args: Vec<InterpValue>,
        span: Span,
        site: Option<crate::control::CallSite>,
    ) -> ControlSignal {
        match target {
            target @ (CallTarget::AgentRevision(_) | CallTarget::Scoped(_)) => {
                self.execute_call_target_at(target, call_args, span, site)
            }
            CallTarget::FlowItem(item) => self.execute_flow_call(item, call_args, span),
            CallTarget::AgentItem(item) => {
                self.execute_agent_call(item, call_args, span, Default::default())
            }
            CallTarget::ToolItem(item) => self.execute_source_tool_call(item, call_args, span),
            CallTarget::Lambda { expr, captured } => {
                self.execute_lambda_call(expr, captured, call_args, span)
            }
            CallTarget::EnumVariant(symbol) => {
                match self.eval_variant_constructor(
                    symbol,
                    call_args,
                    &crate::control::GenericBindings::new(),
                    span,
                ) {
                    Ok(value) => ControlSignal::Value(value),
                    Err(fault) => ControlSignal::Fault(Box::new(fault)),
                }
            }
            CallTarget::NominalConstructor(ty) => {
                match <Vec<InterpValue> as TryInto<[InterpValue; 1]>>::try_into(call_args) {
                    Ok([value]) => ControlSignal::Value(InterpValue::Nominal {
                        ty,
                        value: crate::value::SharedValue::new(value),
                    }),
                    Err(args) => ControlSignal::invalid_arguments(
                        format!(
                            "nominal constructor expects exactly one argument, got {}",
                            args.len()
                        ),
                        span,
                    ),
                }
            }
            CallTarget::PureIntrinsic(call) => {
                match crate::intrinsic::pure::PendingNativeWork::start(
                    &call,
                    &call_args,
                    self.plan.dispatch.pure_abi(),
                ) {
                    Ok(Some(work)) => ControlSignal::pending_native(work),
                    Ok(None) => {
                        let result = crate::intrinsic::pure::execute_pure_intrinsic(
                            &call,
                            call_args,
                            self.plan.dispatch.pure_abi(),
                        );
                        self.complete_pure_intrinsic(&call, result, span)
                    }
                    Err(error) => self.complete_pure_intrinsic(&call, Err(error), span),
                }
            }
            CallTarget::StdIntrinsic(call) => {
                match self.plan.dispatch.resolve_std_callable(call.identity) {
                    Ok(kind) => self.execute_std_callable(kind, &call, call_args, span),
                    Err(message) => ControlSignal::missing_checked_fact(message, span),
                }
            }
            CallTarget::Specialized {
                target,
                type_bindings,
                type_parameter_bindings,
                effect_row_bindings,
            } => self.execute_specialized_call_target(
                target.into_value(),
                crate::control::GenericBindings {
                    types: type_bindings.into_iter().collect(),
                    parameters: type_parameter_bindings.into_iter().collect(),
                    rows: effect_row_bindings.into_iter().collect(),
                },
                call_args,
                span,
                site,
            ),
            CallTarget::Limited { target, limits } => {
                self.execute_limited_call_target(target.into_value(), limits, call_args, span, site)
            }
            CallTarget::Composed(stages) => {
                self.execute_composed_call(stages, call_args, span, site)
            }
        }
    }

    pub(crate) fn complete_pure_intrinsic(
        &mut self,
        call: &crate::intrinsic::dispatch::CheckedPureIntrinsicCall,
        result: Result<InterpValue, crate::intrinsic::pure::AdapterError>,
        span: Span,
    ) -> ControlSignal {
        let error = match result {
            Ok(value) => return ControlSignal::Value(value),
            Err(error) => error,
        };
        if let Some(message) = crate::eval::std_call::builtin_abort_message(&error) {
            return ControlSignal::execution_aborted(message, span);
        }
        if let crate::intrinsic::pure::AdapterError::Builtin(etas_builtin::BuiltinError::Text(
            failure,
        )) = &error
        {
            if etas_std::intrinsic_completion_semantics(call.intrinsic)
                != Some(etas_std::IntrinsicCompletionSemantics::ReturnsOrRaisesDeclaredErrors)
            {
                return ControlSignal::missing_checked_fact(
                    "text intrinsic failure disagrees with its completion descriptor",
                    span,
                );
            }
            let Some(error_type) = self.checked.types.known_std_types.text_error else {
                return ControlSignal::missing_checked_fact(
                    "text intrinsic failure requires the checked std.text.TextError type",
                    span,
                );
            };
            return ControlSignal::pending_perform(PendingPerform::typed_error(
                error_type,
                InterpValue::Variant {
                    name: failure.variant_name().into(),
                    fields: Vec::new().into(),
                },
                span,
                Continuation::BlockValue,
            ));
        }
        ControlSignal::runtime_fault(
            format!(
                "checked pure builtin dispatch failed for {:?}: {error:?}",
                call.intrinsic
            ),
            span,
        )
    }

    fn execute_specialized_call_target(
        &mut self,
        target: CallTarget,
        bindings: crate::control::GenericBindings,
        call_args: Vec<InterpValue>,
        span: Span,
        site: Option<crate::control::CallSite>,
    ) -> ControlSignal {
        match target {
            CallTarget::EnumVariant(symbol) => {
                match self.eval_variant_constructor(symbol, call_args, &bindings, span) {
                    Ok(value) => ControlSignal::Value(value),
                    Err(fault) => ControlSignal::Fault(Box::new(fault)),
                }
            }
            CallTarget::AgentItem(item) => {
                self.execute_agent_call_with_generic_bindings(item, call_args, span, bindings)
            }
            CallTarget::ToolItem(item) => {
                self.execute_source_tool_call_with_generics(item, call_args, span, bindings)
            }
            CallTarget::AgentRevision(_) | CallTarget::Scoped(_) => {
                ControlSignal::missing_checked_fact(
                    "cross-program generic calls require an evaluation-machine boundary",
                    span,
                )
            }
            CallTarget::FlowItem(item) => match self.checked.hir.items.get(item) {
                Some(HirItem::Flow(flow)) => {
                    let signal =
                        self.execute_flow_with_generic_bindings(item, flow, &call_args, bindings);
                    match signal {
                        ControlSignal::Value(value) | ControlSignal::Return(value) => {
                            ControlSignal::Value(value)
                        }
                        other => other,
                    }
                }
                _ => ControlSignal::missing_checked_fact(
                    "specialized flow target is missing from checked HIR",
                    span,
                ),
            },
            CallTarget::PureIntrinsic(call) => {
                match call.specialize(&self.checked.type_store, &bindings) {
                    Ok(call) => self.execute_call_target_frame(
                        CallTarget::PureIntrinsic(call),
                        call_args,
                        span,
                        site,
                    ),
                    Err(message) => ControlSignal::missing_checked_fact(message, span),
                }
            }
            CallTarget::StdIntrinsic(call) => {
                match self.plan.dispatch.resolve_std_callable(call.identity) {
                    Ok(kind) => self.execute_std_callable_with_generic_bindings(
                        kind, &call, call_args, span, &bindings,
                    ),
                    Err(message) => ControlSignal::missing_checked_fact(message, span),
                }
            }
            other => self.execute_call_target_frame(other, call_args, span, site),
        }
    }

    fn execute_composed_call(
        &mut self,
        stages: crate::control::CallTargetChildren,
        call_args: Vec<InterpValue>,
        span: Span,
        mut site: Option<crate::control::CallSite>,
    ) -> ControlSignal {
        let [first_arg]: [InterpValue; 1] = match call_args.try_into() {
            Ok(args) => args,
            Err(args) => {
                return ControlSignal::invalid_arguments(
                    format!(
                        "composed callable expects exactly one argument, got {}",
                        args.len()
                    ),
                    span,
                );
            }
        };
        if let Some(site) = &mut site {
            if let crate::control::CallSiteKind::Pipeline { expr } = site.kind {
                site.kind = crate::control::CallSiteKind::PipelineStage { expr, index: 0 };
            } else if let Some(composition) = &mut site.composition {
                composition.path = composition.path.push(0);
            } else {
                site.composition = Some(crate::control::ComposedInvocation {
                    root: CallTarget::Composed(stages.clone()).into(),
                    path: crate::control::InvocationPath::default().push(0),
                });
            }
        }
        self.resume_composed_call(first_arg, stages, 0, span, site)
    }

    pub(super) fn resume_composed_call(
        &mut self,
        input: InterpValue,
        stages: crate::control::CallTargetChildren,
        next_index: usize,
        span: Span,
        site: Option<crate::control::CallSite>,
    ) -> ControlSignal {
        if next_index == stages.len() {
            return ControlSignal::Value(input);
        }
        let Some(stage) = stages.get(next_index).cloned() else {
            return ControlSignal::missing_checked_fact(
                "composition cursor is out of bounds",
                span,
            );
        };
        let mut next_site = site.clone();
        if let Some(site) = &mut next_site {
            if let Some(composition) = &mut site.composition {
                let Some(next) = composition.path.advance() else {
                    return ControlSignal::missing_checked_fact(
                        "composition stage index overflow",
                        span,
                    );
                };
                composition.path = next;
            } else if let crate::control::CallSiteKind::PipelineStage { index, .. } = &mut site.kind
            {
                let Some(next) = index.checked_add(1) else {
                    return ControlSignal::missing_checked_fact(
                        "pipeline stage index overflow",
                        span,
                    );
                };
                *index = next;
            }
        }
        compose_signal_continuation(
            self.execute_call_target_at(stage, vec![input], span, site),
            Continuation::ComposedCall {
                remaining: stages,
                next_index: next_index + 1,
                site: next_site,
                span,
            },
        )
    }

    pub(crate) fn prepare_call_target(
        &mut self,
        mut target: CallTarget,
        mut continuation: Continuation,
    ) -> Result<(CallTarget, Continuation), Box<ControlSignal>> {
        let mut policy = self.model_policy.clone();
        let mut has_limits = false;
        while let CallTarget::Limited {
            target: inner,
            limits,
        } = target
        {
            for limit in &limits {
                if let Err(fault) = self.apply_runtime_limit_to_model_policy(limit, &mut policy) {
                    return Err(Box::new(ControlSignal::Fault(Box::new(fault))));
                }
            }
            has_limits = true;
            target = inner.into_value();
        }
        if has_limits {
            let previous = std::mem::replace(&mut self.model_policy, policy);
            continuation = Continuation::RestoreModelPolicy {
                previous: Box::new(previous),
                inner: continuation.into(),
            };
        }
        Ok((target, continuation))
    }

    fn execute_limited_call_target(
        &mut self,
        target: CallTarget,
        limits: Vec<crate::eval::limit::RuntimeLimit>,
        call_args: Vec<InterpValue>,
        span: Span,
        site: Option<crate::control::CallSite>,
    ) -> ControlSignal {
        let mut policy = self.model_policy.clone();
        for limit in &limits {
            if let Err(fault) = self.apply_runtime_limit_to_model_policy(limit, &mut policy) {
                return ControlSignal::Fault(Box::new(fault));
            }
        }
        let previous = std::mem::replace(&mut self.model_policy, policy.clone());
        let signal = self.execute_call_target_at(target, call_args, span, site);
        self.model_policy = previous;
        self.scope_model_policy_signal(signal, policy)
    }
}
