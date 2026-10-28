use etas_core::{AnalysisDiagnosticCode, Span};
use etas_effects::Effect;

use crate::{
    control::{
        CheckedActionInvocation, Continuation, ControlSignal, DefaultActionBoundary,
        ExecutionFault, PendingPerform,
    },
    eval::EvalContext,
    intrinsic::dispatch::CheckedStdIntrinsicCall,
    value::InterpValue,
};

type NativeInvocation = (CheckedActionInvocation, Vec<InterpValue>);

impl EvalContext<'_> {
    pub(in crate::eval) fn prepare_native_action(
        &self,
        call: &CheckedStdIntrinsicCall,
        args: &[InterpValue],
        span: Span,
    ) -> Result<Option<NativeInvocation>, ExecutionFault> {
        let missing = |reason: String| {
            ExecutionFault::new(AnalysisDiagnosticCode::MissingCheckedFact, span, reason)
        };
        let Some(adapter) = self.plan.dispatch.default_action(call).map_err(missing)? else {
            return Ok(None);
        };
        if args.len() != call.parameter_types.len() {
            return Err(missing(
                "standard action invocation disagrees with its checked callable ABI".into(),
            ));
        }
        let proof = CheckedActionInvocation::standard(self.checked_execution_program(), call)
            .map_err(missing)?;
        let args = adapter
            .payload
            .iter()
            .map(|index| {
                args.get(*index)
                    .cloned()
                    .ok_or_else(|| missing("native action payload is missing an argument".into()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Some((proof, args)))
    }

    pub(in crate::eval) fn mediate_native_action(
        &self,
        proof: CheckedActionInvocation,
        args: Vec<InterpValue>,
        boundary: DefaultActionBoundary,
        span: Span,
    ) -> ControlSignal {
        let action = match &proof.effect {
            Effect::Action(action) => action,
            Effect::AppliedAction(instance) => &instance.action,
            _ => {
                return ControlSignal::missing_checked_fact(
                    "native action has no checked action identity",
                    span,
                );
            }
        };
        let Some(signature) = self.checked.effect_registry.action_signature(action) else {
            return ControlSignal::missing_checked_fact(
                "native action declaration is missing",
                span,
            );
        };
        let Some(owner) = self.checked.effect_registry.tag_name(action.tag) else {
            return ControlSignal::missing_checked_fact("native action owner is missing", span);
        };
        let segments = owner.split('.').collect::<Vec<_>>();
        let action = etas_hir::ResolvedActionRef {
            effect: etas_hir::HirEffectRef {
                path: etas_hir::unresolved_path_from_segments(&segments, span),
                args: Vec::new(),
                span,
            },
            action: signature.name.clone(),
            action_symbol: etas_hir::ResolveResult::Unresolved,
            span,
        };
        ControlSignal::pending_perform(PendingPerform {
            checked_action: Some(proof),
            default_boundary: Some(Box::new(boundary)),
            forwarded_call: None,
            expr: None,
            inference: None,
            error_type: None,
            action,
            args,
            span,
            continuation: Continuation::BlockValue,
        })
    }
}
