use super::*;
use crate::control::ExecutionFault;

mod forwarder;
mod native;

#[cfg(test)]
#[path = "perform/tests.rs"]
mod tests;

impl<'a> EvalContext<'a> {
    pub(super) fn resume_perform_args(
        &mut self,
        resume: PerformArgsResume,
        frame: &mut Frame,
    ) -> ControlSignal {
        let PerformArgsResume {
            expr,
            action,
            type_args,
            args,
            start_arg_index,
            mut evaluated_args,
            span,
        } = resume;
        evaluated_args.reserve(args.len().saturating_sub(evaluated_args.len()));
        for (index, arg) in args.iter().enumerate().skip(start_arg_index) {
            let expr_id = match arg {
                HirArg::Positional(value) | HirArg::Named { value, .. } => *value,
            };
            match self.eval_expr(expr_id, frame) {
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
                        Continuation::PerformArgs {
                            expr,
                            action,
                            type_args,
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
        if !self.performed_action_fact_matches(expr, &action, &args) {
            return ControlSignal::missing_checked_fact(
                "performed action requires a matching checked action fact",
                span,
            );
        }
        let error_type = match self.error_raise_type_arg(expr, &action, &type_args, frame) {
            Ok(ty) => ty,
            Err(fault) => return ControlSignal::Fault(Box::new(fault)),
        };
        let mut pending = PendingPerform {
            checked_action: None,
            default_boundary: None,
            forwarded_call: None,
            inference: None,
            expr: Some(expr),
            error_type,
            action,
            args: evaluated_args,
            span,
            continuation: Continuation::BlockValue,
        };
        if error_type.is_none() {
            match self.action_invocation(&pending, expr, frame) {
                Ok(proof) => pending.checked_action = Some(proof),
                Err(reason) => return ControlSignal::missing_checked_fact(reason, span),
            }
        }
        ControlSignal::pending_perform(pending)
    }

    fn action_invocation(
        &self,
        pending: &PendingPerform,
        expr: HirExprId,
        frame: &Frame,
    ) -> Result<crate::control::CheckedActionInvocation, String> {
        let mut bindings = frame.type_bindings().clone();
        if let Some(fact) = self.checked.types.generic_instantiations.get(&expr) {
            for (name, ty) in &fact.type_bindings {
                let ty = etas_types::substitute_named_params_in_store(
                    &self.checked.type_store,
                    *ty,
                    frame.type_bindings(),
                )
                .map_err(|error| error.to_string())?;
                bindings.insert(name.clone(), ty);
            }
        }
        self.action_invocation_with_bindings(pending, &bindings)
    }

    pub(in crate::eval) fn action_invocation_with_bindings(
        &self,
        pending: &PendingPerform,
        bindings: &std::collections::HashMap<String, etas_types::TypeId>,
    ) -> Result<crate::control::CheckedActionInvocation, String> {
        let expr = pending
            .expr
            .ok_or("perform has no checked expression origin")?;
        let proof = crate::control::CheckedActionInvocation::source(
            self.checked_execution_program(),
            expr,
            bindings,
        )?;
        if proof.parameters.len() != pending.args.len() {
            return Err("perform payload arity differs from its checked signature".into());
        }
        Ok(proof)
    }

    fn error_raise_type_arg(
        &self,
        expr: HirExprId,
        action: &ResolvedActionRef,
        type_args: &[etas_hir::HirTypeId],
        frame: &Frame,
    ) -> Result<Option<etas_types::TypeId>, ExecutionFault> {
        if action.action != "raise"
            || action
                .effect
                .path
                .segments
                .last()
                .is_none_or(|segment| segment.name != "Error")
        {
            return Ok(None);
        }
        let error_type = if let [error_type] = type_args {
            self.checked.types.type_refs.get(error_type).copied()
        } else {
            self.checked
                .effects
                .performed_actions
                .get(&expr)
                .and_then(|fact| {
                    fact.summary
                        .escaping_effects
                        .effects
                        .iter()
                        .find_map(|effect| match effect {
                            etas_effects::Effect::Error(error) => Some(*error),
                            _ => None,
                        })
                })
        };
        let missing = |message| {
            ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                action.span,
                message,
            )
        };
        let ty = error_type
            .ok_or_else(|| missing("Error.raise has no checked error type".to_owned()))?;
        let ty = etas_types::substitute_named_params_in_store(
            &self.checked.type_store,
            ty,
            frame.type_bindings(),
        )
        .map_err(|error| missing(format!("Error.raise type cannot be instantiated: {error}")))?;
        Ok(Some(ty))
    }

    fn performed_action_fact_matches(
        &self,
        expr: HirExprId,
        action: &ResolvedActionRef,
        args: &[HirArg],
    ) -> bool {
        let Some(fact) = self.checked.effects.performed_actions.get(&expr) else {
            return false;
        };
        if fact.expr != expr || fact.action != action.action {
            return false;
        }
        if !self.performed_action_symbol_matches(action, fact.action_symbol) {
            return false;
        }
        fact.args
            .iter()
            .copied()
            .eq(args.iter().map(|arg| match arg {
                HirArg::Positional(value) | HirArg::Named { value, .. } => *value,
            }))
    }

    fn performed_action_symbol_matches(
        &self,
        action: &ResolvedActionRef,
        fact_symbol: Option<etas_hir::SymbolId>,
    ) -> bool {
        match (&action.action_symbol, fact_symbol) {
            (ResolveResult::Resolved(symbol), Some(fact_symbol)) => *symbol == fact_symbol,
            _ => false,
        }
    }
}

pub(super) struct PerformArgsResume {
    pub expr: HirExprId,
    pub action: ResolvedActionRef,
    pub type_args: Vec<etas_hir::HirTypeId>,
    pub args: std::sync::Arc<[HirArg]>,
    pub start_arg_index: usize,
    pub evaluated_args: Vec<InterpValue>,
    pub span: Span,
}
