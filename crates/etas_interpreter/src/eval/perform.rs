use super::*;

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
                            action: action.clone(),
                            type_args: type_args.clone(),
                            args: args.clone(),
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
        let error_type = self.error_raise_type_arg(expr, &action, &type_args);
        ControlSignal::pending_perform(PendingPerform {
            expr: Some(expr),
            error_type,
            action,
            args: evaluated_args,
            span,
            continuation: Continuation::BlockValue,
        })
    }

    fn error_raise_type_arg(
        &self,
        expr: HirExprId,
        action: &ResolvedActionRef,
        type_args: &[etas_hir::HirTypeId],
    ) -> Option<etas_types::TypeId> {
        if action.action != "raise"
            || action
                .effect
                .path
                .segments
                .last()
                .is_none_or(|segment| segment.name != "Error")
        {
            return None;
        }
        if let [error_type] = type_args {
            return self.checked.types.type_refs.get(error_type).copied();
        }
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
        let arg_exprs = args
            .iter()
            .map(|arg| match arg {
                HirArg::Positional(value) | HirArg::Named { value, .. } => *value,
            })
            .collect::<Vec<_>>();
        fact.args == arg_exprs
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
    pub args: Vec<HirArg>,
    pub start_arg_index: usize,
    pub evaluated_args: Vec<InterpValue>,
    pub span: Span,
}
