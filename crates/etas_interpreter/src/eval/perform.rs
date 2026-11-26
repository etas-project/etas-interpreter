use super::*;
use crate::control::ExecutionFault;

mod defaults;
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
                | ControlSignal::ProgramExpr(_)
                | ControlSignal::Call(_)
                | ControlSignal::Native(_)
                | ControlSignal::Perform(_)
                | ControlSignal::Memory(_)
                | ControlSignal::Session(_)
                | ControlSignal::Console(_)
                | ControlSignal::Command(_)
                | ControlSignal::Model(_)
                | ControlSignal::Host(_)
                | ControlSignal::Task(_)) => {
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
        let (parameter_count, slots, defaults) =
            match self.checked.types.declared_call_arguments.get(&expr) {
                Some(binding) => (
                    binding.argument_slots.len() + binding.defaults.len(),
                    binding.argument_slots.clone(),
                    binding.defaults.clone(),
                ),
                None => (
                    evaluated_args.len(),
                    (0..evaluated_args.len() as u32).collect(),
                    Vec::new(),
                ),
            };
        self.prepare_default_arguments(super::arguments::DefaultArguments {
            tool: None,
            expr,
            parameter_count,
            slots,
            values: evaluated_args,
            defaults,
            next_default: 0,
            destination: super::arguments::ArgumentDestination::Perform {
                action,
                type_args,
                source_args: args,
            },
            span,
            frame: frame.clone(),
        })
    }

    pub(super) fn finish_perform_arguments(
        &mut self,
        expr: HirExprId,
        action: etas_hir::ResolvedActionRef,
        type_args: Vec<etas_hir::HirTypeId>,
        args: std::sync::Arc<[HirArg]>,
        evaluated_args: Vec<InterpValue>,
        span: Span,
        frame: &mut Frame,
    ) -> ControlSignal {
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
            residual: None,
        };
        if error_type.is_none() {
            match self.action_invocation(&pending, frame) {
                Ok(proof) => pending.checked_action = Some(proof),
                Err(reason) => return ControlSignal::missing_checked_fact(reason, span),
            }
        }
        ControlSignal::pending_perform(pending)
    }

    fn action_invocation(
        &self,
        pending: &PendingPerform,
        frame: &Frame,
    ) -> Result<crate::control::CheckedActionInvocation, String> {
        let owner = pending
            .expr
            .ok_or("perform has no checked expression origin")?;
        self.action_invocation_with_bindings(
            pending,
            &frame.bindings_at(self.checked, etas_hir::HirOwner::Expr(owner)),
        )
    }

    pub(in crate::eval) fn action_invocation_with_bindings(
        &self,
        pending: &PendingPerform,
        bindings: &crate::control::GenericBindings,
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
        _type_args: &[etas_hir::HirTypeId],
        frame: &Frame,
    ) -> Result<Option<etas_types::TypeId>, ExecutionFault> {
        let Some(fact) = self.checked.effects.performed_actions.get(&expr) else {
            return Err(ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                action.span,
                "performed action has no checked invocation fact",
            ));
        };
        let etas_effects::Effect::Error(error_type) = fact.invocation else {
            return Ok(None);
        };
        let missing = |message| {
            ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                action.span,
                message,
            )
        };
        let ty = error_type;
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
