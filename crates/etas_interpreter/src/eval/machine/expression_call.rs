use std::sync::Arc;

use etas_code::descriptor::ParameterDefaultInfo;
use etas_hir::HirExprId;

use crate::{
    control::{CallSite, CallSiteKind, GenericBindings},
    plan::code::RevisionProjection,
    value::{CheckedExecutionProgram, InterpValue},
};

mod handler;
mod imported;
pub(crate) use handler::HandlerApplication;

#[derive(Clone, Debug)]
pub(crate) struct PreparedExpression {
    pub(crate) handler_projections: Vec<RevisionProjection>,
    pub(crate) destination: CheckedExecutionProgram,
    pub(crate) bindings: GenericBindings,
    pub(crate) output: etas_types::TypeId,
    pub(crate) projection: Option<ExpressionProjection>,
}

impl PreparedExpression {
    pub(crate) fn action_projections(&self) -> Vec<RevisionProjection> {
        if !self.handler_projections.is_empty() {
            return self.handler_projections.clone();
        }
        self.projection
            .iter()
            .map(|projection| projection.edge.clone())
            .collect()
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ExpressionProjection {
    /// The result direction: defining program to caller program.
    edge: RevisionProjection,
    scope: etas_code::descriptor::GenericOwner,
    arguments: Vec<etas_types::TypeId>,
}

/// Invocation of an authored expression unit. It has no first-class flow value
/// or synthetic declaration; its lexical environment belongs to its code owner.
#[derive(Clone, Debug)]
pub(crate) enum ExpressionUnit {
    Default(ParameterDefaultInfo),
    Handler(Box<HandlerApplication>),
}

#[derive(Clone, Debug)]
pub(crate) struct ExpressionCall {
    pub(crate) source: CheckedExecutionProgram,
    pub(crate) destination: CheckedExecutionProgram,
    pub(crate) unit: ExpressionUnit,
    pub(crate) site: CallSite,
    pub(crate) slots: Vec<u32>,
    pub(crate) bindings: GenericBindings,
    output: etas_types::TypeId,
    pub(crate) projection: Option<ExpressionProjection>,
}

impl ExpressionCall {
    pub(crate) fn is_default(&self) -> bool {
        matches!(self.unit, ExpressionUnit::Default(_))
    }

    pub(crate) fn control(
        &self,
        program: &CheckedExecutionProgram,
        input: crate::control::ContinuationInput,
    ) -> Result<crate::control::ContinuationInput, String> {
        self.check_destination(program)?;
        match &self.unit {
            ExpressionUnit::Handler(handler) => handler.control(input, program),
            ExpressionUnit::Default(_) => {
                Err("lexical control escaped a default expression".into())
            }
        }
    }

    pub(crate) fn enter(
        &self,
        ctx: &mut crate::eval::EvalContext<'_>,
        args: &[InterpValue],
        span: etas_core::Span,
    ) -> crate::control::ControlSignal {
        match &self.unit {
            ExpressionUnit::Default(reference) => ctx.enter_parameter_default(
                reference,
                self.bindings.clone(),
                &self.slots,
                args,
                span,
            ),
            ExpressionUnit::Handler(handler) => handler.enter(self, ctx, span),
        }
    }

    pub(crate) fn presented_parameter_type(
        checked: &etas_frontend::CheckedProgram,
        reference: &ParameterDefaultInfo,
        expr: HirExprId,
        slot: u32,
        outer: &GenericBindings,
    ) -> Result<etas_types::TypeId, String> {
        imported::presented_parameter_type(checked, reference, expr, slot, outer)
    }

    pub(crate) fn at_site(
        source: CheckedExecutionProgram,
        reference: ParameterDefaultInfo,
        site: CallSite,
        slots: Vec<u32>,
    ) -> Result<Self, String> {
        let CallSiteKind::Expr { expr } = site.kind else {
            return Err("default expression requires an actual invocation expression".into());
        };
        if site.receiver_update.is_some()
            || site.composition.is_some()
            || site.task_callback.is_some()
        {
            return Err(
                "default expression cannot carry a receiver update or callable composition".into(),
            );
        }
        let prepared = Self::prepare(
            &source,
            &reference,
            expr,
            &slots,
            site.frame.generic_bindings(),
        )?;
        Ok(Self {
            source,
            destination: prepared.destination,
            unit: ExpressionUnit::Default(reference),
            site,
            slots,
            bindings: prepared.bindings,
            output: prepared.output,
            projection: prepared.projection,
        })
    }

    pub(crate) fn prepare(
        source: &CheckedExecutionProgram,
        reference: &ParameterDefaultInfo,
        expr: HirExprId,
        slots: &[u32],
        outer: &GenericBindings,
    ) -> Result<PreparedExpression, String> {
        if !declares_default(source, expr, reference) {
            return Err("expression invocation differs from its checked default argument".into());
        }
        let Some(unit) = source.plan.arguments.default_unit(reference) else {
            return imported::prepare(source, reference, expr, slots, outer);
        };
        let parameters = source
            .checked
            .hir
            .preceding_parameters(unit.parameter)
            .ok_or("default expression has no declared parameter environment")?;
        let mut seen = std::collections::BTreeSet::new();
        if slots
            .iter()
            .any(|slot| *slot as usize >= parameters.len() || !seen.insert(*slot))
        {
            return Err("default expression has inconsistent preceding argument positions".into());
        }
        let span = source.checked.hir.exprs[expr].span(&source.checked.hir.blocks);
        let bindings = crate::eval::arguments::default_argument_bindings(
            &source.checked,
            expr,
            unit.parameter,
            outer,
            span,
        )
        .map_err(|fault| fault.message)?;
        let plan = source
            .plan
            .select_callable(
                &source.checked,
                etas_frontend::CheckedCallableRoot::ParameterDefault(unit.parameter),
            )
            .map_err(|errors| {
                format!("default expression has no checked execution plan: {errors:?}")
            })?;
        let mut destination = source.clone();
        destination.plan = Arc::new(plan);
        let output = bindings
            .substitution(&destination.checked.type_store)
            .substitute(unit.output)
            .map_err(|error| error.to_string())?;
        Ok(PreparedExpression {
            handler_projections: Vec::new(),
            destination,
            bindings,
            output,
            projection: None,
        })
    }

    pub(crate) fn arguments(
        &self,
        source: &CheckedExecutionProgram,
        args: Vec<InterpValue>,
    ) -> Result<Vec<InterpValue>, String> {
        self.check_source(source)?;
        if matches!(self.unit, ExpressionUnit::Handler(_)) {
            if !args.is_empty() {
                return Err("Handler expression has no positional argument slots".into());
            }
            return Ok(args);
        }
        if args.len() != self.slots.len() {
            return Err("default expression arguments differ from their checked positions".into());
        }
        match &self.projection {
            Some(projection) => args
                .into_iter()
                .zip(&projection.arguments)
                .map(|(value, native)| {
                    let presented = projection.edge.type_in_scope(&projection.scope, *native)?;
                    projection.edge.reversed().value_in_scope(
                        &projection.scope,
                        presented,
                        value,
                        source,
                    )
                })
                .collect(),
            None => Ok(args),
        }
    }

    pub(crate) fn result(
        &self,
        program: &CheckedExecutionProgram,
        value: InterpValue,
    ) -> Result<InterpValue, String> {
        self.check_destination(program)?;
        if let ExpressionUnit::Handler(handler) = &self.unit {
            let kind = value.kind_name();
            return handler
                .result(self.output, value, program)
                .map_err(|error| {
                    format!(
                        "Handler expression result (body {}, output {:?}, kind {kind}): {error}",
                        handler.body, self.output
                    )
                });
        }
        match &self.projection {
            Some(projection) => {
                projection
                    .edge
                    .value_in_scope(&projection.scope, self.output, value, program)
            }
            None => Ok(value),
        }
    }

    pub(crate) fn error(
        &self,
        program: &CheckedExecutionProgram,
        ty: etas_types::TypeId,
        value: InterpValue,
    ) -> Result<(etas_types::TypeId, InterpValue), String> {
        self.check_destination(program)?;
        if let ExpressionUnit::Handler(handler) = &self.unit {
            return handler.project_outgoing(ty, value, program);
        }
        match &self.projection {
            Some(projection) => projection.edge.value(ty, value, program),
            None => Ok((ty, value)),
        }
    }

    pub(crate) fn action_projections(&self) -> Vec<RevisionProjection> {
        if let ExpressionUnit::Handler(handler) = &self.unit {
            return handler.action_projections();
        }
        self.projection
            .iter()
            .map(|projection| projection.edge.clone())
            .collect()
    }

    fn check_destination(&self, program: &CheckedExecutionProgram) -> Result<(), String> {
        if !Arc::ptr_eq(&program.checked, &self.destination.checked)
            || program.plan.root != self.destination.plan.root
        {
            return Err("default expression result belongs to a different execution root".into());
        }
        Ok(())
    }

    pub(crate) fn check_source(&self, program: &CheckedExecutionProgram) -> Result<(), String> {
        if !Arc::ptr_eq(&program.checked, &self.source.checked)
            || program.plan.root != self.source.plan.root
        {
            return Err(
                "default expression invocation belongs to a different caller program".into(),
            );
        }
        Ok(())
    }
}

fn declares_default(
    program: &CheckedExecutionProgram,
    expr: HirExprId,
    reference: &ParameterDefaultInfo,
) -> bool {
    program
        .checked
        .types
        .checked_method_calls
        .get(&expr)
        .map(|call| call.defaults.as_slice())
        .or_else(|| {
            program
                .checked
                .types
                .declared_call_arguments
                .get(&expr)
                .map(|call| call.defaults.as_slice())
        })
        .is_some_and(|defaults| {
            defaults.iter().any(|default| match &default.source {
                etas_types::ParameterDefaultRef::Source { parameter } => {
                    program.plan.arguments.source_default_reference(*parameter) == Some(reference)
                }
                etas_types::ParameterDefaultRef::External {
                    reference: candidate,
                } => candidate == reference,
            })
        })
}
