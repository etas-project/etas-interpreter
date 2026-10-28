use std::sync::Arc;

use etas_frontend::{CheckedCallableRoot, CheckedProgram};
use etas_hir::{HirExpr, HirExprId};

use super::InterpreterPlan;
use crate::{api::PlanOptions, host::HostServiceAvailability};

fn lambda(program: &CheckedProgram) -> HirExprId {
    program
        .hir
        .exprs
        .iter()
        .find_map(|(expr, data)| matches!(data, HirExpr::Lambda { .. }).then_some(expr))
        .unwrap()
}

fn plan(program: &CheckedProgram) -> InterpreterPlan {
    let result = super::build_plan(program, PlanOptions);
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    result.plan.unwrap()
}

fn run_lambda(
    program: &CheckedProgram,
    plan: &InterpreterPlan,
    expr: HirExprId,
) -> crate::value::InterpValue {
    use crate::{
        control::{CallTarget, Continuation, ControlSignal, Frame, PendingCall},
        eval::{
            EvalContext, EvalContextInput,
            machine::{EvalMachine, MachinePoll},
        },
    };
    let (input, mut runtime) = EvalContextInput {
        code_definitions: None,
        syntax_limits: Default::default(),
        storage_limits: Default::default(),
        event_observer: None,
        execution: etas_host::execution::ExecutionScope::new(),
        checked: program,
        plan,
        host_context: Default::default(),
        model_policy: Default::default(),
        execution_limits: Default::default(),
        consumed_steps: 0,
        current_session: None,
        entry_item: plan.entry.item,
        entry_args: &[],
    }
    .into_parts();
    let mut eval = EvalContext::new(input, &mut runtime);
    let mut machine = EvalMachine::new();
    machine.resume(ControlSignal::pending_call(PendingCall {
        site: None,
        target: CallTarget::Lambda {
            expr,
            captured: Frame::new(plan.closures.get(expr).unwrap().slots.clone()),
        },
        args: vec![],
        span: program.hir.exprs[expr].span(&program.hir.blocks),
        continuation: Continuation::BlockValue,
    }));
    loop {
        match machine.run_until_yield(&mut eval) {
            MachinePoll::Complete(value) => {
                assert!(eval.diagnostics.is_empty(), "{:?}", eval.diagnostics);
                return *value;
            }
            MachinePoll::CooperativeYield => {}
            MachinePoll::Fault(fault) => panic!("{fault:?}"),
            _ => panic!("pure lambda unexpectedly crossed an execution boundary"),
        }
    }
}

#[test]
fn lambda_plan_has_its_own_readiness_mediation_and_intrinsic_closure() {
    let program = crate::testing::project::checked_project(
        r#"
module app.main;
import std.io.println;
import std.text.uppercase;
flow helper() -> string { return uppercase("value"); }
flow factory() -> () -> string ![] {
    println("not part of callback")?;
    return () => { return helper(); };
}
flow main() -> unit { return; }
"#,
    );
    let source = plan(&program);
    let expr = lambda(&program);
    let selected = source
        .select_callable(&program, CheckedCallableRoot::Lambda(expr))
        .unwrap();
    assert_eq!(selected.root, CheckedCallableRoot::Lambda(expr));
    assert!(Arc::ptr_eq(&source.spec_facts, &selected.spec_facts));
    assert!(std::ptr::eq(
        source.checked_spec_facts(),
        selected.checked_spec_facts()
    ));
    let (_, cost) = crate::testing::allocation::measure(|| {
        for _ in 0..100 {
            std::hint::black_box(selected.checked_spec_facts());
        }
    });
    assert_eq!(
        cost.count, 0,
        "checked spec queries copied program facts: {cost:?}"
    );
    let span = program.hir.exprs[expr].span(&program.hir.blocks);
    assert!(
        crate::host::validate_host_readiness(&selected, HostServiceAvailability::default(), span)
            .is_empty()
    );
    assert!(selected.action_mediation.requested_actions.is_empty());
    assert!(selected.action_mediation.default_actions.is_empty());
    let intrinsic = |path: &[&str]| {
        let path: Vec<_> = path.iter().map(|part| (*part).to_owned()).collect();
        let descriptor = program
            .std_registry
            .lookup_qualified(&path)
            .unwrap()
            .intrinsic
            .as_ref()
            .unwrap();
        crate::intrinsic::dispatch::StdIntrinsicIdentity {
            intrinsic: descriptor.id,
            dispatch: descriptor.dispatch,
        }
    };
    assert!(
        selected
            .dispatch
            .validate_std_intrinsic(intrinsic(&["std", "text", "uppercase"]))
            .is_ok()
    );
    assert!(
        selected
            .dispatch
            .validate_std_intrinsic(intrinsic(&["std", "io", "println"]))
            .is_err()
    );
    assert!(Arc::ptr_eq(&source.slots, &selected.slots));
    assert!(Arc::ptr_eq(&source.closures, &selected.closures));
    assert!(Arc::ptr_eq(&source.frames, &selected.frames));
    assert!(std::ptr::eq(
        source.dispatch.pure_abi(),
        selected.dispatch.pure_abi()
    ));
    assert_eq!(
        run_lambda(&program, &selected, expr),
        crate::value::InterpValue::String("VALUE".into())
    );
}

#[test]
fn lambda_plan_retains_internal_handled_host_actions() {
    let program = crate::testing::project::checked_project(
        r#"
module app.main;
import std.io.println;
flow factory() -> () -> unit ![] {
    return () => { println("inside callback")?; return; };
}
flow main() -> unit { return; }
"#,
    );
    let source = plan(&program);
    let expr = lambda(&program);
    let selected = source
        .select_callable(&program, CheckedCallableRoot::Lambda(expr))
        .unwrap();
    let span = program.hir.exprs[expr].span(&program.hir.blocks);
    assert!(
        !crate::host::validate_host_readiness(&selected, HostServiceAvailability::default(), span)
            .is_empty()
    );
    assert!(
        crate::host::validate_host_readiness(
            &selected,
            HostServiceAvailability {
                console: true,
                ..Default::default()
            },
            span
        )
        .is_empty()
    );
    assert!(!selected.action_mediation.requested_actions.is_empty());
    assert!(!selected.action_mediation.default_actions.is_empty());
}

#[test]
fn callable_selection_rejects_missing_lambda_facts_and_other_programs() {
    let mut program = crate::testing::project::checked_project(
        r#"
module app.main;
flow main() -> i32 { let action = () => 7; return action(); }
"#,
    );
    let source = plan(&program);
    let expr = lambda(&program);
    let mut other = program.clone();
    Arc::make_mut(&mut other);
    let error = source
        .select_callable(&other, CheckedCallableRoot::Lambda(expr))
        .unwrap_err();
    assert!(
        error
            .iter()
            .any(|diagnostic| diagnostic.message.contains("another checked program"))
    );
    Arc::make_mut(&mut Arc::make_mut(&mut program).effects)
        .latent_effects
        .remove(&expr);
    let source = plan(&program);
    assert!(
        source
            .select_callable(&program, CheckedCallableRoot::Lambda(expr))
            .is_err()
    );
}

#[test]
fn item_plan_rejects_an_inconsistent_support_projection() {
    let mut program = crate::testing::project::checked_project(
        "module app.main; flow main() -> unit { return; }",
    );
    let item = program.entry.unwrap();
    Arc::make_mut(&mut Arc::make_mut(&mut program).effects)
        .interpreter_support
        .items
        .insert(
            item,
            etas_effects::InterpreterSupport::RequiresInterpreterOrchestration(
                etas_effects::InterpreterOrchestrationRequirementSet::feature(
                    etas_effects::InterpreterFeatureKind::Resume,
                ),
            ),
        );
    let result = super::build_plan(&program, PlanOptions);
    assert!(result.plan.is_none());
    assert!(result.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("inconsistent interpreter-support facts")
    }));
}

#[test]
fn standard_callable_plan_does_not_inherit_its_factory_host_requirements() {
    let program = crate::testing::project::checked_project(
        r#"
module app.main;
import std.io.println;
import std.text.uppercase;
flow factory() -> (string) -> string ![] {
    println("not part of the returned callable")?;
    return uppercase;
}
flow main() -> unit { return; }
"#,
    );
    let source = plan(&program);
    let fact = program.effects.std_callable_values.values().next().unwrap();
    let root = CheckedCallableRoot::StdReference(fact.value);
    let selected = source.select_callable(&program, root).unwrap();
    let span = program.hir.exprs[fact.value].span(&program.hir.blocks);
    assert!(
        crate::host::validate_host_readiness(&selected, HostServiceAvailability::default(), span)
            .is_empty()
    );
    assert!(selected.action_mediation.requested_actions.is_empty());
    assert!(selected.action_mediation.default_actions.is_empty());
    assert!(Arc::ptr_eq(&source.slots, &selected.slots));
    assert!(std::ptr::eq(
        source.dispatch.pure_abi(),
        selected.dispatch.pure_abi()
    ));
    assert_eq!(
        program.effects.std_callable_values.len(),
        1,
        "direct-call callee expressions must not duplicate callable-value facts"
    );
}

#[test]
fn standard_callable_selection_rejects_missing_or_mismatched_checked_facts() {
    let original = crate::testing::project::checked_project(
        "module app.main; import std.text.uppercase; flow main() -> unit { let f = uppercase; return; }",
    );
    let expr = *original.effects.std_callable_values.keys().next().unwrap();
    for change in 0..3 {
        let mut program = original.clone();
        let effects = Arc::make_mut(&mut Arc::make_mut(&mut program).effects);
        match change {
            0 => {
                effects.std_callable_values.remove(&expr);
            }
            1 => {
                effects
                    .std_callable_values
                    .get_mut(&expr)
                    .unwrap()
                    .intrinsic = u32::MAX;
            }
            2 => {
                effects
                    .std_callable_values
                    .get_mut(&expr)
                    .unwrap()
                    .flow_type = etas_types::TypeId(u32::MAX);
            }
            _ => unreachable!(),
        }
        assert!(
            program
                .checked_callable_effects(CheckedCallableRoot::StdReference(expr))
                .is_err()
        );
    }
}

#[test]
fn standard_callable_abi_selection_requires_checked_identity_and_exact_types() {
    let program = crate::testing::project::checked_project(
        "module app.main; import std.text.uppercase; flow main() -> unit { let action = uppercase; return; }",
    );
    let source = plan(&program);
    let fact = program.effects.std_callable_values.values().next().unwrap();
    let etas_types::Type::Function(signature) = program.type_store.get(fact.flow_type).unwrap()
    else {
        panic!("checked callable type");
    };
    let identity = source.dispatch.std_intrinsic(fact.symbol).unwrap();
    assert_eq!(
        source
            .dispatch
            .intrinsic_callable_root(identity, &signature.input, signature.output)
            .unwrap(),
        CheckedCallableRoot::StdReference(fact.value)
    );
    assert!(
        source
            .dispatch
            .intrinsic_callable_root(identity, &[], signature.output)
            .is_err()
    );
    assert!(
        source
            .dispatch
            .intrinsic_callable_root(
                crate::intrinsic::dispatch::StdIntrinsicIdentity {
                    dispatch: etas_std::IntrinsicDispatch::Host,
                    ..identity
                },
                &signature.input,
                signature.output
            )
            .is_err()
    );
}

#[test]
fn program_preparation_cannot_execute_intrinsics_before_selecting_a_checked_root() {
    let program = crate::testing::project::checked_project(
        "module app.main; import std.text.uppercase; flow main() -> unit { let action = uppercase; return; }",
    );
    let prepared = super::prepare_program(&program).unwrap();
    let fact = program.effects.std_callable_values.values().next().unwrap();
    let identity = prepared.dispatch.std_intrinsic(fact.symbol).unwrap();
    assert!(prepared.dispatch.validate_std_intrinsic(identity).is_err());
    let etas_types::Type::Function(signature) = program.type_store.get(fact.flow_type).unwrap()
    else {
        panic!("checked callable")
    };
    let snapshot = crate::orchestration::CallTargetSnapshot::PureIntrinsic {
        intrinsic: identity.intrinsic,
        parameter_types: signature.input.clone(),
        result_type: signature.output,
    };
    let plan = prepared
        .select_snapshot_callable(fact.flow_type, &snapshot)
        .unwrap();
    assert_eq!(plan.root, CheckedCallableRoot::StdReference(fact.value));
    assert!(plan.dispatch.validate_std_intrinsic(identity).is_ok());
    assert!(Arc::ptr_eq(&plan.slots, &prepared.slots));
    let mut invalid = snapshot.clone();
    let crate::orchestration::CallTargetSnapshot::PureIntrinsic { intrinsic, .. } = &mut invalid
    else {
        unreachable!()
    };
    *intrinsic = etas_std::StdIntrinsicId(u32::MAX);
    assert!(
        prepared
            .select_snapshot_callable(fact.flow_type, &invalid)
            .is_err()
    );
    let invalid = crate::orchestration::CallTargetSnapshot::StdIntrinsic {
        origin: fact.value,
        substitutions: Vec::new(),
        intrinsic: identity.intrinsic,
        dispatch: etas_std::IntrinsicDispatch::Host,
        parameter_types: signature.input.clone(),
        result_type: signature.output,
    };
    assert!(
        prepared
            .select_snapshot_callable(fact.flow_type, &invalid)
            .is_err()
    );
    assert!(prepared.dispatch.validate_std_intrinsic(identity).is_err());
}
