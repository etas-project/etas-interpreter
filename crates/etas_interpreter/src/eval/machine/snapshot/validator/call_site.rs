use super::*;
use etas_hir::HirExpr;

impl SnapshotValidator<'_> {
    pub(super) fn invocation_site(
        &self,
        site: &crate::orchestration::CallSiteSnapshot,
        span: etas_core::Span,
        context: &str,
    ) -> Result<(), String> {
        self.frame(&site.frame, context)?;
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
            let bindings = site.frame.generic_bindings();
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
        self.call_target_in(target, context, call_target::ReferenceScope::CheckedSlot)?;
        let site = self.call_site(*call, args, *span, context)?;
        let bindings = frame.generic_bindings();
        self.pending_call_target(*call, target, site, &bindings)?;
        self.index_at_most(*next_arg_index, args.len(), context)?;
        let prefix = next_arg_index
            .checked_sub(1)
            .ok_or_else(|| format!("{context}: call arguments have no pending expression"))?;
        let receiver = match site {
            HirExpr::SpecMethodCall { receiver, .. } => Some(*receiver),
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
