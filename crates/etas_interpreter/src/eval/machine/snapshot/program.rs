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
    orchestration::{CallSiteSnapshot, CallTargetSnapshot, ProgramReturnSnapshot},
    value::CheckedExecutionProgram,
};

pub(in crate::eval::machine) fn capture_scopes(
    machine: &EvalMachine,
) -> Result<BTreeMap<usize, ProgramScope>, String> {
    let mut scope = machine.program_scope.clone();
    let mut scopes = BTreeMap::new();
    for (index, frame) in machine.stack.iter().enumerate().rev() {
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
                .or_else(|| machine.checkpoint_root.as_ref().map(|root| &root.program))
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
    if scope.is_some() {
        return Err("active program scope has no return boundary".into());
    }
    Ok(scopes)
}

pub(in crate::eval::machine) fn capture(
    frame: &crate::eval::machine::frame::ProgramReturnFrame,
    destination: &ProgramScope,
    context: &mut CaptureContext,
) -> Result<ProgramReturnSnapshot, String> {
    let target = frame.call.call_target();
    let call_site = CallSiteSnapshot::capture(
        frame
            .site
            .as_ref()
            .ok_or("program boundary has no checked invocation origin")?,
        context,
    )?;
    let target = context.call_target(&target)?;
    validate_specialization(
        &frame.parent_program.checked,
        &call_site,
        frame.span,
        &target,
    )?;
    if !validate_composed_site(
        &frame.parent_program.checked,
        &call_site,
        frame.span,
        &target,
    )? {
        validate_call_site(
            &frame.parent_program.checked,
            &call_site,
            frame.span,
            &frame.call,
        )?;
    }
    Ok(ProgramReturnSnapshot {
        call: target,
        call_site,
        handler_floor: destination.handler_floor,
        outer: context.continuation(&frame.outer)?,
        parent_model_policy: capture_model_policy(&frame.parent_model_policy),
        span: frame.span,
        retry_depth: frame.retry_depth,
    })
}

pub(crate) fn resolve_destination(
    snapshot: &ProgramReturnSnapshot,
    parent: &CheckedProgram,
    revisions: Option<&crate::api::code::PreparedRevisionSet>,
    limits: &etas_host::StorageLimits,
) -> Result<CheckedExecutionProgram, String> {
    let revisions =
        revisions.ok_or("active program checkpoint requires freshly prepared revisions")?;
    let composed =
        validate_composed_site(parent, &snapshot.call_site, snapshot.span, &snapshot.call)?;
    validate_specialization(parent, &snapshot.call_site, snapshot.span, &snapshot.call)?;
    let target: &CallTargetSnapshot = match &snapshot.call {
        CallTargetSnapshot::Specialized { target, .. } => target,
        target => target,
    };
    match target {
        CallTargetSnapshot::AgentRevision(reference) => {
            let mut target =
                crate::control::CallTarget::AgentRevision(revisions.restore(reference, parent)?);
            if let CallTargetSnapshot::Specialized {
                type_bindings,
                effect_row_bindings,
                ..
            } = &snapshot.call
            {
                target = crate::control::CallTarget::Specialized {
                    target: target.into(),
                    type_bindings: type_bindings.clone(),
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
            super::SnapshotValidator::new(
                &native.checked,
                &native.plan.slots,
                &native.plan.dispatch,
                &native.plan.closures,
                limits,
            )
            .with_revisions(Some(revisions))
            .validate_callable_target(target)?;
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

pub(in crate::eval::machine) fn action_projections(
    snapshot: &ProgramReturnSnapshot,
    parent: &CheckedProgram,
    revisions: &crate::api::code::PreparedRevisionSet,
) -> Result<Vec<crate::plan::code::RevisionProjection>, String> {
    let target: &CallTargetSnapshot = match &snapshot.call {
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
        let bindings = site.frame.generic_bindings();
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
    let (_, expression) = site.receiver(parent, span)?;
    let expected = expression
        .map(|expr| {
            crate::control::CheckedCallBindings::resolve(
                parent,
                expr,
                &site.frame.generic_bindings(),
            )
        })
        .transpose()?;
    let expected_types = expected
        .as_ref()
        .map_or(&[][..], |bindings| bindings.types.as_slice());
    let expected_rows = expected
        .as_ref()
        .map_or(&[][..], |bindings| bindings.rows.as_slice());
    match target {
        CallTargetSnapshot::Specialized {
            target,
            type_bindings,
            effect_row_bindings,
        } => {
            if matches!(&**target, CallTargetSnapshot::Specialized { .. })
                || type_bindings != expected_types
                || effect_row_bindings != expected_rows
            {
                return Err(
                    "program boundary specialization differs from its checked call-site bindings"
                        .into(),
                );
            }
        }
        _ if !expected_types.is_empty() || !expected_rows.is_empty() => {
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
    let (receiver, _) = site.receiver(parent, span)?;
    let expected = *parent
        .types
        .expr_types
        .get(&receiver)
        .ok_or("scoped callable boundary has no checked receiver type")?;
    let bindings = site.frame.generic_bindings();
    let expected = bindings
        .substitution(&parent.type_store)
        .substitute(expected)
        .map_err(|error| error.to_string())?;
    if expected != ty {
        return Err("scoped callable boundary differs from its checked call site type".into());
    }
    Ok(())
}
