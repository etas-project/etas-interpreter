use super::*;
use crate::control::{
    PendingManagedAgent,
    managed_agent::{ManagedAgentOperation, ManagedAgentOrigin},
};

impl EvalContext<'_> {
    pub(in crate::eval) fn prepare_managed_agent_construction(
        &mut self,
        expr: HirExprId,
        frame: &Frame,
        span: Span,
    ) -> Option<ControlSignal> {
        let ty = self.checked.types.expression_producer_type(expr)?;
        if !self
            .checked
            .types
            .is_active_agent(&self.checked.type_store, ty)
        {
            return None;
        }
        let bindings = frame
            .bindings_at(self.checked, etas_hir::HirOwner::Expr(expr))
            .into_owned();
        let ty = match self.materialized_type(ty, &bindings, span) {
            Ok(ty) => ty,
            Err(fault) => return Some(ControlSignal::Fault(Box::new(fault))),
        };
        if !self
            .checked
            .types
            .is_active_agent(&self.checked.type_store, ty)
        {
            return None;
        }
        Some(self.prepare_managed_agent(
            ManagedAgentOrigin::Create { expr, bindings },
            ManagedAgentOperation::Initialize {
                expr,
                frame: frame.clone(),
            },
            span,
        ))
    }

    pub(in crate::eval) fn prepare_managed_agent_call(
        &mut self,
        reference: crate::value::AgentReference,
        updating: bool,
        target: CallTarget,
        args: Vec<InterpValue>,
        span: Span,
        autonomous: bool,
    ) -> ControlSignal {
        let mut current = &target;
        let mut bindings = crate::control::GenericBindings::default();
        loop {
            match current {
                CallTarget::Limited { target, .. } => current = target,
                CallTarget::Specialized {
                    target,
                    type_bindings,
                    type_parameter_bindings,
                    effect_row_bindings,
                } => {
                    bindings.types.extend(type_bindings.iter().cloned());
                    bindings
                        .parameters
                        .extend(type_parameter_bindings.iter().copied());
                    bindings.rows.extend(effect_row_bindings.iter().cloned());
                    current = target;
                }
                _ => break,
            }
        }
        let CallTarget::FlowItem(item) = current else {
            return ControlSignal::missing_checked_fact(
                "managed Agent request lost its checked method",
                span,
            );
        };
        self.prepare_managed_agent(
            ManagedAgentOrigin::Call {
                item: *item,
                receiver: reference.ty,
                bindings,
            },
            ManagedAgentOperation::Submit {
                reference,
                updating,
                autonomous,
                target,
                args,
            },
            span,
        )
    }

    fn prepare_managed_agent(
        &mut self,
        origin: ManagedAgentOrigin,
        operation: ManagedAgentOperation,
        span: Span,
    ) -> ControlSignal {
        let proof = match crate::control::CheckedActionInvocation::managed_agent(
            self.checked_execution_program(),
            origin.clone(),
        ) {
            Ok(proof) => proof,
            Err(reason) => return ControlSignal::missing_checked_fact(reason, span),
        };
        let autonomous = matches!(
            &operation,
            ManagedAgentOperation::Submit {
                autonomous: true,
                ..
            }
        );
        ControlSignal::pending_task(crate::control::PendingTaskControl::ManagedAgent(Box::new(
            PendingManagedAgent {
                proof,
                origin,
                operation,
                span,
                continuation: Continuation::BlockValue,
                autonomous_completion: autonomous.then(|| Box::new(Continuation::BlockValue)),
                residual: None,
            },
        )))
    }

    pub(crate) fn observe_managed_agent(
        &mut self,
        pending: &mut PendingManagedAgent,
    ) -> Result<(), ExecutionFault> {
        if pending.residual.is_none() {
            pending.residual = self.observe_residual_action(
                Some(&pending.proof),
                match &mut pending.autonomous_completion {
                    Some(completion) => completion.as_mut(),
                    None => &mut pending.continuation,
                },
                pending.span,
                &format!("Agentic.{}", pending.origin.name()),
            )?;
        }
        if let Some(tag) = &pending.residual {
            tag.consume_permit().map_err(|reason| {
                ExecutionFault::new(
                    AnalysisDiagnosticCode::TraceSpecResidualViolation,
                    pending.span,
                    reason,
                )
            })?;
        }
        Ok(())
    }

    fn begin_agent_record_initialization(
        &mut self,
        expr: HirExprId,
        frame: &mut Frame,
        span: Span,
    ) -> ControlSignal {
        let Some(ty) = self.checked.types.expression_producer_type(expr) else {
            return ControlSignal::missing_checked_fact(
                "Agent constructor has no result type",
                span,
            );
        };
        let ty = match self.materialized_type(
            ty,
            &frame.bindings_at(self.checked, etas_hir::HirOwner::Expr(expr)),
            span,
        ) {
            Ok(ty) => ty,
            Err(fault) => return ControlSignal::Fault(Box::new(fault)),
        };
        let program = self.checked_execution_program();
        let initialization = match self
            .scheduler
            .begin_agent_construction(self.task, program, ty)
        {
            Ok(init) => init,
            Err(error) => return ControlSignal::runtime_fault(error.to_string(), span),
        };
        self.task
            .agent_initializations
            .retain(|weak| weak.upgrade().is_some_and(|init| init.is_pending()));
        self.task
            .agent_initializations
            .push(std::rc::Rc::downgrade(&initialization));
        let signal = self.resume_record_fields(expr, 0, Vec::new(), frame);
        self.continue_chain_signal(
            signal,
            Continuation::PublishAgentConstruction {
                initialization,
                span,
            },
        )
    }

    pub(crate) fn complete_managed_agent(&mut self, pending: PendingManagedAgent) -> ControlSignal {
        match pending.operation {
            ManagedAgentOperation::Initialize { expr, mut frame } => {
                let signal = self.begin_agent_record_initialization(expr, &mut frame, pending.span);
                self.continue_chain_signal(signal, pending.continuation)
            }
            ManagedAgentOperation::Submit { .. } => {
                unreachable!("the coordinator admits managed method requests")
            }
        }
    }
}
