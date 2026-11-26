use super::{SnapshotValidator, call_target};
use crate::orchestration::{ArgumentDestinationSnapshot, DefaultArgumentsSnapshot};
use etas_hir::{HirArg, HirExpr};

impl SnapshotValidator<'_> {
    pub(crate) fn expression_site(
        &self,
        site: &crate::orchestration::CallSiteSnapshot,
        span: etas_core::Span,
        context: &str,
    ) -> Result<(), String> {
        self.frame(&site.frame, context)?;
        let crate::control::CallSiteKind::Expr { expr } = site.kind else {
            return Err("default expression has no source invocation expression".into());
        };
        self.expr(expr, context)?;
        if site.composition.is_some()
            || site.task_callback.is_some()
            || self.checked.hir.exprs[expr].span(&self.checked.hir.blocks) != span
        {
            return Err("default expression has inconsistent invocation origin".into());
        }
        Ok(())
    }
    pub(super) fn default_arguments(
        &self,
        state: &DefaultArgumentsSnapshot,
        context: &str,
    ) -> Result<(), String> {
        self.expr(state.expr, context)?;
        self.frame(&state.frame, context)?;
        if let Some(item) = state.tool {
            let Some(etas_hir::HirItem::Tool(tool)) = self.checked.hir.items.get(item) else {
                return Err("pending tool arguments have no checked tool owner".into());
            };
            let Some(etas_types::ItemSignature::Tool(signature)) =
                self.checked.types.item_signatures.get(&item)
            else {
                return Err("pending tool arguments have no checked signature".into());
            };
            if state.parameter_count != tool.params.len()
                || state.next_default == 0
                || state.next_default > state.defaults.len()
                || state.slots.len() != state.values.len()
            {
                return Err("pending tool arguments have inconsistent binding progress".into());
            }
            let mut slots = std::collections::BTreeSet::new();
            for (slot, value) in state.slots.iter().zip(&state.values) {
                let ty = *signature
                    .params
                    .get(*slot as usize)
                    .ok_or("pending tool argument exceeds its signature")?;
                if !slots.insert(*slot) {
                    return Err("pending tool argument is bound twice".into());
                }
                self.reference_value(
                    value,
                    ty,
                    &state
                        .frame
                        .bindings_at(self.checked, etas_hir::HirOwner::Expr(state.expr)),
                )?;
                self.snapshot_value_in(value, call_target::ReferenceScope::CheckedSlot)?;
            }
            for (index, default) in state.defaults.iter().enumerate() {
                let parameter = *tool
                    .params
                    .get(default.slot as usize)
                    .ok_or("pending tool default exceeds its signature")?;
                if default.source != (etas_types::ParameterDefaultRef::Source { parameter })
                    || !self.checked.hir.parameter_defaults.contains_key(&parameter)
                    || (index >= state.next_default - 1 && !slots.insert(default.slot))
                {
                    return Err("pending tool default differs from its checked declaration".into());
                }
            }
            if slots.len() != tool.params.len() {
                return Err("pending tool argument slots are incomplete".into());
            }
            return Ok(());
        }
        let site = &self.checked.hir.exprs[state.expr];
        let (count, mut slots, defaults) = if let Some(method) = self.dispatch.method(state.expr) {
            let mut slots = Vec::new();
            if let etas_types::method::MethodReceiver::Value { slot } = method.receiver {
                slots.push(slot);
            }
            slots.extend(&method.argument_slots);
            (method.signature.params.len(), slots, &method.defaults)
        } else {
            let binding = self
                .checked
                .types
                .declared_call_arguments
                .get(&state.expr)
                .ok_or("pending defaults have no checked argument binding")?;
            (
                binding.argument_slots.len() + binding.defaults.len(),
                binding.argument_slots.clone(),
                &binding.defaults,
            )
        };
        if state.defaults != *defaults
            || state.parameter_count != count
            || state.next_default == 0
            || state.next_default > defaults.len()
        {
            return Err(
                "pending defaults differ from their checked declaration or progress".into(),
            );
        }
        slots.extend(
            defaults[..state.next_default - 1]
                .iter()
                .map(|default| default.slot),
        );
        if slots != state.slots || state.values.len() != slots.len() {
            return Err("pending defaults have inconsistent bound argument positions".into());
        }
        let bindings = state
            .frame
            .bindings_at(self.checked, etas_hir::HirOwner::Expr(state.expr));
        let args = match (&state.destination, site) {
            (
                ArgumentDestinationSnapshot::Call { target },
                HirExpr::Call { args, .. }
                | HirExpr::MethodCall { args, .. }
                | HirExpr::SpecMethodCall { args, .. },
            ) => {
                let site = self.call_site(state.expr, args, state.span, context)?;
                match target {
                    Some(target) => {
                        self.call_target_in(
                            target,
                            context,
                            call_target::ReferenceScope::CheckedSlot,
                        )?;
                        self.pending_call_target(
                            state.expr,
                            target,
                            site,
                            &bindings,
                            state.values.first(),
                        )?;
                    }
                    None if self
                        .dispatch
                        .method(state.expr)
                        .is_some_and(|method| method.receiver.is_updating()) => {}
                    None => return Err("pending default call has no checked target".into()),
                }
                args
            }
            (
                ArgumentDestinationSnapshot::Perform {
                    action,
                    type_args,
                    source_args,
                },
                HirExpr::Perform {
                    action: expected,
                    args,
                    span,
                    ..
                },
            ) => {
                if action != expected
                    || state.span != *span
                    || !source_args
                        .iter()
                        .map(argument_key)
                        .eq(args.iter().map(argument_key))
                {
                    return Err("pending default action differs from checked HIR".into());
                }
                self.action(action, context)?;
                self.hir_types(type_args, context)?;
                args
            }
            _ => return Err("pending defaults have a different invocation destination".into()),
        };
        let receiver = match site {
            HirExpr::MethodCall { receiver, .. } | HirExpr::SpecMethodCall { receiver, .. }
                if self.dispatch.method(state.expr).is_some_and(|method| {
                    matches!(
                        method.receiver,
                        etas_types::method::MethodReceiver::Value { .. }
                    )
                }) =>
            {
                Some(*receiver)
            }
            _ => None,
        };
        let provided = receiver.into_iter().chain(args.iter().map(|arg| match arg {
            HirArg::Positional(expr) | HirArg::Named { value: expr, .. } => *expr,
        }));
        for (value, expr) in state.values.iter().zip(provided) {
            self.reference_expression(value, expr, &bindings)?;
            self.snapshot_value_in(value, call_target::ReferenceScope::CheckedSlot)?;
        }
        let provided_count = state.slots.len() - (state.next_default - 1);
        for (value, default) in state.values[provided_count..].iter().zip(defaults) {
            let parameter = match &default.source {
                etas_types::ParameterDefaultRef::Source { parameter } => *parameter,
                etas_types::ParameterDefaultRef::External { reference } => {
                    let ty = crate::eval::machine::expression_call::ExpressionCall::presented_parameter_type(
                        self.checked,
                        reference,
                        state.expr,
                        default.slot,
                        &bindings,
                    )?;
                    self.reference_value(value, ty, &crate::control::GenericBindings::new())?;
                    self.snapshot_value_in(value, call_target::ReferenceScope::CheckedSlot)?;
                    continue;
                }
            };
            let expr = self
                .checked
                .hir
                .parameter_defaults
                .get(&parameter)
                .ok_or("pending default has no authored expression")?;
            let default_bindings = crate::eval::arguments::default_argument_bindings(
                self.checked,
                state.expr,
                parameter,
                &bindings,
                state.span,
            )
            .map_err(|fault| fault.message)?;
            self.reference_expression(value, *expr, &default_bindings)?;
            self.snapshot_value_in(value, call_target::ReferenceScope::CheckedSlot)?;
        }
        Ok(())
    }
}

fn argument_key(argument: &HirArg) -> (Option<&str>, etas_hir::HirExprId, Option<etas_core::Span>) {
    match argument {
        HirArg::Positional(expr) => (None, *expr, None),
        HirArg::Named { name, value, span } => (Some(name.as_str()), *value, Some(*span)),
    }
}
