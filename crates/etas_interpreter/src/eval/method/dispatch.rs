use super::*;
use etas_types::method::{MethodReceiver, MethodTarget};

impl EvalContext<'_> {
    pub(in crate::eval) fn bind_method_value(
        &self,
        expr: HirExprId,
        receiver: InterpValue,
        frame: &Frame,
        span: Span,
    ) -> ControlSignal {
        let Some(fact) = self.plan.dispatch.method(expr) else {
            return ControlSignal::missing_checked_fact(
                "method value has no prepared target",
                span,
            );
        };
        let MethodReceiver::Value { slot } = fact.receiver else {
            unreachable!()
        };
        match self.prepared_method_target(expr, Some(&receiver), frame, span) {
            Ok(target) => ControlSignal::Value(InterpValue::Callable(CallTarget::BoundMethod {
                target: target.into(),
                receiver: crate::value::SharedValue::new(receiver),
                slot,
            })),
            Err(fault) => ControlSignal::Fault(Box::new(fault)),
        }
    }

    pub(in crate::eval) fn eval_method_call(
        &mut self,
        dispatch: MethodDispatch<'_>,
        receiver: HirExprId,
        frame: &mut Frame,
    ) -> ControlSignal {
        let Some(fact) = self.plan.dispatch.method(dispatch.expr) else {
            return ControlSignal::missing_checked_fact(
                "method call has no prepared checked target",
                dispatch.span,
            );
        };
        if fact.receiver == MethodReceiver::Associated {
            return self.resume_checked_method(
                dispatch.expr,
                None,
                dispatch.args,
                dispatch.span,
                frame,
            );
        }
        match self.eval_expr(receiver, frame) {
            ControlSignal::Value(value) => self.eval_method_on_receiver(dispatch, value, frame),
            signal if is_pending_host_boundary_signal(&signal) => {
                attach_method_receiver_continuation(
                    signal,
                    dispatch.expr,
                    dispatch.method,
                    dispatch.type_args,
                    dispatch.args,
                    dispatch.span,
                    frame,
                )
            }
            other => other,
        }
    }

    pub(in crate::eval) fn eval_method_on_receiver(
        &mut self,
        dispatch: MethodDispatch<'_>,
        receiver: InterpValue,
        frame: &mut Frame,
    ) -> ControlSignal {
        self.resume_checked_method(
            dispatch.expr,
            Some(receiver),
            dispatch.args,
            dispatch.span,
            frame,
        )
    }

    fn resume_checked_method(
        &mut self,
        call: HirExprId,
        receiver: Option<InterpValue>,
        args: &[HirArg],
        span: Span,
        frame: &mut Frame,
    ) -> ControlSignal {
        let receiver_mode = match self.plan.dispatch.method(call) {
            Some(fact) => fact.receiver,
            None => {
                return ControlSignal::missing_checked_fact(
                    "method call has no prepared checked target",
                    span,
                );
            }
        };
        let target = match self.prepared_method_target(call, receiver.as_ref(), frame, span) {
            Ok(target) => target,
            Err(fault) => return ControlSignal::Fault(Box::new(fault)),
        };
        let evaluated_args = match (receiver_mode, receiver) {
            (MethodReceiver::Value { .. }, Some(value)) => vec![value],
            (MethodReceiver::Associated, None) | (MethodReceiver::Component, Some(_)) => Vec::new(),
            _ => {
                return ControlSignal::missing_checked_fact(
                    "method receiver does not match its checked ABI",
                    span,
                );
            }
        };
        self.resume_call_args(
            CallArgsState {
                call,
                target,
                args: args.into(),
                start_arg_index: 0,
                evaluated_args,
                span,
            },
            frame,
        )
    }

    fn prepared_method_target(
        &self,
        call: HirExprId,
        receiver: Option<&InterpValue>,
        frame: &Frame,
        span: Span,
    ) -> Result<CallTarget, ExecutionFault> {
        let missing = |message: &str| {
            ExecutionFault::new(AnalysisDiagnosticCode::MissingCheckedFact, span, message)
        };
        let selection = self
            .plan
            .dispatch
            .resolve_method(
                self.checked,
                call,
                frame.generic_bindings(),
                receiver.and_then(|value| match value {
                    InterpValue::Existential(value) => {
                        Some(crate::value::ExistentialEvidence::Runtime(value))
                    }
                    _ => None,
                }),
            )
            .map_err(|reason| missing(&reason))?;
        let bindings = selection.bindings;
        let target = match selection.target {
            MethodTarget::Source { callable, .. } => CallTarget::FlowItem(*callable),
            MethodTarget::Standard { .. } => {
                let checked = self
                    .plan
                    .dispatch
                    .std_method(call)
                    .ok_or_else(|| missing("standard method has no prepared intrinsic ABI"))?;
                if checked.identity.dispatch == etas_std::IntrinsicDispatch::PureKernel {
                    CallTarget::PureIntrinsic(
                        crate::intrinsic::dispatch::CheckedPureIntrinsicCall {
                            intrinsic: checked.identity.intrinsic,
                            parameter_types: checked.parameter_types.clone(),
                            result_type: checked.result_type,
                        }
                        .specialize(&self.checked.type_store, frame.generic_bindings())
                        .map_err(|reason| missing(&reason))?,
                    )
                } else {
                    CallTarget::StdIntrinsic(
                        checked
                            .specialize_generic(&self.checked.type_store, frame.generic_bindings())
                            .map_err(|reason| missing(&reason))?,
                    )
                }
            }
            MethodTarget::Agent { .. } => self.checked_agent_method_target(
                call,
                receiver
                    .ok_or_else(|| missing("agent method has no evaluated component"))?
                    .clone(),
                span,
            )?,
            MethodTarget::BoundSpec { .. } => {
                unreachable!("prepared witness resolves the generic target")
            }
            MethodTarget::External { .. } => {
                return Err(missing(
                    "external method has no materialized executable body",
                ));
            }
        };
        if bindings.types.is_empty() && bindings.parameters.is_empty() && bindings.rows.is_empty() {
            return Ok(target);
        }
        Ok(CallTarget::Specialized {
            target: target.into(),
            type_bindings: bindings.types,
            type_parameter_bindings: bindings.parameters,
            effect_row_bindings: bindings.rows,
        })
    }
}
