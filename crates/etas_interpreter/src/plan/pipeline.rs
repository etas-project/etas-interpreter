use std::sync::Arc;

use etas_frontend::CheckedProject;
use etas_utils::{PassControl, PassManager, Pipeline};

use crate::{
    api::{PlanOptions, PlanResult},
    diagnostics,
};

use super::{
    InterpreterPlan, action_mediation::ComputeEntryActionMediationPass, context::PlanContext,
    dispatch::BuildIntrinsicDispatchTablePass, entry::BuildEntryPlanPass,
    globals::BuildGlobalTablePass, readiness::ComputeReachableHostRequirementsPass,
    resources::BuildResourceHandleTablePass, slots::BuildSlotLayoutPass,
    validate::ValidateCheckedProjectPass,
};

pub fn build_plan(project: &CheckedProject, _options: PlanOptions) -> PlanResult {
    let mut context = PlanContext::new(project);
    let mut pipeline = plan_pipeline();
    let run = PassManager::new().run_pipeline(&mut pipeline, &mut context);
    if let PassControl::Failed(failure) = run.control {
        context
            .diagnostics
            .push(diagnostics::unhandled_runtime_error(
                diagnostics::primary_span(context.project),
                format!("interpreter plan pipeline failed: {}", failure.message),
            ));
    }

    require_plan_artifacts(&mut context);

    let plan = if context
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity == etas_core::Severity::Error)
    {
        None
    } else {
        Some(InterpreterPlan {
            entry: context.entry.expect("entry should be available"),
            slots: Arc::new(
                context
                    .slots
                    .expect("slot layout table should be available"),
            ),
            globals: context.globals.expect("global table should be available"),
            resources: context
                .resources
                .expect("resource table should be available"),
            dispatch: context
                .dispatch
                .expect("intrinsic dispatch table should be available"),
            host_requirements: context
                .host_requirements
                .expect("host requirement table should be available"),
            action_mediation: context
                .action_mediation
                .expect("action mediation table should be available"),
            diagnostics: context.diagnostics.clone(),
        })
    };

    PlanResult {
        plan,
        diagnostics: context.diagnostics,
    }
}

fn require_plan_artifacts(context: &mut PlanContext<'_>) {
    let span = diagnostics::primary_span(context.project);
    if context.entry.is_none() {
        context.diagnostics.push(diagnostics::missing_checked_fact(
            span,
            "interpreter plan is missing entry facts",
        ));
    }
    if context.slots.is_none() {
        context.diagnostics.push(diagnostics::missing_checked_fact(
            span,
            "interpreter plan is missing slot layout facts",
        ));
    }
    if context.globals.is_none() {
        context.diagnostics.push(diagnostics::missing_checked_fact(
            span,
            "interpreter plan is missing global table facts",
        ));
    }
    if context.resources.is_none() {
        context.diagnostics.push(diagnostics::missing_checked_fact(
            span,
            "interpreter plan is missing resource table facts",
        ));
    }
    if context.dispatch.is_none() {
        context.diagnostics.push(diagnostics::missing_checked_fact(
            span,
            "interpreter plan is missing intrinsic dispatch facts",
        ));
    }
    if context.host_requirements.is_none() {
        context.diagnostics.push(diagnostics::missing_checked_fact(
            span,
            "interpreter plan is missing host requirement facts",
        ));
    }
    if context.action_mediation.is_none() {
        context.diagnostics.push(diagnostics::missing_checked_fact(
            span,
            "interpreter plan is missing action mediation facts",
        ));
    }
}

fn plan_pipeline<'a>() -> Pipeline<PlanContext<'a>> {
    Pipeline::new("interpreter.plan")
        .pass(ValidateCheckedProjectPass)
        .pass(BuildEntryPlanPass)
        .pass(BuildSlotLayoutPass)
        .pass(BuildGlobalTablePass)
        .pass(BuildResourceHandleTablePass)
        .pass(BuildIntrinsicDispatchTablePass)
        .pass(ComputeReachableHostRequirementsPass)
        .pass(ComputeEntryActionMediationPass)
}
