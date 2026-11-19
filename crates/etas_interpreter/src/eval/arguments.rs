use super::*;
use crate::control::{CheckedCallBindings, ExecutionFault, GenericBindings, PendingExpr};

#[derive(Clone, Debug)]
pub(crate) enum ArgumentDestination {
    Call {
        target: Option<CallTarget>,
    },
    Perform {
        action: etas_hir::ResolvedActionRef,
        type_args: Vec<etas_hir::HirTypeId>,
        source_args: std::sync::Arc<[HirArg]>,
    },
}

/// Invocation preparation, before any callable body or action dispatch.
#[derive(Clone, Debug)]
pub(crate) struct DefaultArguments {
    pub expr: HirExprId,
    pub parameter_count: usize,
    pub slots: Vec<u32>,
    pub values: Vec<InterpValue>,
    pub defaults: Vec<etas_types::DefaultArgument>,
    pub next_default: usize,
    pub destination: ArgumentDestination,
    pub span: Span,
    pub frame: Frame,
}

impl<'a> EvalContext<'a> {
    pub(super) fn prepare_default_arguments(
        &mut self,
        mut state: DefaultArguments,
    ) -> ControlSignal {
        if let Some(default) = state.defaults.get(state.next_default) {
            let reference = match &default.source {
                etas_types::ParameterDefaultRef::External { reference } => reference.clone(),
                etas_types::ParameterDefaultRef::Source { parameter } => {
                    let Some(reference) = self.plan.arguments.source_default_reference(*parameter)
                    else {
                        return ControlSignal::missing_checked_fact(
                            "parameter default has no prepared authored expression unit",
                            state.span,
                        );
                    };
                    reference.clone()
                }
            };
            let (slots, args): (Vec<_>, Vec<_>) = state
                .slots
                .iter()
                .zip(&state.values)
                .filter_map(|(slot, value)| {
                    // A component receiver can occupy a caller-only position.
                    // Bind values in the defining declaration's slot universe.
                    let native =
                        i64::from(*slot) + i64::from(reference.parameter) - i64::from(default.slot);
                    (native >= 0 && native < i64::from(reference.parameter))
                        .then(|| (native as u32, value.clone()))
                })
                .unzip();
            let site = crate::control::CallSite {
                kind: crate::control::CallSiteKind::Expr { expr: state.expr },
                receiver_update: None,
                frame: state.frame.clone(),
                composition: None,
                task_callback: None,
            };
            let call = match super::machine::expression_call::ExpressionCall::at_site(
                self.checked_execution_program(),
                reference,
                site,
                slots,
            ) {
                Ok(call) => call,
                Err(reason) => return ControlSignal::missing_checked_fact(reason, state.span),
            };
            state.next_default += 1;
            return ControlSignal::ProgramExpr(Box::new(crate::control::PendingProgramExpr {
                call,
                args,
                span: state.span,
                continuation: Continuation::DefaultArguments(Box::new(state)),
            }));
        }
        match state.destination {
            ArgumentDestination::Call { target } => self.finish_call_arguments(
                state.expr,
                target,
                state.parameter_count,
                state.slots,
                state.values,
                state.span,
                &mut state.frame,
            ),
            ArgumentDestination::Perform {
                action,
                type_args,
                source_args,
            } => {
                let values = match dense_arguments(state.parameter_count, state.slots, state.values)
                {
                    Ok(values) => values,
                    Err(reason) => return ControlSignal::missing_checked_fact(reason, state.span),
                };
                self.finish_perform_arguments(
                    state.expr,
                    action,
                    type_args,
                    source_args,
                    values,
                    state.span,
                    &mut state.frame,
                )
            }
        }
    }

    pub(super) fn resume_default_argument(
        &mut self,
        mut state: DefaultArguments,
        value: InterpValue,
    ) -> ControlSignal {
        let default = &state.defaults[state.next_default - 1];
        state.slots.push(default.slot);
        state.values.push(value);
        self.prepare_default_arguments(state)
    }

    pub(crate) fn enter_parameter_default(
        &mut self,
        reference: &etas_code::descriptor::ParameterDefaultInfo,
        bindings: GenericBindings,
        slots: &[u32],
        values: &[InterpValue],
        span: Span,
    ) -> ControlSignal {
        let Some(unit) = self.plan.arguments.default_unit(reference).cloned() else {
            return ControlSignal::missing_checked_fact(
                "default entry has no matching authored expression revision",
                span,
            );
        };
        if self.plan.root != etas_frontend::CheckedCallableRoot::ParameterDefault(unit.parameter) {
            return ControlSignal::missing_checked_fact(
                "default entry is executing under a different checked expression root",
                span,
            );
        }
        let frame = match self.parameter_default_frame(&unit, bindings, slots, values, span) {
            Ok(frame) => frame,
            Err(fault) => return ControlSignal::Fault(Box::new(fault)),
        };
        ControlSignal::Expr(Box::new(PendingExpr {
            expr: unit.expression,
            frame,
            continuation: Continuation::BlockValue,
        }))
    }

    fn parameter_default_frame(
        &self,
        unit: &etas_frontend::code::parameter_defaults::ParameterDefaultUnit,
        bindings: GenericBindings,
        slots: &[u32],
        values: &[InterpValue],
        span: Span,
    ) -> Result<Frame, ExecutionFault> {
        let mut frame = self.callable_frame_with_generics(unit.scope, bindings, span)?;
        for (slot, symbol) in self
            .checked
            .hir
            .preceding_parameters(unit.parameter)
            .expect("checked default parameter list")
            .iter()
            .enumerate()
        {
            if let Some(index) = slots.iter().position(|bound| *bound as usize == slot) {
                frame.insert(*symbol, values[index].clone());
            }
        }
        Ok(frame)
    }
}

pub(crate) fn default_argument_bindings(
    checked: &etas_frontend::CheckedProject,
    call: HirExprId,
    parameter: etas_hir::SymbolId,
    outer: &GenericBindings,
    span: Span,
) -> Result<GenericBindings, ExecutionFault> {
    let missing =
        |reason| ExecutionFault::new(AnalysisDiagnosticCode::MissingCheckedFact, span, reason);
    let call_bindings = CheckedCallBindings::resolve(checked, call, outer).map_err(missing)?;
    let mut bindings = GenericBindings {
        types: call_bindings.types.into_iter().collect(),
        parameters: call_bindings.parameters.into_iter().collect(),
        rows: call_bindings.rows.into_iter().collect(),
    };
    let Some(etas_hir::SymbolDef::Param { owner, .. }) =
        checked.hir.symbols.get(parameter).map(|symbol| &symbol.def)
    else {
        return Err(missing("default origin is not a checked parameter".into()));
    };
    if let Some(etas_hir::HirItem::Spec(spec)) = checked.hir.items.get(*owner) {
        let method = checked
            .types
            .checked_method_calls
            .get(&call)
            .ok_or_else(|| missing("spec default requires its checked method evidence".into()))?;
        let (subject, arguments) = match &method.evidence {
            etas_types::method::MethodEvidence::Implementation { subject, args, .. }
            | etas_types::method::MethodEvidence::Bound { subject, args, .. } => (*subject, args),
            etas_types::method::MethodEvidence::Inherent => {
                return Err(missing("spec default has no spec evidence".into()));
            }
        };
        let mut substitution = outer.substitution(&checked.type_store);
        for (symbol, argument) in spec.type_params.iter().zip(arguments) {
            let argument = substitution
                .substitute(*argument)
                .map_err(|error| missing(error.to_string()))?;
            bind_source_parameter(checked, &mut bindings, *symbol, argument);
        }
        if let Some(receiver) = spec.items.iter().find_map(|member| match member {
            etas_hir::HirSpecItem::FlowSignature(signature)
                if signature.params.contains(&parameter) =>
            {
                signature.receiver_type_param
            }
            _ => None,
        }) {
            let subject = substitution
                .substitute(subject)
                .map_err(|error| missing(error.to_string()))?;
            bind_source_parameter(checked, &mut bindings, receiver, subject);
        }
        if let Some(signature) = spec.items.iter().find_map(|member| match member {
            etas_hir::HirSpecItem::FlowSignature(signature)
                if signature.params.contains(&parameter) =>
            {
                Some(signature)
            }
            _ => None,
        }) {
            for (symbol, actual) in signature
                .type_params
                .iter()
                .zip(&method.signature.generic_params)
            {
                if let Some(argument) = bindings.types.get(&actual.name).copied() {
                    bind_source_parameter(checked, &mut bindings, *symbol, argument);
                }
            }
        }
    }
    Ok(bindings)
}
pub(crate) fn bind_source_parameter(
    checked: &etas_frontend::CheckedProject,
    bindings: &mut GenericBindings,
    symbol: etas_hir::SymbolId,
    argument: etas_types::TypeId,
) {
    let declared = checked
        .hir
        .symbols
        .get(symbol)
        .expect("checked generic parameter");
    bindings.types.insert(declared.name.clone(), argument);
    let etas_types::SymbolTypeFact::Param { ty } = checked
        .types
        .symbol_types
        .get(&symbol)
        .expect("checked source generic parameter type")
    else {
        unreachable!("generic parameter type fact")
    };
    bindings.parameters.insert(*ty, argument);
}

pub(super) fn dense_arguments(
    count: usize,
    slots: Vec<u32>,
    values: Vec<InterpValue>,
) -> Result<Vec<InterpValue>, String> {
    if values.len() == count
        && slots.len() == count
        && slots
            .iter()
            .enumerate()
            .all(|(index, slot)| index == *slot as usize)
    {
        return Ok(values);
    }
    let mut arguments = vec![None; count];
    for (slot, value) in slots.into_iter().zip(values) {
        arguments[slot as usize] = Some(value);
    }
    arguments
        .into_iter()
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| "invocation arguments do not cover the checked parameter list".into())
}
