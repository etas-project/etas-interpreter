use std::sync::Arc;

use etas_core::{Diagnostic, Severity};
use etas_frontend::{CheckedCallableRoot, CheckedProject};
use etas_utils::{PassControl, PassManager, Pipeline};

use crate::{
    api::{PlanOptions, PlanResult},
    diagnostics,
};

use super::{
    InterpreterPlan, PreparedProgram,
    action_mediation::ComputeEntryActionMediationPass,
    arguments::BuildCallArgumentsPass,
    captures::BuildClosureLayoutsPass,
    context::PlanContext,
    dispatch::BuildIntrinsicDispatchTablePass,
    entry::BuildEntryPlanPass,
    frames::BuildFrameLayoutsPass,
    globals::BuildGlobalTablePass,
    readiness::ComputeReachableHostRequirementsPass,
    records::BuildRecordLayoutsPass,
    residual::{
        BindResidualSitesPass, UsePreparedResidualPlansPass, ValidateResidualPlansPass,
        VerifyResidualCoveragePass,
    },
    resources::BuildResourceHandleTablePass,
    slots::BuildSlotLayoutPass,
    validate::ValidateCheckedProjectPass,
    variants::BuildNamedVariantLayoutsPass,
};

pub fn build_plan(project: &CheckedProject, _options: PlanOptions) -> PlanResult {
    build_plan_for_entry(project, project.entry)
}

pub(crate) fn build_plan_for_entry(
    project: &CheckedProject,
    entry: Option<etas_hir::HirItemId>,
) -> PlanResult {
    build_plan_with_residual_binding(project, entry, project.residual_plans().binding())
}

/// Plans `entry` of a program whose residual plans are bound outside it.
pub(crate) fn build_plan_with_residual_binding(
    project: &CheckedProject,
    entry: Option<etas_hir::HirItemId>,
    residual_binding: Option<&etas_frontend::CheckedResidualBinding>,
) -> PlanResult {
    let result = entry
        .ok_or_else(|| {
            vec![diagnostics::missing_entry(
                diagnostics::primary_span(project),
                "checked project does not contain a resolved entry item",
            )]
        })
        .and_then(|entry| {
            prepare_program_with_residual_binding(project, residual_binding)?
                .select_callable(CheckedCallableRoot::Item(entry))
        });
    match result {
        Ok(plan) => PlanResult {
            diagnostics: plan.diagnostics.clone(),
            plan: Some(plan),
        },
        Err(diagnostics) => PlanResult {
            plan: None,
            diagnostics,
        },
    }
}

pub(crate) fn prepare_program(
    project: &CheckedProject,
) -> Result<Arc<PreparedProgram>, Vec<Diagnostic>> {
    prepare_program_with_residual_binding(project, project.residual_plans().binding())
}

/// Prepares a program whose residual plans are bound outside it, such as an
/// emitted candidate bound to its frozen-contract revision.
pub(crate) fn prepare_program_with_residual_binding(
    project: &CheckedProject,
    residual_binding: Option<&etas_frontend::CheckedResidualBinding>,
) -> Result<Arc<PreparedProgram>, Vec<Diagnostic>> {
    prepare_program_with_layouts(project, residual_binding, None)
}

pub(super) fn prepare_program_with_layouts(
    project: &CheckedProject,
    residual_binding: Option<&etas_frontend::CheckedResidualBinding>,
    installed: Option<super::installed::InstalledLayouts>,
) -> Result<Arc<PreparedProgram>, Vec<Diagnostic>> {
    let mut context = PlanContext::new(project);
    context.residual_binding = residual_binding.cloned();
    // Layout/ABI preparation must not select main or require analysis of a
    // merely reflected declaration.
    context.requested_root = None;
    if let Some(installed) = installed {
        installed.apply(&mut context);
        run_pipeline(
            &mut Pipeline::new("interpreter.plan.load_installed_layouts")
                .pass(ValidateCheckedProjectPass)
                .pass(BuildRecordLayoutsPass)
                .pass(BuildIntrinsicDispatchTablePass)
                .pass(ValidateResidualPlansPass)
                .pass(BindResidualSitesPass),
            &mut context,
        );
    } else {
        run_pipeline(&mut layout_pipeline(), &mut context);
    }
    if context
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity == Severity::Error)
    {
        return Err(context.diagnostics);
    }
    let (
        Some(slots),
        Some(frames),
        Some(arguments),
        Some(closures),
        Some(named_variants),
        Some(records),
        Some(globals),
        Some(resources),
        Some(dispatch),
        Some(residual),
    ) = (
        context.slots,
        context.frames,
        context.arguments,
        context.closures,
        context.named_variants,
        context.records,
        context.globals,
        context.resources,
        context.dispatch,
        context.residual,
    )
    else {
        return Err(vec![diagnostics::missing_checked_fact(
            diagnostics::primary_span(project),
            "interpreter program preparation is missing layout or ABI artifacts",
        )]);
    };
    let numeric_literals = super::arithmetic::numeric_literals(project);
    Ok(Arc::new(PreparedProgram {
        expression_routes: expression_routes(project),
        materializations: Default::default(),
        sequence_layouts: Default::default(),
        enum_layouts: Default::default(),
        arithmetic: super::arithmetic::prepare(project, &numeric_literals),
        numeric_literals,
        standard_library: super::standard_library::StandardLibraryProgram::prepare(project)
            .map_err(|reason| {
                vec![diagnostics::missing_checked_fact(
                    diagnostics::primary_span(project),
                    reason,
                )]
            })?,
        program: project.clone(),
        known_std_types: crate::eval::KnownStdTypes::from_checked(project),
        spec_facts: Arc::new(std::sync::OnceLock::new()),
        slots: Arc::new(slots),
        frames: Arc::new(frames),
        arguments: Arc::new(arguments),
        closures: Arc::new(closures),
        named_variants: Arc::new(named_variants),
        records: Arc::new(records),
        globals: Arc::new(globals),
        resources: Arc::new(resources),
        dispatch,
        residual: Arc::new(residual),
        diagnostics: context.diagnostics,
    }))
}

fn expression_routes(
    project: &CheckedProject,
) -> std::collections::HashMap<etas_hir::HirExprId, super::types::ExpressionRoute> {
    use super::types::ExpressionRoute;
    use etas_hir::{HirExpr, ResolveResult, SymbolDef};
    project
        .hir
        .exprs
        .iter()
        .filter_map(|(id, expr)| {
            let route = if project
                .types
                .std_intrinsic_calls
                .get(&id)
                .is_some_and(|call| call.intrinsic == etas_std::intrinsic::runtime::TOOL_REFERENCE)
                || etas_types::source_execution::call_target(&project.hir, &project.types, id)
                    == Some(etas_std::StdExecutionRole::ToolReference)
            {
                ExpressionRoute::ToolReference
            } else if project.types.existential_packs.contains_key(&id) {
                return None;
            } else {
                match expr {
                    HirExpr::Literal(_) => ExpressionRoute::Literal,
                    HirExpr::Path(path) if !project.types.bound_method_values.contains_key(&id) => {
                        let ResolveResult::Resolved(symbol) = path.resolution else {
                            return None;
                        };
                        match project.hir.symbols.get(symbol)?.def {
                            SymbolDef::Param { .. }
                            | SymbolDef::Local { .. }
                            | SymbolDef::PatternBinding { .. } => ExpressionRoute::Local(symbol),
                            _ => return None,
                        }
                    }
                    _ => return None,
                }
            };
            Some((id, route))
        })
        .collect()
}

impl PreparedProgram {
    pub(crate) fn parameter_default(
        &self,
        reference: &etas_code::descriptor::ParameterDefaultInfo,
    ) -> Option<&etas_frontend::code::parameter_defaults::ParameterDefaultUnit> {
        self.arguments.default_unit(reference)
    }

    pub(crate) fn select_parameter_default(
        self: &Arc<Self>,
        reference: &etas_code::descriptor::ParameterDefaultInfo,
    ) -> Result<InterpreterPlan, Vec<Diagnostic>> {
        let unit = self.parameter_default(reference).ok_or_else(|| {
            vec![diagnostics::missing_checked_fact(
                diagnostics::primary_span(&self.program),
                "parameter default has no matching authored expression revision",
            )]
        })?;
        self.select_callable(CheckedCallableRoot::ParameterDefault(unit.parameter))
    }
    pub(crate) fn standard_library(
        &self,
    ) -> Option<&Arc<super::standard_library::StandardLibraryProgram>> {
        self.standard_library.as_ref()
    }
    pub(crate) fn checked(&self) -> &CheckedProject {
        &self.program
    }

    pub(crate) fn select_snapshot_callable(
        self: &Arc<Self>,
        ty: etas_types::TypeId,
        target: &crate::orchestration::CallTargetSnapshot,
    ) -> Result<Arc<InterpreterPlan>, String> {
        let root = crate::value::ScopedCallableValue::snapshot_root(&self.dispatch, target)?;
        let plan = Arc::new(self.select_callable(root).map_err(|errors| {
            format!("checkpoint execution root has no current checked plan: {errors:?}")
        })?);
        crate::value::ScopedCallableValue::checked_snapshot_root(
            &crate::value::CheckedExecutionProgram {
                checked: self.program.clone(),
                plan: plan.clone(),
                definitions: None,
                revision: None,
                foreign: None,
            },
            ty,
            target,
        )?;
        Ok(plan)
    }

    pub(crate) fn select_callable(
        self: &Arc<Self>,
        root: CheckedCallableRoot,
    ) -> Result<InterpreterPlan, Vec<Diagnostic>> {
        let project = &self.program;
        let mut context = PlanContext::new(project);
        context.diagnostics.clone_from(&self.diagnostics);
        context.requested_root = Some(root);
        context.prepared_residual = Some(self.residual.clone());
        let mut pipeline = Pipeline::new("interpreter.plan.select_entry")
            .pass(UsePreparedResidualPlansPass)
            .pass(BuildEntryPlanPass)
            .pass(VerifyResidualCoveragePass)
            .pass(ComputeReachableHostRequirementsPass)
            .pass(ComputeEntryActionMediationPass);
        run_pipeline(&mut pipeline, &mut context);
        let dispatch = self
            .dispatch
            .select_callable(project, root)
            .map_err(|reason| {
                vec![diagnostics::missing_checked_fact(
                    diagnostics::primary_span(project),
                    reason,
                )]
            })?;
        if context
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == Severity::Error)
        {
            return Err(context.diagnostics);
        }
        let (
            Some(entry),
            Some(host_requirements),
            Some(action_mediation),
            Some(residual_installation),
        ) = (
            context.entry,
            context.host_requirements,
            context.action_mediation,
            context.residual_installation,
        )
        else {
            return Err(vec![diagnostics::missing_checked_fact(
                diagnostics::primary_span(project),
                "selected callable plan is missing checked entry facts or residual installation evidence",
            )]);
        };
        Ok(InterpreterPlan {
            standard_library: self.standard_library.clone(),
            prepared: self.clone(),
            program: self.program.clone(),
            root,
            entry,
            dispatch,
            host_requirements,
            action_mediation,
            residual: self.residual.clone(),
            residual_installation,
            diagnostics: context.diagnostics,
            known_std_types: self.known_std_types,
            spec_facts: self.spec_facts.clone(),
            slots: self.slots.clone(),
            frames: self.frames.clone(),
            arguments: self.arguments.clone(),
            closures: self.closures.clone(),
            named_variants: self.named_variants.clone(),
            records: self.records.clone(),
            globals: self.globals.clone(),
            resources: self.resources.clone(),
        })
    }
}

impl InterpreterPlan {
    pub(crate) fn checked_spec_facts(&self) -> &etas_types::SpecFacts {
        self.spec_facts
            .get_or_init(|| etas_types::SpecFacts::from(self.program.types.as_ref()))
    }

    pub(crate) fn prepared_program(&self) -> &Arc<PreparedProgram> {
        &self.prepared
    }

    pub(crate) fn select_entry(
        &self,
        project: &CheckedProject,
        entry: etas_hir::HirItemId,
    ) -> Result<Self, Vec<Diagnostic>> {
        self.select_callable(project, CheckedCallableRoot::Item(entry))
    }

    /// Selects an exact checked root while sharing program layouts and ABI data.
    pub fn select_callable(
        &self,
        project: &CheckedProject,
        root: CheckedCallableRoot,
    ) -> Result<Self, Vec<Diagnostic>> {
        if !Arc::ptr_eq(project, &self.program) {
            return Err(vec![diagnostics::missing_checked_fact(
                diagnostics::primary_span(project),
                "callable selection cannot reuse another checked program's plan",
            )]);
        }
        self.prepared.select_callable(root)
    }
}

fn run_pipeline<'a>(pipeline: &mut Pipeline<PlanContext<'a>>, context: &mut PlanContext<'a>) {
    let mut manager = PassManager::new();
    let profile = context
        .project
        .standard_library
        .as_ref()
        .map(|library| library.profile.clone())
        .unwrap_or_default();
    manager.config_mut().collect_timing = profile.is_enabled();
    let run = manager.run_pipeline(pipeline, context);
    for record in &run.records {
        if let Some(timing) = &record.timing {
            profile.counter(
                format!("interpreter.pass.{}.duration_ns", record.pass),
                timing.duration.as_nanos() as u64,
            );
        }
    }
    if let PassControl::Failed(failure) = run.control {
        context.push_missing_fact(
            diagnostics::primary_span(context.project),
            &format!("interpreter plan pipeline failed: {}", failure.message,),
        );
    }
}

fn layout_pipeline<'a>() -> Pipeline<PlanContext<'a>> {
    Pipeline::new("interpreter.plan.prepare_program")
        .pass(ValidateCheckedProjectPass)
        .pass(BuildSlotLayoutPass)
        .pass(BuildFrameLayoutsPass)
        .pass(BuildCallArgumentsPass)
        .pass(BuildClosureLayoutsPass)
        .pass(BuildNamedVariantLayoutsPass)
        .pass(BuildRecordLayoutsPass)
        .pass(BuildGlobalTablePass)
        .pass(BuildResourceHandleTablePass)
        .pass(BuildIntrinsicDispatchTablePass)
        .pass(ValidateResidualPlansPass)
        .pass(BindResidualSitesPass)
}
