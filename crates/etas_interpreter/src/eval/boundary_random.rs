use super::*;
use etas_host::{HostError, HostErrorCode, HostValue};
use etas_types::{EffectArgRef, PrimitiveType, Type};

impl EvalContext<'_> {
    pub(crate) fn source_random_action(
        &mut self,
        perform: &PendingPerform,
    ) -> Option<ControlSignal> {
        if self.source_default_signature(perform)? != ("Random", "entropy") {
            return None;
        }
        let proof = perform.checked_action.as_ref().unwrap();
        if !matches!(&proof.effect, etas_effects::Effect::Action(_)) {
            return Some(ControlSignal::missing_checked_fact(
                "Random.entropy runtime ABI has no static selectors",
                perform.span,
            ));
        }
        let Some(error_type) = resolve_std_type(self.checked, &["std", "random", "RandomError"])
        else {
            return Some(ControlSignal::missing_checked_fact(
                "Random.entropy requires its checked RandomError type",
                perform.span,
            ));
        };
        let signature_ok = proof.parameters.len() == 1
            && self.checked.type_store.get(proof.parameters[0])
                == Some(&Type::Primitive(PrimitiveType::USize))
            && self.checked.type_store.get(proof.output)
                == Some(&Type::Primitive(PrimitiveType::Bytes));
        let row = perform
            .expr
            .and_then(|expr| match self.checked.hir.exprs.get(expr) {
                Some(HirExpr::Perform { action, .. }) => match action.action_symbol {
                    ResolveResult::Resolved(symbol) => self
                        .checked
                        .types
                        .action_signatures
                        .get(&symbol)
                        .and_then(|signature| signature.default_handler_effects.as_ref()),
                    _ => None,
                },
                _ => None,
            });
        let row_ok = row.is_some_and(|row| row.tail.is_none() && matches!(row.effects.as_slice(), [effect]
            if self.checked.effect_registry.tag_by_name(&effect.name) == Some(etas_effects::ERROR_TAG)
                && matches!(effect.args.as_slice(), [EffectArgRef::Type(ty)] if *ty == error_type)));
        let [InterpValue::Number(count)] = perform.args.as_slice() else {
            return Some(ControlSignal::missing_checked_fact(
                "Random.entropy requires its checked usize argument",
                perform.span,
            ));
        };
        let Some(count) = count.as_usize() else {
            return Some(ControlSignal::missing_checked_fact(
                "Random.entropy count exceeds its usize ABI",
                perform.span,
            ));
        };
        if !signature_ok || !row_ok {
            return Some(ControlSignal::missing_checked_fact(
                "Random.entropy signature differs from its runtime ABI",
                perform.span,
            ));
        }
        Some(ControlSignal::pending_host(PendingHostBoundary {
            request: HostBoundaryRequest::Entropy(etas_host::EntropyRequest {
                id: self.next_host_request_id(),
                count,
                max_bytes: self.storage_limits.max_result_bytes,
                authority: self.host_authority(),
                trace: self.host_trace(),
                budget: self.host_budget(),
            }),
            decode: HostBoundaryDecode::Bytes,
            span: perform.span,
            continuation: perform.continuation.clone(),
        }))
    }

    pub(crate) fn random_error_value(
        &self,
        error: HostError,
        span: Span,
    ) -> Result<InterpValue, crate::control::ExecutionFault> {
        let error_type = resolve_std_type(self.checked, &["std", "random", "RandomError"])
            .expect("checked Random default error ABI");
        let value = match error.code {
            HostErrorCode::ProviderUnavailable => HostValue::Variant {
                name: "Unavailable".into(),
                fields: vec![],
            },
            _ => HostValue::Variant {
                name: "Host".into(),
                fields: vec![HostValue::String(error.message)],
            },
        };
        super::host_value::decode_checked_host_value(value, error_type, self.checked).map_err(
            |reason| {
                crate::control::ExecutionFault::new(
                    AnalysisDiagnosticCode::MissingCheckedFact,
                    span,
                    reason,
                )
            },
        )
    }

    pub(crate) fn resume_random_error(
        &mut self,
        boundary: PendingHostBoundary,
        value: InterpValue,
    ) -> ControlSignal {
        let error_type = resolve_std_type(self.checked, &["std", "random", "RandomError"])
            .expect("checked Random default error ABI");
        self.propagate_perform_signal(PendingPerform {
            checked_action: None,
            default_boundary: None,
            forwarded_call: None,
            inference: None,
            expr: None,
            action: ResolvedActionRef {
                effect: etas_hir::HirEffectRef {
                    path: etas_hir::unresolved_path_from_segments(&["Error"], boundary.span),
                    args: vec![],
                    span: boundary.span,
                },
                action: "raise".into(),
                action_symbol: ResolveResult::Unresolved,
                span: boundary.span,
            },
            error_type: Some(error_type),
            args: vec![value],
            span: boundary.span,
            continuation: boundary.continuation,
            residual: None,
        })
    }
}
