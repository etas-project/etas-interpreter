use super::*;

impl EvalContext<'_> {
    pub(in crate::eval) fn checked_agent_member_receiver(
        &self,
        call: HirExprId,
        callee: HirExprId,
        span: Span,
    ) -> Result<Option<etas_hir::SymbolId>, crate::control::ExecutionFault> {
        let Some(method) = self.checked.types.agent_method_calls.get(&call) else {
            return Ok(None);
        };
        let HirExpr::Path(path) = &self.checked.hir.exprs[callee] else {
            return Ok(None);
        };
        let ResolveResult::PartiallyResolved(partial) = &path.resolution else {
            return Ok(None);
        };
        let missing = |reason| {
            crate::control::ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                span,
                reason,
            )
        };
        let receiver = partial
            .resolved_prefix
            .ok_or_else(|| missing("agent member path has no resolved receiver"))?;
        let selected = self
            .checked
            .types
            .agent_methods
            .get(method)
            .ok_or_else(|| missing("agent member path has no checked method signature"))?;
        let receiver_type = self
            .checked
            .types
            .symbol_types
            .get(&receiver)
            .ok_or_else(|| missing("agent member path has no checked receiver type"))?;
        let component = match receiver_type {
            etas_types::SymbolTypeFact::Agent { signature } => Some(signature.component),
            etas_types::SymbolTypeFact::Param { ty }
            | etas_types::SymbolTypeFact::Local { ty, .. }
            | etas_types::SymbolTypeFact::Value { ty }
            | etas_types::SymbolTypeFact::TopLevelLet { ty, .. } => {
                if *ty == selected.component {
                    Some(*ty)
                } else if matches!(
                    self.checked.type_store.get(*ty),
                    Some(etas_types::Type::Nominal(_) | etas_types::Type::Applied { .. })
                ) {
                    self.checked
                        .types
                        .agent_signature(*ty)
                        .map(|agent| agent.component)
                } else {
                    None
                }
            }
            _ => None,
        };
        // A record field may itself contain an Agent. In that case the whole
        // callee is a value projection, not a method on the path's root symbol.
        let Some(component) = component else {
            return Ok(None);
        };
        if component != selected.component {
            return Err(missing(
                "agent member path disagrees with its checked receiver identity",
            ));
        }
        Ok(Some(receiver))
    }

    pub(in crate::eval) fn checked_agent_method_target(
        &self,
        call: HirExprId,
        receiver: InterpValue,
        span: Span,
    ) -> Result<CallTarget, ExecutionFault> {
        let missing = |message| {
            ExecutionFault::new(AnalysisDiagnosticCode::MissingCheckedFact, span, message)
        };
        let symbol = self
            .checked
            .types
            .agent_method_calls
            .get(&call)
            .ok_or_else(|| missing("agent method call is missing its checked selection"))?;
        let selected = self
            .checked
            .types
            .agent_methods
            .get(symbol)
            .ok_or_else(|| missing("agent method call has no checked method signature"))?;
        if let InterpValue::Callable(CallTarget::AgentRevision(revision)) = receiver {
            return revision
                .select_method(self.checked, selected.agent, selected.body)
                .map(CallTarget::AgentRevision)
                .map_err(|reason| {
                    ExecutionFault::new(AnalysisDiagnosticCode::MissingCheckedFact, span, reason)
                });
        }
        let InterpValue::Callable(CallTarget::AgentItem(agent)) = receiver else {
            return Err(missing(
                "agent method receiver is not a checked agent value",
            ));
        };
        if agent != selected.agent {
            return Err(missing(
                "agent method selection disagrees with its receiver identity",
            ));
        }
        match self.checked.hir.items.get(selected.body) {
            Some(HirItem::Agent(_)) if selected.body == agent => Ok(CallTarget::AgentItem(agent)),
            Some(HirItem::Flow(_)) => self.source_flow_call_target(selected.body, span),
            _ => Err(missing(
                "agent method body is not a canonical callable item",
            )),
        }
    }
}
