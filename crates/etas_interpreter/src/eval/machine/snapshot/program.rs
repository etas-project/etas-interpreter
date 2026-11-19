use std::{collections::BTreeMap, sync::Arc};

use etas_frontend::CheckedProgram;

use super::CaptureContext;
pub(in crate::eval::machine) use super::{capture_model_policy, restore_model_policy};
use crate::{
    eval::machine::{
        EvalMachine,
        frame::EvalFrame,
        scoped_call::{ProgramCall, ProgramCallTarget},
        state::ProgramScope,
    },
    orchestration::{
        CallSiteSnapshot, CallTargetSnapshot, ExpressionCallSnapshot, ProgramCallSnapshot,
        ProgramReturnSnapshot,
    },
    value::CheckedExecutionProgram,
};

pub(in crate::eval::machine) fn capture_scopes(
    machine: &EvalMachine,
) -> Result<BTreeMap<usize, ProgramScope>, String> {
    capture_segment_scopes(
        &machine.stack,
        machine.program_scope.clone(),
        machine.checkpoint_root.as_ref().map(|root| &root.program),
        None,
    )
}

pub(in crate::eval::machine) fn capture_segment_scopes(
    frames: &[EvalFrame],
    mut scope: Option<ProgramScope>,
    root: Option<&CheckedExecutionProgram>,
    base: Option<&ProgramScope>,
) -> Result<BTreeMap<usize, ProgramScope>, String> {
    let mut scopes = BTreeMap::new();
    for (index, frame) in frames.iter().enumerate().rev() {
        if let EvalFrame::ProgramReturn(frame) = frame {
            let active = scope.take().ok_or("program boundary has no active scope")?;
            let destination = frame.call.destination()?;
            if !Arc::ptr_eq(&active.program.checked, &destination.checked)
                || !Arc::ptr_eq(&active.program.plan, &destination.plan)
            {
                return Err("program boundary does not own the active native program".into());
            }
            let parent = frame
                .parent
                .as_ref()
                .map(|scope| &scope.program)
                .or(root)
                .ok_or("program boundary has no pinned root owner")?;
            if !Arc::ptr_eq(&parent.checked, &frame.parent_program.checked)
                || !Arc::ptr_eq(&parent.plan, &frame.parent_program.plan)
            {
                return Err(
                    "program boundary parent differs from its pinned execution owner".into(),
                );
            }
            scopes.insert(index, active);
            scope = frame.parent.clone();
        }
    }
    let matches_base = match (&scope, base) {
        (None, None) => true,
        (Some(actual), Some(expected)) => {
            actual.handler_floor == expected.handler_floor
                && Arc::ptr_eq(&actual.program.checked, &expected.program.checked)
                && Arc::ptr_eq(&actual.program.plan, &expected.program.plan)
        }
        _ => false,
    };
    if !matches_base {
        return Err("active program scope has no return boundary".into());
    }
    Ok(scopes)
}

pub(in crate::eval::machine) fn capture(
    frame: &crate::eval::machine::frame::ProgramReturnFrame,
    destination: &ProgramScope,
    context: &mut CaptureContext,
) -> Result<ProgramReturnSnapshot, String> {
    let call_site = CallSiteSnapshot::capture(
        frame
            .site
            .as_ref()
            .ok_or("program boundary has no checked invocation origin")?,
        context,
    )?;
    let target = capture_call(&frame.call, context)?;
    if let ProgramCallSnapshot::Callable(target) = &target {
        validate_specialization(
            &frame.parent_program.checked,
            &call_site,
            frame.span,
            target,
        )?;
        if !validate_composed_site(
            &frame.parent_program.checked,
            &call_site,
            frame.span,
            target,
        )? {
            validate_call_site(
                &frame.parent_program.checked,
                &call_site,
                frame.span,
                &frame.call,
            )?;
        }
    }
    Ok(ProgramReturnSnapshot {
        call: target,
        call_site,
        handler_floor: destination.handler_floor,
        outer: context.continuation(&frame.outer)?,
        parent_model_policy: capture_model_policy(&frame.parent_model_policy),
        parent_agent_context: frame.parent_agent_context.as_ref().map(|request| {
            Box::new(super::capture_agent_context(
                request,
                &mut context.revisions,
            ))
        }),
        span: frame.span,
        retry_depth: frame.retry_depth,
    })
}

pub(crate) fn resolve_destination(
    snapshot: &ProgramReturnSnapshot,
    parent: &CheckedProgram,
    revisions: Option<&crate::api::code::PreparedRevisionSet>,
) -> Result<CheckedExecutionProgram, String> {
    if let ProgramCallSnapshot::Expression(expression) = &snapshot.call {
        if expression.site.kind != snapshot.call_site.kind
            || expression.site.frame.id != snapshot.call_site.frame.id
        {
            return Err(
                "default expression boundary has inconsistent caller frame identity".into(),
            );
        }
        return expression_destination(expression, parent, revisions);
    }
    let ProgramCallSnapshot::Callable(saved_call) = &snapshot.call else {
        unreachable!()
    };
    let revisions =
        revisions.ok_or("active program checkpoint requires freshly prepared revisions")?;
    let composed = validate_composed_site(parent, &snapshot.call_site, snapshot.span, saved_call)?;
    validate_specialization(parent, &snapshot.call_site, snapshot.span, saved_call)?;
    let target: &CallTargetSnapshot = match saved_call {
        CallTargetSnapshot::Specialized { target, .. } => target,
        target => target,
    };
    match target {
        CallTargetSnapshot::AgentRevision(reference) => {
            let mut target =
                crate::control::CallTarget::AgentRevision(revisions.restore(reference, parent)?);
            if let CallTargetSnapshot::Specialized {
                type_bindings,
                type_parameter_bindings,
                effect_row_bindings,
                ..
            } = saved_call
            {
                target = crate::control::CallTarget::Specialized {
                    target: target.into(),
                    type_bindings: type_bindings.clone(),
                    type_parameter_bindings: type_parameter_bindings.clone(),
                    effect_row_bindings: effect_row_bindings.clone(),
                };
            }
            let call = ProgramCall::restore_target(target, parent)?;
            if !composed {
                validate_call_site(parent, &snapshot.call_site, snapshot.span, &call)?;
            }
            call.destination()
        }
        CallTargetSnapshot::Scoped { metadata, target } => {
            let projections = revisions.callable_projections(metadata, parent)?;
            let mut native = revisions.execution_callable(metadata, target)?;
            let root = crate::value::ScopedCallableValue::checked_snapshot_root(
                &native,
                metadata.native_type,
                target,
            )?;
            let ty = projections
                .last()
                .map_or(metadata.native_type, |edge| edge.target().1);
            if !composed {
                validate_callable_site(parent, &snapshot.call_site, snapshot.span, ty)?;
            }
            native.plan = Arc::new(native.plan.select_callable(&native.checked, root).map_err(
                |errors| format!("checkpoint callable plan is not executable: {errors:?}"),
            )?);
            Ok(native)
        }
        _ => Err("checkpoint program boundary has no checked revision callable".into()),
    }
}

fn validate_composed_site(
    parent: &CheckedProgram,
    site: &CallSiteSnapshot,
    span: etas_core::Span,
    target: &CallTargetSnapshot,
) -> Result<bool, String> {
    let Some(composition) = &site.composition else {
        return Ok(false);
    };
    site.receiver(parent, span)?;
    if composition.selected()? != target {
        return Err("program boundary differs from its selected composition stage".into());
    }
    Ok(true)
}

pub(in crate::eval::machine) fn capture_expression(
    call: &super::super::expression_call::ExpressionCall,
    context: &mut CaptureContext,
) -> Result<ExpressionCallSnapshot, String> {
    use crate::eval::machine::expression_call::ExpressionUnit;
    use crate::orchestration::ExpressionUnitSnapshot;
    let unit = match &call.unit {
        ExpressionUnit::Default(reference) => ExpressionUnitSnapshot::Default(reference.clone()),
        ExpressionUnit::Handler(handler) => ExpressionUnitSnapshot::Handler {
            caller: context.execution_identity(&handler.caller)?,
            site: CallSiteSnapshot::capture(&handler.site, context)?,
            handler: crate::orchestration::SnapshotBox::new(context.value(
                &crate::value::InterpValue::ScopedHandler(Box::new(handler.handler.clone())),
            )?),
            body: handler.body,
        },
    };
    Ok(ExpressionCallSnapshot {
        source: context.execution_identity(&call.source)?,
        unit,
        site: CallSiteSnapshot::capture(&call.site, context)?,
        slots: call.slots.clone(),
    })
}

pub(in crate::eval::machine) fn capture_call(
    call: &ProgramCall,
    context: &mut CaptureContext,
) -> Result<ProgramCallSnapshot, String> {
    Ok(match call.target() {
        ProgramCallTarget::Expression(expression) => {
            ProgramCallSnapshot::Expression(Box::new(capture_expression(expression, context)?))
        }
        _ => ProgramCallSnapshot::Callable(
            context.call_target(
                &call
                    .call_target()
                    .expect("callable boundary has a callable target"),
            )?,
        ),
    })
}

pub(in crate::eval::machine) fn restore_expression(
    saved: ExpressionCallSnapshot,
    context: &mut super::RestoreContext,
    parent: &CheckedExecutionProgram,
) -> Result<super::super::expression_call::ExpressionCall, String> {
    let source = CheckedExecutionProgram::restore_identity(
        &saved.source,
        parent,
        context.revisions.as_deref(),
    )?;
    if !Arc::ptr_eq(&source.checked, &parent.checked) || source.plan.root != parent.plan.root {
        return Err("saved default expression has a different caller execution root".into());
    }
    use crate::orchestration::ExpressionUnitSnapshot;
    match saved.unit {
        ExpressionUnitSnapshot::Default(reference) => {
            let site = saved.site.restore(context)?;
            super::super::expression_call::ExpressionCall::at_site(
                parent.clone(),
                reference,
                site,
                saved.slots,
            )
        }
        ExpressionUnitSnapshot::Handler {
            caller,
            site,
            handler,
            body,
        } => {
            let caller = CheckedExecutionProgram::restore_identity(
                &caller,
                parent,
                context.revisions.as_deref(),
            )?;
            let mut related = context.related_program(caller.clone());
            let site = site.restore(&mut related)?;
            let crate::value::InterpValue::ScopedHandler(handler) =
                handler.into_value().restore_with(&mut related)?
            else {
                return Err("Handler expression has no scoped Handler value".into());
            };
            let call = super::super::expression_call::HandlerApplication {
                caller,
                site,
                handler: *handler,
                body,
            }
            .expression()?;
            call.check_source(parent)?;
            Ok(call)
        }
    }
}

pub(in crate::eval::machine) fn restore_call(
    saved: ProgramCallSnapshot,
    context: &mut super::RestoreContext,
    parent: &CheckedExecutionProgram,
) -> Result<ProgramCall, String> {
    match saved {
        ProgramCallSnapshot::Callable(target) => {
            ProgramCall::restore_target(context.restore_call_target(target)?, &parent.checked)
        }
        ProgramCallSnapshot::Expression(expression) => Ok(ProgramCall::expression(
            restore_expression(*expression, context, parent)?,
        )),
    }
}

pub(in crate::eval::machine) fn expression_destination(
    saved: &ExpressionCallSnapshot,
    parent: &CheckedProgram,
    revisions: Option<&crate::api::code::PreparedRevisionSet>,
) -> Result<CheckedExecutionProgram, String> {
    prepare_expression(saved, parent, revisions).map(|prepared| prepared.destination)
}

fn prepare_expression(
    saved: &ExpressionCallSnapshot,
    parent: &CheckedProgram,
    revisions: Option<&crate::api::code::PreparedRevisionSet>,
) -> Result<super::super::expression_call::PreparedExpression, String> {
    let source = if let Some(revisions) = revisions {
        revisions.execution_snapshot(&saved.source)?
    } else {
        if !matches!(
            saved.source.program,
            crate::orchestration::CheckpointProgramReference::Source
        ) {
            return Err("default expression code owner requires prepared revisions".into());
        }
        let prepared = crate::plan::prepare_program(parent).map_err(|errors| {
            format!("default expression source has no checked plan: {errors:?}")
        })?;
        use crate::orchestration::ExecutionRootSnapshot as R;
        let root = match &saved.source.root {
            R::Item(item) => etas_frontend::CheckedCallableRoot::Item(*item),
            R::Lambda(expr) => etas_frontend::CheckedCallableRoot::Lambda(*expr),
            R::StdReference(expr) => etas_frontend::CheckedCallableRoot::StdReference(*expr),
            R::ParameterDefault(reference) => etas_frontend::CheckedCallableRoot::ParameterDefault(
                prepared
                    .parameter_default(reference)
                    .ok_or("default caller root has no authored revision")?
                    .parameter,
            ),
        };
        CheckedExecutionProgram {
            checked: parent.clone(),
            plan: Arc::new(prepared.select_callable(root).map_err(|errors| {
                format!("default caller root has no checked plan: {errors:?}")
            })?),
            definitions: None,
            revision: None,
            foreign: None,
        }
    };
    if !Arc::ptr_eq(&source.checked, parent) {
        return Err("default expression snapshot has a different caller code owner".into());
    }
    let crate::control::CallSiteKind::Expr { expr } = saved.site.kind else {
        return Err("default expression snapshot has no invocation expression".into());
    };
    match &saved.unit {
        crate::orchestration::ExpressionUnitSnapshot::Default(reference) => {
            super::super::expression_call::ExpressionCall::prepare(
                &source,
                reference,
                expr,
                &saved.slots,
                &saved.site.bindings(parent),
            )
        }
        crate::orchestration::ExpressionUnitSnapshot::Handler {
            caller,
            site,
            handler,
            body,
        } => {
            let revisions =
                revisions.ok_or("Handler expression requires freshly prepared program owners")?;
            let caller = revisions.execution_snapshot(caller)?;
            let crate::orchestration::ValueSnapshot::ScopedHandler { metadata, value } =
                handler.as_ref()
            else {
                return Err("Handler expression has no scoped Handler metadata".into());
            };
            let (native, projections) = metadata.resolve(revisions)?;
            let expected_source = if *body { &native } else { &caller };
            if !std::sync::Arc::ptr_eq(&expected_source.checked, parent)
                || expected_source.plan.root != source.plan.root
                || !saved.slots.is_empty()
            {
                return Err("Handler expression has a different checked source root".into());
            }
            let crate::control::CallSiteKind::Expr { expr: handle_expr } = site.kind else {
                return Err("Handler expression has no checked handle site".into());
            };
            let Some(etas_hir::HirExpr::Handle {
                body: body_expr,
                handler: handler_expr,
                ..
            }) = caller.checked.hir.exprs.get(handle_expr)
            else {
                return Err("Handler expression origin is not a handle expression".into());
            };
            let expected_site = if *body {
                let crate::orchestration::ValueSnapshot::Handler { fact_expr, .. } = value.as_ref()
                else {
                    return Err("Handler body has no retained native literal".into());
                };
                *fact_expr
            } else {
                handle_expr
            };
            if expr != expected_site {
                return Err("Handler boundary differs from its checked expression site".into());
            }
            let fact = caller
                .checked
                .effects
                .handle_applications
                .get(&handle_expr)
                .ok_or("Handler expression has no checked application contract")?;
            if fact.handler
                != (etas_effects::HandlerValueRef::Expr {
                    expr: *handler_expr,
                })
            {
                return Err("Handler expression changes its checked Handler input".into());
            }
            let bindings = site.bindings(&caller.checked);
            let mut output = bindings
                .substitution(&caller.checked.type_store)
                .substitute(
                    *caller
                        .checked
                        .types
                        .expr_types
                        .get(if *body { body_expr } else { &handle_expr })
                        .ok_or("handled expression has no checked output")?,
                )
                .map_err(|error| error.to_string())?;
            let mut program = &native.checked;
            let mut handler_type = metadata.native_type;
            for edge in &projections {
                if !std::sync::Arc::ptr_eq(program, edge.source()) {
                    return Err("Handler expression projection has a different source owner".into());
                }
                handler_type = edge.runtime_type(handler_type)?;
                program = edge.target();
            }
            let expected = bindings
                .substitution(&caller.checked.type_store)
                .substitute(caller.checked.types.expr_types[handler_expr])
                .map_err(|error| error.to_string())?;
            if !std::sync::Arc::ptr_eq(program, &caller.checked) || handler_type != expected {
                return Err("Handler expression changes its checked receiving interface".into());
            }
            let edges = if *body {
                projections
                    .iter()
                    .rev()
                    .map(crate::plan::code::RevisionProjection::reversed)
                    .collect()
            } else {
                projections.clone()
            };
            if !*body {
                for edge in projections
                    .iter()
                    .rev()
                    .map(crate::plan::code::RevisionProjection::reversed)
                {
                    output = edge.runtime_type(output)?;
                }
            }
            Ok(super::super::expression_call::PreparedExpression {
                destination: if *body { caller } else { native },
                output,
                bindings: crate::control::GenericBindings::new(),
                projection: None,
                handler_projections: edges,
            })
        }
    }
}

pub(in crate::eval::machine) fn action_projections(
    snapshot: &ProgramReturnSnapshot,
    parent: &CheckedProgram,
    revisions: &crate::api::code::PreparedRevisionSet,
) -> Result<Vec<crate::plan::code::RevisionProjection>, String> {
    let saved_call = match &snapshot.call {
        ProgramCallSnapshot::Callable(saved_call) => saved_call,
        ProgramCallSnapshot::Expression(expression) => {
            return Ok(
                prepare_expression(expression, parent, Some(revisions))?.action_projections()
            );
        }
    };
    let target: &CallTargetSnapshot = match saved_call {
        CallTargetSnapshot::Specialized { target, .. } => target,
        target => target,
    };
    match target {
        CallTargetSnapshot::AgentRevision(reference) => {
            Ok(revisions.restore(reference, parent)?.action_projections())
        }
        CallTargetSnapshot::Scoped { metadata, .. } => revisions
            .callable_projections(metadata, parent)?
            .iter()
            .map(|projection| projection.action_projection())
            .filter_map(Result::transpose)
            .collect(),
        _ => Err("suspended action has no checked native call boundary".into()),
    }
}

fn validate_call_site(
    parent: &CheckedProgram,
    site: &CallSiteSnapshot,
    saved_span: etas_core::Span,
    call: &ProgramCall,
) -> Result<(), String> {
    if let ProgramCallTarget::Callable(call) = call.target() {
        return validate_callable_site(parent, site, saved_span, call.presented_type());
    }
    let ProgramCallTarget::Agent(agent) = call.target() else {
        return Err("program boundary has no checked callable identity".into());
    };
    let (receiver, call_expr) = site.receiver(parent, saved_span)?;
    if let Some(method) = call_expr.and_then(|expr| parent.types.agent_method_calls.get(&expr)) {
        let selected = parent
            .types
            .agent_methods
            .get(method)
            .ok_or("program boundary has no checked agent method selection")?;
        if selected.component != agent.source_component()?
            || agent
                .select_method(parent, selected.agent, selected.body)?
                .checkpoint_reference()?
                != agent.checkpoint_reference()?
        {
            return Err("program boundary differs from its checked agent method".into());
        }
    } else {
        let ty = parent
            .types
            .expr_types
            .get(&receiver)
            .ok_or("program boundary callee has no checked type")?;
        let bindings = site.bindings(parent);
        let ty = bindings
            .substitution(&parent.type_store)
            .substitute(*ty)
            .map_err(|error| error.to_string())?;
        if ty != agent.source_component()? || !agent.is_default_entry() {
            return Err("program boundary differs from its checked nominal callee".into());
        }
    }
    Ok(())
}

fn validate_specialization(
    parent: &CheckedProgram,
    site: &CallSiteSnapshot,
    span: etas_core::Span,
    target: &CallTargetSnapshot,
) -> Result<(), String> {
    if matches!(
        site.kind,
        crate::control::CallSiteKind::TaskCallback { .. }
            | crate::control::CallSiteKind::SourceTaskCallback { .. }
    ) {
        site.task_callback_type(parent)?;
        return Ok(());
    }
    let (_, expression) = site.receiver(parent, span)?;
    let expected = expression
        .map(|expr| {
            crate::control::CheckedCallBindings::resolve(parent, expr, &site.bindings(parent))
        })
        .transpose()?;
    let expected_types = expected
        .as_ref()
        .map_or(&[][..], |bindings| bindings.types.as_slice());
    let expected_rows = expected
        .as_ref()
        .map_or(&[][..], |bindings| bindings.rows.as_slice());
    let expected_parameters = expected
        .as_ref()
        .map_or(&[][..], |bindings| bindings.parameters.as_slice());
    match target {
        CallTargetSnapshot::Specialized {
            target,
            type_bindings,
            type_parameter_bindings,
            effect_row_bindings,
        } => {
            if matches!(&**target, CallTargetSnapshot::Specialized { .. })
                || type_bindings != expected_types
                || type_parameter_bindings != expected_parameters
                || effect_row_bindings != expected_rows
            {
                return Err(
                    "program boundary specialization differs from its checked call-site bindings"
                        .into(),
                );
            }
        }
        _ if !expected_types.is_empty()
            || !expected_parameters.is_empty()
            || !expected_rows.is_empty() =>
        {
            return Err("program boundary is missing its checked generic specialization".into());
        }
        _ => {}
    }
    Ok(())
}

fn validate_callable_site(
    parent: &CheckedProgram,
    site: &CallSiteSnapshot,
    span: etas_core::Span,
    ty: etas_types::TypeId,
) -> Result<(), String> {
    let (expected_type, expression) = if matches!(
        site.kind,
        crate::control::CallSiteKind::TaskCallback { .. }
            | crate::control::CallSiteKind::SourceTaskCallback { .. }
    ) {
        (Some(site.task_callback_type(parent)?), None)
    } else {
        let (receiver, expression) = site.receiver(parent, span)?;
        (parent.types.expr_types.get(&receiver).copied(), expression)
    };
    let mut bindings = site.bindings(parent);
    if let Some(expression) = expression {
        let checked = crate::control::CheckedCallBindings::resolve(parent, expression, &bindings)?;
        bindings.types.extend(checked.types);
        bindings.parameters.extend(checked.parameters);
        bindings.rows.extend(checked.rows);
    }
    let (inputs, output, effects) = if let Some(method) =
        expression.and_then(|expr| parent.types.checked_method_calls.get(&expr))
    {
        let signature = &method.signature;
        (&signature.params, signature.output, &signature.effects)
    } else {
        let Some(etas_types::Type::Function(expected)) =
            expected_type.and_then(|ty| parent.type_store.get(ty))
        else {
            return Err(format!(
                "scoped callable site {:?} has no checked function ABI for {expected_type:?}",
                site.kind
            ));
        };
        (&expected.input, expected.output, &expected.effects)
    };
    let Some(etas_types::Type::Function(actual)) = parent.type_store.get(ty) else {
        return Err(format!(
            "scoped callable site {:?} has a non-function target {ty:?}",
            site.kind
        ));
    };
    let mut substitution = bindings.substitution(&parent.type_store);
    let parameters = inputs
        .iter()
        .map(|ty| {
            substitution
                .substitute(*ty)
                .map_err(|error| error.to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    let output = substitution
        .substitute(output)
        .map_err(|error| error.to_string())?;
    let actual_parameters = actual
        .input
        .iter()
        .map(|ty| {
            substitution
                .substitute(*ty)
                .map_err(|error| error.to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    let actual_output = substitution
        .substitute(actual.output)
        .map_err(|error| error.to_string())?;
    let actual_effects = actual
        .effects
        .clone()
        .map(|row| {
            substitution
                .substitute_effect_row(row)
                .map_err(|error| error.to_string())
        })
        .transpose()?;
    let effects_match = match effects {
        Some(row) => {
            Some(
                substitution
                    .substitute_effect_row(row.clone())
                    .map_err(|error| error.to_string())?,
            ) == actual_effects
        }
        None => true, // An inferred receiver row carries no explicit row bound.
    };
    if parameters != actual_parameters || output != actual_output || !effects_match {
        return Err("scoped callable boundary differs from its checked call site type".into());
    }

    Ok(())
}
