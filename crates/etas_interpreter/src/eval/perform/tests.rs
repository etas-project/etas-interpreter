use super::*;
use crate::{api::RunOptions, testing::allocation::measure};

#[test]
fn perform_suspension_does_not_copy_named_argument_descriptors() {
    for count in [1000, 2000, 4000] {
        let params = (0..count)
            .map(|i| format!("p{i}: i32"))
            .collect::<Vec<_>>()
            .join(",");
        let arguments = (0..count)
            .map(|i| format!("p{i} = {i}"))
            .collect::<Vec<_>>()
            .join(",");
        let checked = crate::testing::project::checked_project(&format!(
            "module app.main; effect Probe {{ action send({params}) -> unit; }} flow main() -> unit ![Probe.send] {{ perform Probe.send({arguments}); }}"
        ));
        let plan = crate::Interpreter
            .plan(&checked, crate::api::PlanOptions)
            .plan
            .unwrap();
        let expr = checked
            .hir
            .exprs
            .iter()
            .find_map(|(id, value)| matches!(value, HirExpr::Perform { .. }).then_some(id))
            .unwrap();
        let options = RunOptions::default();
        let mut eval = EvalContext::new(EvalContextInput {
            storage_limits: Default::default(),
            event_observer: None,
            execution: etas_host::execution::ExecutionScope::new_owned(),
            checked: &checked,
            plan: &plan,
            host_context: options.host_context,
            model_policy: options.model_policy,
            execution_limits: options.execution_limits,
            consumed_steps: 0,
            current_session: None,
            entry_item: checked.entry.unwrap(),
            entry_args: &[],
        });
        let mut frame = Frame::new(plan.slots.clone());
        let (signal, cost) = measure(|| eval.eval_expr_frame(expr, &mut frame));
        eprintln!("perform entry n={count}: {cost:?}");
        assert!(
            cost.count <= 32,
            "descriptor copies on perform entry: {cost:?}"
        );
        let ControlSignal::Expr(pending) = signal else {
            panic!("pending argument")
        };
        let continuation = perform_args(&pending.continuation).unwrap();
        let Continuation::PerformArgs {
            args,
            evaluated_args,
            ..
        } = continuation
        else {
            unreachable!()
        };
        let planned = plan.arguments.get(expr).unwrap();
        assert!(std::sync::Arc::ptr_eq(args, &planned));
        assert!(evaluated_args.capacity() >= count);
        let (snapshot, capture_cost) =
            measure(|| crate::orchestration::ContinuationSnapshot::capture(continuation).unwrap());
        let (alias, clone_cost) = measure(|| snapshot.clone());
        let (restored, restore_cost) = measure(|| {
            snapshot
                .restore_with(&mut crate::eval::machine::snapshot::RestoreContext::default())
                .unwrap()
        });
        let Continuation::PerformArgs { args, .. } = restored else {
            panic!("restored perform")
        };
        assert!(std::sync::Arc::ptr_eq(&args, &planned));
        for cost in [capture_cost, clone_cost, restore_cost] {
            assert!(
                cost.count < 32 && cost.bytes < 4096,
                "descriptor snapshot copies: {cost:?}"
            );
        }
        eprintln!(
            "perform snapshot n={count}: capture={capture_cost:?}, clone={clone_cost:?}, restore={restore_cost:?}"
        );
        let limits = etas_host::StorageLimits::default();
        let validator = crate::eval::machine::snapshot::SnapshotValidator::new(
            &checked,
            &plan.slots,
            &plan.dispatch,
            &plan.closures,
            &limits,
        );
        let machine = |continuation| crate::orchestration::MachineSnapshot {
            frames: vec![crate::orchestration::MachineFrameSnapshot::Continuation { continuation }],
        };
        validator.validate_machine(&machine(alias.clone())).unwrap();
        let mut bad = alias.clone();
        let crate::orchestration::ContinuationSnapshot::PerformArgs { args, .. } = &mut bad else {
            unreachable!()
        };
        let HirArg::Named { name, .. } = &mut std::sync::Arc::make_mut(args)[0] else {
            panic!("named")
        };
        *name = "not_the_checked_parameter".into();
        let error = validator.validate_machine(&machine(bad)).unwrap_err();
        assert!(error.contains("descriptors disagree"), "{error}");
        validator.validate_machine(&machine(alias)).unwrap();

        let HirExpr::Perform { action, span, .. } = &checked.hir.exprs[expr] else {
            unreachable!()
        };
        let (matches, fact_cost) =
            measure(|| eval.performed_action_fact_matches(expr, action, &planned));
        assert!(matches);
        assert_eq!(
            fact_cost.count, 0,
            "action fact query copied descriptors: {fact_cost:?}"
        );
        let mut reordered = planned.clone();
        std::sync::Arc::make_mut(&mut reordered).swap(0, 1);
        assert!(!eval.performed_action_fact_matches(expr, action, &reordered));
        let mut values = Vec::with_capacity(count);
        values.resize(count / 2, InterpValue::i32(0));
        let pointer = values.as_ptr();
        let (signal, cost) = measure(|| {
            eval.resume_perform_args(
                PerformArgsResume {
                    expr,
                    action: action.clone(),
                    type_args: vec![],
                    args: planned.clone(),
                    start_arg_index: count / 2,
                    evaluated_args: values,
                    span: *span,
                },
                &mut frame,
            )
        });
        let ControlSignal::Expr(pending) = signal else {
            panic!("pending resume")
        };
        let Continuation::PerformArgs {
            evaluated_args,
            args,
            ..
        } = perform_args(&pending.continuation).unwrap()
        else {
            unreachable!()
        };
        assert_eq!(evaluated_args.as_ptr(), pointer);
        assert!(std::sync::Arc::ptr_eq(args, &planned));
        assert!(
            cost.count < 16 && cost.bytes < 1024,
            "partial argument copies: {cost:?}"
        );
    }
}

fn perform_args(value: &Continuation) -> Option<&Continuation> {
    match value {
        Continuation::PerformArgs { .. } => Some(value),
        Continuation::Chain { inner, outer } => perform_args(inner).or_else(|| perform_args(outer)),
        _ => None,
    }
}
