use super::*;
use etas_host::{ClockObservation, ClockOperation, HostError, HostErrorCode, HostValue};
use etas_types::{EffectArgRef, PrimitiveType, Type};

impl EvalContext<'_> {
    pub(crate) fn source_clock_action(
        &mut self,
        perform: &PendingPerform,
    ) -> Option<ControlSignal> {
        let ("Clock", action @ ("now" | "sleep" | "monotonic")) =
            self.source_default_signature(perform)?
        else {
            return None;
        };
        let sleep = action == "sleep";
        let monotonic = action == "monotonic";
        let proof = perform.checked_action.as_ref().unwrap();
        let time = resolve_std_type(self.checked, &["std", "runtime", "time", "Time"]);
        let instant = resolve_std_type(self.checked, &["std", "runtime", "time", "Instant"]);
        let duration = resolve_std_type(self.checked, &["std", "runtime", "time", "Duration"]);
        let error_type = resolve_std_type(self.checked, &["std", "runtime", "time", "TimeError"]);
        let signature_ok = matches!(&proof.effect, etas_effects::Effect::Action(_))
            && if sleep {
                duration.is_some()
                    && proof.parameters.as_slice() == duration.as_slice()
                    && self.checked.type_store.get(proof.output)
                        == Some(&Type::Primitive(PrimitiveType::Unit))
            } else {
                proof.parameters.is_empty()
                    && Some(proof.output) == if monotonic { instant } else { time }
            };
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
        let row_ok = error_type.is_some() && row.is_some_and(|row| row.tail.is_none()
            && matches!(row.effects.as_slice(), [effect]
                if self.checked.effect_registry.tag_by_name(&effect.name) == Some(etas_effects::ERROR_TAG)
                    && matches!(effect.args.as_slice(), [EffectArgRef::Type(ty)] if Some(*ty) == error_type)));
        if !signature_ok || !row_ok {
            return Some(ControlSignal::missing_checked_fact(
                "Clock action signature differs from its runtime ABI",
                perform.span,
            ));
        }
        let operation = if sleep {
            let [value] = perform.args.as_slice() else {
                return Some(ControlSignal::missing_checked_fact(
                    "Clock.sleep requires Duration",
                    perform.span,
                ));
            };
            let InterpValue::Record(fields) = super::std_call::nominal_representation_ref(value)
            else {
                return Some(ControlSignal::missing_checked_fact(
                    "Duration requires its millis record",
                    perform.span,
                ));
            };
            let Some(millis) = fields
                .get_ref("millis")
                .and_then(InterpValue::as_number)
                .and_then(|value| value.as_u128())
            else {
                return Some(ControlSignal::missing_checked_fact(
                    "Duration.millis requires u128",
                    perform.span,
                ));
            };
            ClockOperation::Sleep { millis }
        } else if monotonic {
            ClockOperation::Monotonic {
                run: self.replay_keys.id(),
            }
        } else {
            ClockOperation::Now
        };
        let output = proof.output;
        Some(ControlSignal::pending_host(PendingHostBoundary {
            request: HostBoundaryRequest::Clock(etas_host::ClockRequest {
                id: self.next_host_request_id(),
                operation,
                authority: self.host_authority(),
                trace: self.host_trace(),
                budget: self.host_budget(),
            }),
            decode: HostBoundaryDecode::Clock {
                result_type: output,
            },
            span: perform.span,
            continuation: perform.continuation.clone(),
        }))
    }

    pub(crate) fn clock_observation_value(
        &self,
        output: etas_types::TypeId,
        observation: ClockObservation,
    ) -> Result<InterpValue, String> {
        let value = match observation {
            ClockObservation::Time { unix_millis } => {
                HostValue::Record(vec![("unix_millis".into(), HostValue::Int(unix_millis))])
            }
            ClockObservation::Instant { domain, millis } => {
                let clock =
                    crate::value::host_handle::source_handle_type(self.checked, output, "clock")?;
                crate::value::host_handle::validate_clock_domain(self.checked, clock, &domain)?;
                let value = InterpValue::Nominal {
                    ty: output,
                    value: InterpValue::Record(
                        vec![
                            (
                                "clock".into(),
                                InterpValue::HostHandle(
                                    crate::value::HostHandleValue::clock_domain(
                                        clock,
                                        domain.clone(),
                                    ),
                                ),
                            ),
                            ("domain".into(), InterpValue::String(domain.into())),
                            (
                                "millis".into(),
                                InterpValue::Number(crate::value::NumericValue::U128(millis)),
                            ),
                        ]
                        .into(),
                    )
                    .into(),
                };
                crate::value::contract::runtime::validate_value(
                    self.checked,
                    self.plan,
                    &value,
                    output,
                    &self.storage_limits,
                )?;
                return Ok(value);
            }
            ClockObservation::Woke => HostValue::Unit,
        };
        decode_checked_host_value(value, output, self.checked)
    }

    pub(crate) fn clock_error_value(
        &self,
        error: HostError,
        span: Span,
    ) -> Result<InterpValue, crate::control::ExecutionFault> {
        let error_type = resolve_std_type(self.checked, &["std", "runtime", "time", "TimeError"])
            .expect("checked Clock default error ABI");
        let value = if error.code == HostErrorCode::ProviderUnavailable {
            HostValue::Variant {
                name: "Unavailable".into(),
                fields: vec![],
            }
        } else if error.code == HostErrorCode::InvalidRequest
            && error
                .details
                .iter()
                .any(|detail| detail.key == "time_error" && detail.value == "Overflow")
        {
            HostValue::Variant {
                name: "Overflow".into(),
                fields: vec![],
            }
        } else {
            HostValue::Variant {
                name: "Host".into(),
                fields: vec![HostValue::String(error.message)],
            }
        };
        decode_checked_host_value(value, error_type, self.checked).map_err(|reason| {
            crate::control::ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                span,
                reason,
            )
        })
    }

    pub(crate) fn resume_clock_error(
        &mut self,
        boundary: PendingHostBoundary,
        value: InterpValue,
    ) -> ControlSignal {
        let error_type = resolve_std_type(self.checked, &["std", "runtime", "time", "TimeError"])
            .expect("checked Clock default error ABI");
        self.propagate_perform_signal(PendingPerform::typed_error(
            error_type,
            value,
            boundary.span,
            boundary.continuation,
        ))
    }
}
