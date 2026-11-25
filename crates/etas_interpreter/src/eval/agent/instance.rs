use super::*;
use crate::value::AgentReference;

impl EvalContext<'_> {
    pub(in crate::eval) fn active_method_request(
        &self,
        target: &CallTarget,
        args: &[InterpValue],
    ) -> Option<(AgentReference, bool)> {
        let Some(InterpValue::Agent(reference)) = args.first() else {
            return None;
        };
        let mut target = target;
        while let CallTarget::Specialized { target: inner, .. }
        | CallTarget::Limited { target: inner, .. } = target
        {
            target = inner;
        }
        let CallTarget::FlowItem(item) = target else {
            return None;
        };
        let Some(HirItem::Flow(flow)) = self.checked.hir.items.get(*item) else {
            return None;
        };
        let method = self.checked.types.agent_methods.get(&flow.symbol)?;
        if !self
            .checked
            .types
            .is_active_agent(&self.checked.type_store, method.component)
        {
            return None;
        }
        Some((
            *reference,
            flow.receiver_mode == etas_hir::HirReceiverMode::Updating,
        ))
    }

    pub(in crate::eval) fn is_agent_turn_helper(
        &self,
        reference: AgentReference,
        site: Option<&crate::control::CallSite>,
    ) -> bool {
        if !self
            .task
            .agent_turn
            .as_ref()
            .is_some_and(|turn| turn.reference == reference && turn.updating)
        {
            return false;
        }
        let Some(crate::control::CallSite {
            kind: crate::control::CallSiteKind::Expr { expr },
            ..
        }) = site
        else {
            return false;
        };
        self.checked
            .types
            .checked_method_calls
            .get(expr)
            .is_some_and(|method| {
                method.kind == etas_types::facts::methods::MethodCallKind::AgentTurnHelper
            })
    }

    pub(crate) fn prepare_agent_request(
        &mut self,
        reference: AgentReference,
        target: CallTarget,
        args: Vec<InterpValue>,
        span: Span,
    ) -> Result<crate::driver::scheduler::TaskHandle, crate::driver::scheduler::SchedulerError>
    {
        let program = self.checked_execution_program();
        let mut machine = crate::eval::machine::EvalMachine::new();
        machine.resume(ControlSignal::pending_call(PendingCall {
            purity: crate::control::CallPurity::Ordinary,
            site: None,
            target,
            args,
            span,
            continuation: Continuation::BlockValue,
        }));
        let scope = self.scheduler.create_scope_with_policy(
            self.task.host_task_scope(),
            std::num::NonZeroUsize::new(crate::driver::scheduler::MAX_CONCURRENCY).unwrap(),
            crate::driver::scheduler::TaskGroupPolicy::Run,
        );
        let handle = self.scheduler.spawn_in_scope(
            self.task,
            scope,
            machine,
            Some(program.clone()),
            self.handler_floor,
            span.range.start.0 as usize,
        )?;
        if let Err(error) = self
            .scheduler
            .set_scope_owner(scope, handle, self.task.logical_identity)
            .and_then(|()| {
                self.scheduler
                    .bind_agent_request_context(handle, reference, &program)
            })
        {
            self.scheduler.cancel(
                handle,
                etas_host::execution::CancellationReason::OwnerDropped,
            )?;
            return Err(error);
        }
        Ok(handle)
    }
}

impl EvalContext<'_> {
    pub(in crate::eval) fn start_autonomous_main(
        &mut self,
        reference: AgentReference,
        span: Span,
    ) -> ControlSignal {
        if !etas_types::active_agent::has_main(
            &self.checked.types,
            &self.checked.type_store,
            reference.ty,
        ) {
            return ControlSignal::Value(InterpValue::Agent(reference));
        }
        let entry = match etas_types::active_agent::autonomous_main(
            &self.checked.types,
            &self.checked.type_store,
            self.plan.checked_spec_facts(),
            reference.ty,
        ) {
            Ok(Some(witness)) => (witness.declaration.target.clone(), witness.owner_bindings),
            Ok(None) => return ControlSignal::Value(InterpValue::Agent(reference)),
            Err(reason) => return ControlSignal::missing_checked_fact(reason, span),
        };
        let target = match entry.0 {
            etas_types::method::MethodTarget::Source { callable, .. }
            | etas_types::method::MethodTarget::Agent { callable, .. } => {
                CallTarget::FlowItem(callable)
            }
            _ => {
                return ControlSignal::missing_checked_fact(
                    "Autonomous.main has no linked executable method",
                    span,
                );
            }
        };
        let target = if entry.1.is_empty() {
            target
        } else {
            CallTarget::Specialized {
                target: target.into(),
                type_bindings: Vec::new(),
                type_parameter_bindings: entry.1.into_iter().collect(),
                effect_row_bindings: Vec::new(),
            }
        };
        self.prepare_managed_agent_call(
            reference,
            true,
            target,
            vec![InterpValue::Agent(reference)],
            span,
            true,
        )
    }
}
