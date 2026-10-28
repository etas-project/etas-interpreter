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
        type_args: &[etas_hir::HirTypeId],
        args: &[HirArg],
        span: Span,
        frame: &mut Frame,
    ) -> ControlSignal {
        if let Some(signal) =
            self.eval_checked_field_method_call(callee, type_args, args, span, frame)
        {
            return signal;
        }
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

    fn eval_checked_field_method_call(
        &mut self,
        callee: HirExprId,
        type_args: &[etas_hir::HirTypeId],
        args: &[HirArg],
        span: Span,
        frame: &mut Frame,
    ) -> Option<ControlSignal> {
        let HirExpr::Field { base, field, .. } = &self.checked.hir.exprs[callee] else {
            return self.eval_checked_partial_method_call(callee, type_args, args, span, frame);
        };
        if field != "cast" || type_args.is_empty() {
            return None;
        }
        Some(self.eval_method_call(
            crate::eval::method::MethodDispatch {
                expr: callee,
                method: field,
                type_args,
                args,
                span,
            },
            *base,
            frame,
        ))
    }

    fn eval_checked_partial_method_call(
        &mut self,
        callee: HirExprId,
        type_args: &[etas_hir::HirTypeId],
        args: &[HirArg],
        span: Span,
        frame: &mut Frame,
    ) -> Option<ControlSignal> {
        let HirExpr::Path(path) = &self.checked.hir.exprs[callee] else {
            return None;
        };
        let ResolveResult::PartiallyResolved(partial) = &path.resolution else {
            return None;
        };
        if partial.reason != PartialResolutionReason::MemberRequiresTypeChecking {
            return None;
        }
        let [method] = partial.remaining.as_slice() else {
            return None;
        };
        if method != "cast" || type_args.is_empty() {
            return None;
        }
        let receiver_symbol = partial.resolved_prefix?;
        let receiver = frame.get(receiver_symbol)?;
        Some(self.resume_local_method_args(
            crate::eval::method::LocalMethodArgsState {
                expr: callee,
                receiver,
                method: method.to_owned(),
                type_args: type_args.to_vec(),
                args: args.to_vec(),
                start_arg_index: 0,
                evaluated_args: Vec::new(),
                span,
            },
            frame,
        ))
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
        let site = matches!(
            target,
            CallTarget::AgentRevision(_)
                | CallTarget::Scoped(_)
                | CallTarget::Composed(_)
                | CallTarget::Limited { .. }
                | CallTarget::Specialized { .. }
        )
        .then(|| crate::control::CallSite {
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
            CallTarget::SpecImplMethod(symbol) => {
                self.execute_spec_impl_method_call(symbol, call_args, span)
            }
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
                match crate::intrinsic::pure::execute_pure_intrinsic(
                    &call,
                    call_args,
                    self.plan.dispatch.pure_abi(),
                ) {
                    Ok(value) => ControlSignal::Value(value),
                    Err(error) => {
                        if let Some(message) = crate::eval::std_call::builtin_abort_message(&error)
                        {
                            return ControlSignal::execution_aborted(message, span);
                        }
                        ControlSignal::runtime_fault(
                            format!(
                                "checked pure builtin dispatch failed for {:?}: {error:?}",
                                call.intrinsic
                            ),
                            span,
                        )
                    }
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
                effect_row_bindings,
            } => self.execute_specialized_call_target(
                target.into_value(),
                type_bindings,
                effect_row_bindings,
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

    fn execute_specialized_call_target(
        &mut self,
        target: CallTarget,
        type_bindings: Vec<(String, etas_types::TypeId)>,
        effect_row_bindings: Vec<(String, etas_types::EffectRowRef)>,
        call_args: Vec<InterpValue>,
        span: Span,
        site: Option<crate::control::CallSite>,
    ) -> ControlSignal {
        let bindings = type_bindings
            .into_iter()
            .collect::<std::collections::HashMap<_, _>>();
        let rows = effect_row_bindings.into_iter().collect();
        match target {
            CallTarget::EnumVariant(symbol) => match self.eval_variant_constructor(
                symbol,
                call_args,
                &crate::control::GenericBindings {
                    types: bindings,
                    rows,
                },
                span,
            ) {
                Ok(value) => ControlSignal::Value(value),
                Err(fault) => ControlSignal::Fault(Box::new(fault)),
            },
            CallTarget::AgentItem(item) => self.execute_agent_call_with_generic_bindings(
                item,
                call_args,
                span,
                crate::control::GenericBindings {
                    types: bindings,
                    rows,
                },
            ),
            CallTarget::ToolItem(item) => self.execute_source_tool_call_with_generics(
                item,
                call_args,
                span,
                crate::control::GenericBindings {
                    types: bindings,
                    rows,
                },
            ),
            CallTarget::AgentRevision(_) | CallTarget::Scoped(_) => {
                ControlSignal::missing_checked_fact(
                    "cross-program generic calls require an evaluation-machine boundary",
                    span,
                )
            }
            CallTarget::FlowItem(item) => match self.checked.hir.items.get(item) {
                Some(HirItem::Flow(flow)) => {
                    let signal = self.execute_flow_with_generic_bindings(
                        item,
                        flow,
                        &call_args,
                        crate::control::GenericBindings {
                            types: bindings,
                            rows,
                        },
                    );
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
                match call.specialize(
                    &self.checked.type_store,
                    &crate::control::GenericBindings {
                        types: bindings,
                        rows,
                    },
                ) {
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
                    Ok(kind) => self.execute_std_callable_with_type_bindings(
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
