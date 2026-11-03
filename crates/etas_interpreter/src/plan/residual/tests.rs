use super::*;
use crate::plan::{
    context::{PlanContext, plan_pass},
    entry::BuildEntryPlanPass,
};
use artifacts::*;
use etas_utils::{Pass, PassContext, PassControl, PassManager, PassResult, Pipeline};

fn checked() -> etas_frontend::CheckedProject {
    let output = crate::testing::project::residual_project_output(
        r#"
module app.main;
effect Gate { action check() -> unit; action send() -> unit; }
spec Safe: trace = Gate.check >> Gate.send;
flow main() -> unit ![] ~ Safe {
    handle { perform Gate.check(); perform Gate.send(); } with {
        Gate.check() => { resume (); }
        Gate.send() => { resume (); }
    };
}
"#,
    );
    assert!(output.diagnostics.is_empty(), "{:#?}", output.diagnostics);
    output.checked.unwrap()
}

#[test]
fn binding_requires_validation_through_the_pass_manager() {
    let checked = checked();
    let mut context = PlanContext::new(&checked);
    let mut manager = PassManager::new();
    let run = manager.run_pipeline(
        &mut Pipeline::new("test").pass(BindResidualSitesPass),
        &mut context,
    );
    let PassControl::Failed(failure) = run.control else {
        panic!("binding must require validation")
    };
    assert_eq!(failure.missing_artifact, Some(VALIDATED_RESIDUAL_PLANS));
    assert!(!manager.available_artifacts().contains(BOUND_RESIDUAL_SITES));
    assert!(context.residual.is_none());
}

struct LoseValidatedObject;
impl Pass<PlanContext<'_>> for LoseValidatedObject {
    fn descriptor(&self) -> etas_utils::PassDescriptor {
        plan_pass("test.lose_validated")
    }
    fn run(
        &mut self,
        context: &mut PlanContext<'_>,
        _: &PassContext<PlanContext<'_>>,
        _: &mut PassManager<PlanContext<'_>>,
    ) -> PassResult {
        context.validated_residual = None;
        PassResult::unchanged()
    }
}

#[test]
fn artifact_marker_without_the_validated_object_cannot_bind() {
    let checked = checked();
    let mut context = PlanContext::new(&checked);
    let mut manager = PassManager::new();
    let run = manager.run_pipeline(
        &mut Pipeline::new("test")
            .pass(ValidateResidualPlansPass)
            .pass(LoseValidatedObject)
            .pass(BindResidualSitesPass),
        &mut context,
    );
    let PassControl::Failed(failure) = run.control else {
        panic!("missing object must fail")
    };
    assert!(failure.message.contains("lost its validated plan artifact"));
    assert!(!manager.available_artifacts().contains(BOUND_RESIDUAL_SITES));
    assert!(context.residual.is_none());
}

#[test]
fn failed_validation_never_publishes_a_ready_artifact() {
    let checked = checked();
    let mut context = PlanContext::new(&checked);
    context.residual_binding = None;
    let mut manager = PassManager::new();
    let run = manager.run_pipeline(
        &mut Pipeline::new("test")
            .pass(ValidateResidualPlansPass)
            .pass(BindResidualSitesPass),
        &mut context,
    );
    let PassControl::Failed(failure) = run.control else {
        panic!("missing binding must fail")
    };
    assert!(failure.message.contains("residual plan validation failed"));
    assert!(
        !manager
            .available_artifacts()
            .contains(VALIDATED_RESIDUAL_PLANS)
    );
    assert!(context.validated_residual.is_none());
    assert!(context.residual.is_none());
}

#[test]
fn coverage_requires_prepared_factories_and_selected_callable() {
    let checked = checked();
    let prepared = crate::plan::prepare_program(&checked).unwrap();
    for missing in [PREPARED_RESIDUAL_PLANS, SELECTED_CALLABLE] {
        let mut context = PlanContext::new(&checked);
        context.prepared_residual = Some(prepared.residual.clone());
        let mut pipeline = Pipeline::new("test");
        if missing == PREPARED_RESIDUAL_PLANS {
            pipeline = pipeline.pass(BuildEntryPlanPass);
        } else {
            pipeline = pipeline.pass(UsePreparedResidualPlansPass);
        }
        pipeline = pipeline.pass(VerifyResidualCoveragePass);
        let mut manager = PassManager::new();
        let run = manager.run_pipeline(&mut pipeline, &mut context);
        let PassControl::Failed(failure) = run.control else {
            panic!("coverage input must be required")
        };
        assert_eq!(failure.missing_artifact, Some(missing));
        assert!(
            !manager
                .available_artifacts()
                .contains(VERIFIED_RESIDUAL_COVERAGE)
        );
        assert!(context.residual_installation.is_none());
    }
}

#[test]
fn installation_evidence_is_bound_to_the_native_program_root_and_preparation() {
    let checked = checked();
    let plan = crate::plan::build_plan(&checked, Default::default())
        .plan
        .unwrap();
    let evidence = &plan.residual_installation;
    evidence
        .validate(&checked, plan.root, &plan.residual)
        .unwrap();
    let other = self::checked();
    assert!(
        evidence
            .validate(&other, plan.root, &plan.residual)
            .is_err()
    );
    let prepared_again = crate::plan::prepare_program(&checked).unwrap();
    assert!(
        evidence
            .validate(&checked, plan.root, &prepared_again.residual)
            .is_err()
    );
    assert!(
        evidence
            .validate(
                &checked,
                etas_frontend::CheckedCallableRoot::Lambda(etas_hir::HirExprId(0)),
                &plan.residual
            )
            .is_err()
    );
}
