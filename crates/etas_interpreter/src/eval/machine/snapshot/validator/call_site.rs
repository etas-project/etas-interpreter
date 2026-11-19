use super::*;
use etas_hir::HirExpr;

impl SnapshotValidator<'_> {
    pub(super) fn pending_execution_call(
        &self,
        site: Option<&crate::orchestration::CallSiteSnapshot>,
        target: &CallTargetSnapshot,
        receiver: Option<&ValueSnapshot>,
        span: etas_core::Span,
    ) -> Result<(), String> {
        let context = "checkpoint pending call";
        let Some(site) = site else {
            return self.call_target(target, context);
        };
        self.invocation_site(site, span, context)?;
        self.call_target_in(target, context, call_target::ReferenceScope::CheckedSlot)?;
        let bindings = site.bindings(self.checked);
        if let Some(composition) = &site.composition {
            if composition.selected()? != target {
                return Err("pending call differs from its checked composition stage".into());
            }
            return Ok(());
        }
        use crate::control::CallSiteKind;
        match site.kind {
            CallSiteKind::TaskCallback { .. } | CallSiteKind::SourceTaskCallback { .. } => self
                .reference_target_type_with_bindings(
                    target,
                    site.task_callback_type(self.checked)?,
                    &bindings,
                ),
            CallSiteKind::Expr { expr } => self.pending_call_target(
                expr,
                target,
                &self.checked.hir.exprs[expr],
                &bindings,
                receiver,
            ),
            CallSiteKind::PipelineStage { .. } => {
                let (callee, _) = site.receiver(self.checked, span)?;
                self.reference_target_type_with_bindings(
                    target,
                    self.checked.types.expr_types[&callee],
                    &bindings,
                )
            }
            CallSiteKind::Pipeline { .. } => {
                Err("pending call has no selected pipeline stage".into())
            }
        }
    }

    pub(super) fn pending_method_target(
        &self,
        call: HirExprId,
        mut target: &CallTargetSnapshot,
        selected: &etas_types::method::MethodTarget,
        bindings: &crate::control::GenericBindings,
    ) -> Result<(), String> {
        while let CallTargetSnapshot::Limited { target: inner, .. }
        | CallTargetSnapshot::Specialized { target: inner, .. } = target
        {
            target = inner;
        }
        use etas_types::method::MethodTarget;
        let matches = match (selected, target) {
            (MethodTarget::Source { callable, .. }, CallTargetSnapshot::FlowItem(item)) => {
                callable == item
            }
            (
                MethodTarget::Standard { .. },
                CallTargetSnapshot::StdIntrinsic {
                    origin,
                    intrinsic,
                    dispatch,
                    ..
                },
            ) => self.dispatch.std_method(call).is_some_and(|expected| {
                *origin == call
                    && *intrinsic == expected.identity.intrinsic
                    && *dispatch == expected.identity.dispatch
            }),
            (
                MethodTarget::Standard { .. },
                CallTargetSnapshot::PureIntrinsic {
                    intrinsic,
                    parameter_types,
                    result_type,
                },
            ) => {
                let expected = self
                    .dispatch
                    .std_method(call)
                    .ok_or("checkpoint method has no prepared standard ABI")?;
                let expected = crate::intrinsic::dispatch::CheckedPureIntrinsicCall {
                    intrinsic: expected.identity.intrinsic,
                    parameter_types: expected.parameter_types.clone(),
                    result_type: expected.result_type,
                }
                .specialize(&self.checked.type_store, bindings)?;
                *intrinsic == expected.intrinsic
                    && *parameter_types == expected.parameter_types
                    && *result_type == expected.result_type
            }
            (MethodTarget::Agent { .. }, CallTargetSnapshot::AgentRevision(_)) => true,
            _ => false,
        };
        if !matches {
            return Err(
                "checkpoint method target differs from its prepared checked selection".into(),
            );
        }
        Ok(())
    }

    pub(super) fn call_origin(
        &self,
        origin: &crate::control::CallOrigin,
        span: etas_core::Span,
        context: &str,
    ) -> Result<(), String> {
        use crate::control::CallSiteKind;
        if let CallSiteKind::SourceTaskCallback { item } = origin.kind {
            self.item(item, context)?;
            return if matches!(
                self.checked.hir.items.get(item),
                Some(etas_hir::HirItem::Flow(etas_hir::HirFlowDecl {
                    body: etas_hir::HirFlowBody::Runtime {
                        role: etas_std::StdExecutionRole::TaskStart,
                        ..
                    },
                    ..
                }))
            ) {
                Ok(())
            } else {
                Err("source task callback origin is not a checked task entry".into())
            };
        }
        let (CallSiteKind::Expr { expr }
        | CallSiteKind::TaskCallback { expr }
        | CallSiteKind::Pipeline { expr }
        | CallSiteKind::PipelineStage { expr, .. }) = origin.kind
        else {
            unreachable!()
        };
        self.expr(expr, context)?;
        let source = &self.checked.hir.exprs[expr];
        if matches!(origin.kind, CallSiteKind::TaskCallback { .. }) {
            return self.dispatch.task_callback_origin(expr).map(|_| ());
        }
        let valid = match (origin.kind, source) {
            (
                CallSiteKind::Expr { .. },
                HirExpr::Call { .. } | HirExpr::MethodCall { .. } | HirExpr::SpecMethodCall { .. },
            ) => true,
            (CallSiteKind::Pipeline { .. }, HirExpr::Pipeline { .. }) => true,
            (CallSiteKind::PipelineStage { index, .. }, HirExpr::Pipeline { stages, .. }) => {
                index < stages.len()
            }
            _ => false,
        };
        if !valid || source.span(&self.checked.hir.blocks) != span {
            return Err(format!(
                "{context} caller does not match its checked call location"
            ));
        }
        Ok(())
    }

    pub(super) fn invocation_site(
        &self,
        site: &crate::orchestration::CallSiteSnapshot,
        span: etas_core::Span,
        context: &str,
    ) -> Result<(), String> {
        self.frame(&site.frame, context)?;
        if matches!(
            site.kind,
            crate::control::CallSiteKind::SourceTaskCallback { .. }
        ) {
            site.task_callback_type(self.checked)?;
            return Ok(());
        }
        let (receiver, _) = site.receiver(self.checked, span)?;
        if let Some(composition) = &site.composition {
            self.call_target_in(
                &composition.root,
                context,
                call_target::ReferenceScope::CheckedSlot,
            )?;
            let ty = *self
                .checked
                .types
                .expr_types
                .get(&receiver)
                .ok_or("checkpoint composed invocation has no checked receiver type")?;
            let bindings = site.bindings(self.checked);
            self.reference_target_type_with_bindings(&composition.root, ty, &bindings)?;
            composition.stages_and_index()?;
        }
        Ok(())
    }

    pub(super) fn pending_direct_intrinsic(
        &self,
        call: HirExprId,
        mut target: &CallTargetSnapshot,
        site: &HirExpr,
        bindings: &crate::control::GenericBindings,
    ) -> Result<bool, String> {
        let HirExpr::Call { callee, args, .. } = site else {
            return Ok(false);
        };
        let Some(HirExpr::Path(path)) = self.checked.hir.exprs.get(*callee) else {
            return Ok(false);
        };
        let ResolveResult::Resolved(symbol) = path.resolution else {
            return Ok(false);
        };
        let Some(identity) = self.dispatch.std_intrinsic(symbol) else {
            return Ok(false);
        };
        // A direct invocation is checked at its call site, rather than using
        // the different ABI of a first-class std function reference.
        while let CallTargetSnapshot::Limited { target: inner, .. }
        | CallTargetSnapshot::Specialized { target: inner, .. } = target
        {
            target = inner;
        }
        match target {
            CallTargetSnapshot::PureIntrinsic {
                intrinsic,
                parameter_types,
                result_type,
            } if identity.dispatch == etas_std::IntrinsicDispatch::PureKernel
                && identity.intrinsic == *intrinsic =>
            {
                let expected_parameters = args
                    .iter()
                    .map(|arg| {
                        let (HirArg::Positional(expr) | HirArg::Named { value: expr, .. }) = arg;
                        self.checked
                            .types
                            .expr_types
                            .get(expr)
                            .copied()
                            .ok_or("checkpoint pure intrinsic argument has no checked type")
                            .map_err(str::to_owned)
                    })
                    .collect::<Result<Vec<_>, String>>()?;
                let expected_result = *self
                    .checked
                    .types
                    .expr_types
                    .get(&call)
                    .ok_or("checkpoint pure intrinsic call has no checked result type")?;
                let expected = crate::intrinsic::dispatch::CheckedPureIntrinsicCall {
                    intrinsic: *intrinsic,
                    parameter_types: expected_parameters,
                    result_type: expected_result,
                }
                .specialize(&self.checked.type_store, bindings)?;
                if parameter_types != &expected.parameter_types
                    || *result_type != expected.result_type
                {
                    return Err(
                        "checkpoint pure intrinsic ABI differs from its checked call site".into(),
                    );
                }
            }
            CallTargetSnapshot::StdIntrinsic {
                origin,
                intrinsic,
                dispatch,
                ..
            } if *origin == call
                && *intrinsic == identity.intrinsic
                && *dispatch == identity.dispatch => {}
            _ => {
                return Err(
                    "checkpoint intrinsic target differs from its checked call selection".into(),
                );
            }
        }
        Ok(true)
    }

    pub(super) fn call_site<'a>(
        &'a self,
        call: HirExprId,
        args: &[HirArg],
        span: etas_core::Span,
        context: &str,
    ) -> Result<&'a HirExpr, String> {
        self.args(args, context)?;
        let site = self
            .checked
            .hir
            .exprs
            .get(call)
            .ok_or_else(|| format!("{context}: missing checked call site"))?;
        let (expected, expected_span) = match site {
            HirExpr::Call { args, span, .. }
            | HirExpr::MethodCall { args, span, .. }
            | HirExpr::SpecMethodCall { args, span, .. } => (args, *span),
            _ => {
                return Err(format!(
                    "{context}: continuation does not reference a checked call site"
                ));
            }
        };
        if args.len() != expected.len()
            || !args
                .iter()
                .zip(expected)
                .all(|(actual, expected)| match (actual, expected) {
                    (HirArg::Positional(a), HirArg::Positional(b)) => a == b,
                    (
                        HirArg::Named {
                            name: a,
                            value: av,
                            span: asp,
                        },
                        HirArg::Named {
                            name: b,
                            value: bv,
                            span: bsp,
                        },
                    ) => a == b && av == bv && asp == bsp,
                    _ => false,
                })
            || span != expected_span
        {
            return Err(format!(
                "{context}: pending arguments disagree with their checked call site"
            ));
        }
        Ok(site)
    }

    pub(super) fn pending_call_arguments(
        &self,
        continuation: &ContinuationSnapshot,
        context: &str,
    ) -> Result<(), String> {
        let ContinuationSnapshot::CallArgs {
            call,
            target,
            args,
            next_arg_index,
            evaluated_args,
            span,
            frame,
        } = continuation
        else {
            return Err(format!("{context}: expected a pending call"));
        };
        let site = self.call_site(*call, args, *span, context)?;
        let bindings = frame.bindings_at(self.checked, etas_hir::HirOwner::Expr(*call));
        match target {
            Some(target) => {
                if self
                    .dispatch
                    .method(*call)
                    .is_some_and(|method| method.receiver.is_updating())
                {
                    return Err(
                        "updating call must acquire its receiver after ordinary arguments".into(),
                    );
                }
                self.call_target_in(target, context, call_target::ReferenceScope::CheckedSlot)?;
                self.pending_call_target(*call, target, site, &bindings, evaluated_args.first())?;
            }
            None => {
                if !self
                    .dispatch
                    .method(*call)
                    .is_some_and(|method| method.receiver.is_updating())
                {
                    return Err(
                        "only updating calls may defer their target until after arguments".into(),
                    );
                }
                etas_types::method::updating_receiver_symbol(
                    &self.checked.hir,
                    &self.checked.types,
                    *call,
                )?;
            }
        }
        self.index_at_most(*next_arg_index, args.len(), context)?;
        let prefix = next_arg_index
            .checked_sub(1)
            .ok_or_else(|| format!("{context}: call arguments have no pending expression"))?;
        let receiver = match site {
            HirExpr::MethodCall { receiver, .. } | HirExpr::SpecMethodCall { receiver, .. }
                if self.dispatch.method(*call).is_some_and(|fact| {
                    matches!(
                        fact.receiver,
                        etas_types::method::MethodReceiver::Value { .. }
                    )
                }) =>
            {
                Some(*receiver)
            }
            _ => None,
        };
        if evaluated_args.len() != prefix + usize::from(receiver.is_some()) {
            return Err(format!(
                "{context}: inconsistent evaluated call argument prefix"
            ));
        }
        for (value, expr) in evaluated_args
            .iter()
            .zip(receiver.into_iter().chain(args.iter().map(|arg| match arg {
                HirArg::Positional(value) | HirArg::Named { value, .. } => *value,
            })))
        {
            self.reference_expression(value, expr, &bindings)?;
            self.snapshot_value_in(value, call_target::ReferenceScope::CheckedSlot)?;
        }
        self.frame(frame, context)
    }
}
