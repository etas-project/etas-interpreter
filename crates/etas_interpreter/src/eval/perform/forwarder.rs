use etas_core::Span;
use etas_hir::{HirEffectRef, ResolveResult, ResolvedActionRef};

use crate::{
    control::{Continuation, ControlSignal, PendingPerform},
    eval::EvalContext,
    intrinsic::dispatch::CheckedStdIntrinsicCall,
    value::InterpValue,
};

impl EvalContext<'_> {
    pub(in crate::eval) fn execute_checked_default_action(
        &mut self,
        mut perform: PendingPerform,
    ) -> ControlSignal {
        let Some(proof) = &perform.checked_action else {
            return ControlSignal::missing_checked_fact(
                "default boundary has no checked action invocation",
                perform.span,
            );
        };
        let action = match &proof.effect {
            etas_effects::Effect::Action(action) => action,
            etas_effects::Effect::AppliedAction(instance) => &instance.action,
            _ => {
                return ControlSignal::missing_checked_fact(
                    "default boundary does not identify a checked action",
                    perform.span,
                );
            }
        };
        if !std::sync::Arc::ptr_eq(&proof.program, self.checked)
            || !self.plan.action_mediation.has_default_action(action)
        {
            return ControlSignal::missing_checked_fact(
                "default action is absent from the current checked mediation plan",
                perform.span,
            );
        }
        let Some(boundary) = perform.default_boundary.take() else {
            return ControlSignal::missing_checked_fact(
                "default action has no owned Host boundary",
                perform.span,
            );
        };
        let signal = match *boundary {
            crate::control::DefaultActionBoundary::Console(pending) => {
                ControlSignal::Console(pending)
            }
            crate::control::DefaultActionBoundary::Native {
                kind,
                call,
                args,
                type_bindings,
                span,
            } => self.execute_std_default(kind, &call, args, span, &type_bindings),
            crate::control::DefaultActionBoundary::StreamRead {
                stream,
                max_bytes,
                timeout_ms,
                span,
            } => {
                self.execute_stream_read_default(stream, max_bytes, timeout_ms, proof.output, span)
            }
            crate::control::DefaultActionBoundary::FileReadLimited {
                path,
                max_bytes,
                span,
            } => self.execute_file_read_default(path, max_bytes, span),
        };
        self.continue_chain_signal(signal, perform.continuation)
    }

    pub(in crate::eval) fn mediate_default_action(
        &self,
        call: &CheckedStdIntrinsicCall,
        action: etas_effects::ActionRef,
        args: Vec<InterpValue>,
        boundary: crate::control::DefaultActionBoundary,
        span: Span,
    ) -> ControlSignal {
        let Some(signature) = self.checked.effect_registry.action_signature(&action) else {
            return ControlSignal::missing_checked_fact(
                "default action has no checked registry signature",
                span,
            );
        };
        if args.len() != call.parameter_types.len() || !signature.effect_args.is_empty() {
            return ControlSignal::missing_checked_fact(
                "default action requires a matching closed checked invocation",
                span,
            );
        }
        let proof = match crate::control::CheckedActionInvocation::standard(
            self.checked_execution_program(),
            call,
        ) {
            Ok(proof) if proof.effect == etas_effects::Effect::Action(action.clone()) => proof,
            Ok(_) => {
                return ControlSignal::missing_checked_fact(
                    "default action differs from its checked descriptor",
                    span,
                );
            }
            Err(reason) => return ControlSignal::missing_checked_fact(reason, span),
        };
        let Some(owner) = self.checked.effect_registry.tag_name(action.tag) else {
            return ControlSignal::missing_checked_fact(
                "default action has no canonical checked owner",
                span,
            );
        };
        let segments = owner.split('.').collect::<Vec<_>>();
        ControlSignal::pending_perform(PendingPerform {
            checked_action: Some(proof),
            default_boundary: Some(Box::new(boundary)),
            forwarded_call: None,
            expr: None,
            inference: None,
            error_type: None,
            action: ResolvedActionRef {
                effect: HirEffectRef {
                    path: etas_hir::unresolved_path_from_segments(&segments, span),
                    args: Vec::new(),
                    span,
                },
                action: signature.name.clone(),
                action_symbol: ResolveResult::Unresolved,
                span,
            },
            args,
            span,
            continuation: Continuation::BlockValue,
        })
    }
    pub(in crate::eval) fn checked_perform_action(
        &self,
        perform: &PendingPerform,
    ) -> Result<Option<etas_effects::Effect>, crate::control::ExecutionFault> {
        let missing = |reason: String| {
            crate::control::ExecutionFault::new(
                etas_core::AnalysisDiagnosticCode::MissingCheckedFact,
                perform.span,
                reason,
            )
        };
        if let Some(proof) = &perform.checked_action {
            if !std::sync::Arc::ptr_eq(&proof.program, self.checked) {
                return Err(missing(
                    "action invocation belongs to a different checked program".into(),
                ));
            }
            return Ok(Some(proof.effect.clone()));
        }
        if let Some(call) = &perform.forwarded_call {
            let target = self
                .plan
                .dispatch
                .forwarded_action(call.identity)
                .map_err(missing)?;
            return Ok(Some(etas_effects::Effect::Action(target.action.clone())));
        }
        if perform.error_type.is_some() {
            return Ok(None);
        }
        let Some(expr) = perform.expr else {
            return Ok(None);
        };
        let fact = self
            .checked
            .effects
            .performed_actions
            .get(&expr)
            .ok_or_else(|| missing("perform has no checked action instance".into()))?;
        if fact.expr != expr
            || fact.action != perform.action.action
            || !fact.invocation.is_action()
            || !fact
                .summary
                .requested_actions
                .effects
                .contains(&fact.invocation)
        {
            return Err(missing(
                "perform invocation disagrees with its checked action fact".into(),
            ));
        }
        Ok(Some(fact.invocation.clone()))
    }

    pub(in crate::eval) fn execute_action_forwarder(
        &self,
        call: &CheckedStdIntrinsicCall,
        args: Vec<InterpValue>,
        span: Span,
    ) -> ControlSignal {
        let target = match self.plan.dispatch.forwarded_action(call.identity) {
            Ok(target) => target,
            Err(reason) => return ControlSignal::missing_checked_fact(reason, span),
        };
        if args.len() != call.parameter_types.len()
            || !self.plan.action_mediation.requires_action(&target.action)
        {
            return ControlSignal::missing_checked_fact(
                "action forwarder has no matching checked call/request contract",
                span,
            );
        }
        let proof = match crate::control::CheckedActionInvocation::standard(
            self.checked_execution_program(),
            call,
        ) {
            Ok(proof) => proof,
            Err(reason) => return ControlSignal::missing_checked_fact(reason, span),
        };
        let segments = target
            .owner_path
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>();
        ControlSignal::pending_perform(PendingPerform {
            default_boundary: None,
            checked_action: Some(proof),
            forwarded_call: Some(call.clone()),
            expr: None,
            inference: None,
            error_type: None,
            action: ResolvedActionRef {
                effect: HirEffectRef {
                    path: etas_hir::unresolved_path_from_segments(&segments, span),
                    args: Vec::new(),
                    span,
                },
                action: target.name.clone(),
                action_symbol: ResolveResult::Unresolved,
                span,
            },
            args,
            span,
            continuation: Continuation::BlockValue,
        })
    }
}
