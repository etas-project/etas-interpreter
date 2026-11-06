use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use etas_core::{AnalysisDiagnosticCode, Span};
use etas_hir::{HirBlockId, HirItemId};

use crate::control::{
    ControlSignal, ExecutionFault, Frame, PendingBlock,
    residual::{ResidualScopeActivation, ResidualScopeInstance, SharedResidualScope},
};
use crate::eval::EvalContext;

impl EvalContext<'_> {
    /// Starts a callable body. A callable with an installed residual plan
    /// runs its body under a fresh, independent scope activated by the
    /// machine before the first statement executes.
    pub(in crate::eval) fn execute_item_body(
        &mut self,
        item: HirItemId,
        body: HirBlockId,
        frame: &mut Frame,
    ) -> ControlSignal {
        let residual = match self.plan.residual.scope(item) {
            Some(scope) => {
                let bindings = frame.generic_bindings();
                let application = scope.applications.iter().find(|application| {
                    application
                        .checked
                        .type_bindings
                        .iter()
                        .all(|(name, ty)| bindings.types.get(name) == Some(ty))
                        && application
                            .checked
                            .effect_row_bindings
                            .iter()
                            .all(|(name, row)| bindings.rows.get(name) == Some(row))
                });
                let Some(application) = application else {
                    return ControlSignal::fault(
                        AnalysisDiagnosticCode::MissingCheckedFact,
                        self.checked.hir.items.get(item).unwrap().span(),
                        "residual scope has no checked generic application",
                    );
                };
                Some(Box::new(ResidualScopeActivation {
                    scope: scope.clone(),
                    application: application.clone(),
                }))
            }
            None => None,
        };
        ControlSignal::pending_block(PendingBlock {
            block: body,
            next_stmt_index: 0,
            frame: frame.clone(),
            continuation: crate::control::Continuation::BlockValue,
            residual,
        })
    }

    /// Creates the live scope state for one activation. The activation must
    /// come from the prepared plans of the program being executed.
    pub(crate) fn activate_residual_scope(
        &mut self,
        activation: ResidualScopeActivation,
        span: Span,
    ) -> Result<SharedResidualScope, ExecutionFault> {
        let fail = |reason: String| {
            ExecutionFault::new(
                AnalysisDiagnosticCode::TraceSpecResidualViolation,
                span,
                reason,
            )
        };
        self.plan
            .residual_installation
            .validate(self.checked, self.plan.root, &self.plan.residual)
            .map_err(fail)?;
        let installed = self
            .plan
            .residual
            .scope(activation.scope.item)
            .is_some_and(|scope| Arc::ptr_eq(scope, &activation.scope));
        if !installed {
            return Err(fail(
                "residual scope activation is not installed in the executing program".into(),
            ));
        }
        let limits = self.task.residual.limits;
        self.task
            .residual
            .log
            .check_activation_capacity(limits.max_active_scopes)
            .map_err(fail)?;
        let invocation = self.task.residual.next_invocation().map_err(fail)?;
        let log = self.task.residual.log.clone();
        let instance = ResidualScopeInstance::activate(
            activation.scope,
            activation.application,
            self.checked.clone(),
            invocation,
            &log,
            limits,
        )
        .map_err(fail)?;
        let scope = Rc::new(RefCell::new(instance));
        log.register(&scope, limits.max_active_scopes)
            .map_err(fail)?;
        Ok(scope)
    }
}
