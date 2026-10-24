use super::*;
use crate::{api::RunOptions, orchestration::ContinuationSnapshot, testing::allocation::measure};

#[test]
fn argument_suspension_moves_descriptors_and_partial_results_without_copying() {
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
            "module app.main; flow consume({params}) -> unit {{ return; }} flow main() -> unit {{ consume({arguments}); }}"
        ));
        let plan = crate::Interpreter
            .plan(&checked, crate::api::PlanOptions)
            .plan
            .unwrap();
        let (call, callee, args, span) = checked
            .hir
            .exprs
            .iter()
            .find_map(|(id, expr)| match expr {
                HirExpr::Call {
                    callee, args, span, ..
                } => Some((id, *callee, args, *span)),
                _ => None,
            })
            .unwrap();
        let HirExpr::Path(path) = &checked.hir.exprs[callee] else {
            panic!("callee")
        };
        let ResolveResult::Resolved(symbol) = path.resolution else {
            panic!("resolution")
        };
        let SymbolDef::Item { item } = checked.symbols.get(symbol).unwrap().def else {
            panic!("flow")
        };
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
        for _ in 0..2 {
            let (signal, cost) =
                measure(|| eval.eval_call(call, callee, &[], args, span, &mut frame));
            let ControlSignal::Expr(pending) = signal else {
                panic!("pending call argument")
            };
            let continuation = call_args(&pending.continuation).unwrap();
            let Continuation::CallArgs { args, .. } = continuation else {
                unreachable!()
            };
            let planned = plan.arguments.get(call).unwrap();
            assert!(std::sync::Arc::ptr_eq(args, &planned));
            eprintln!("call entry n={count}: {cost:?}");
            assert!(
                cost.count <= 16,
                "call entry copied argument descriptors: {cost:?}"
            );

            let (snapshot, capture_cost) =
                measure(|| ContinuationSnapshot::capture(continuation).unwrap());
            let (retained, clone_cost) = measure(|| snapshot.clone());
            let ContinuationSnapshot::CallArgs { args, .. } = &retained else {
                panic!("call snapshot")
            };
            assert!(std::sync::Arc::ptr_eq(args, &planned));
            assert_eq!(clone_cost.count, 0, "{clone_cost:?}");
            assert!(
                capture_cost.count <= 4 && capture_cost.bytes < 1024,
                "{capture_cost:?}"
            );
            let (restored, restore_cost) = measure(|| {
                snapshot
                    .restore_with(&mut super::super::machine::snapshot::RestoreContext::default())
                    .unwrap()
            });
            let Continuation::CallArgs { args, .. } = restored else {
                panic!("restored call")
            };
            assert!(std::sync::Arc::ptr_eq(&args, &planned));
            assert!(
                restore_cost.count <= 16 && restore_cost.bytes < 2048,
                "{restore_cost:?}"
            );
            let limits = etas_host::StorageLimits::default();
            let validator = super::super::machine::snapshot::SnapshotValidator::new(
                &checked,
                &plan.slots,
                &plan.dispatch,
                &plan.closures,
                &limits,
            );
            let machine = |continuation| crate::orchestration::MachineSnapshot {
                frames: vec![crate::orchestration::MachineFrameSnapshot::Continuation {
                    continuation,
                }],
            };
            validator
                .validate_machine(&machine(retained.clone()))
                .unwrap();
            let mut bad = retained.clone();
            let ContinuationSnapshot::CallArgs { args: bad_args, .. } = &mut bad else {
                unreachable!()
            };
            std::sync::Arc::make_mut(bad_args)[0] = HirArg::Positional(HirExprId(u32::MAX));
            assert!(!std::sync::Arc::ptr_eq(bad_args, &planned));
            let error = validator.validate_machine(&machine(bad)).unwrap_err();
            assert!(error.contains("missing HIR expression"), "{error}");
            validator.validate_machine(&machine(retained)).unwrap();
            eprintln!(
                "call snapshot n={count}: capture={capture_cost:?}, clone={clone_cost:?}, restore={restore_cost:?}"
            );
        }
        let args = plan.arguments.get(call).unwrap();
        let descriptor_pointer = args.as_ptr();
        let mut evaluated = Vec::with_capacity(count);
        evaluated.resize(count / 2, InterpValue::i32(0));
        let results_pointer = evaluated.as_ptr();
        let (signal, cost) = measure(|| {
            eval.resume_call_args(
                CallTarget::FlowItem(item),
                args,
                count / 2,
                evaluated,
                span,
                &mut frame,
            )
        });
        let ControlSignal::Expr(pending) = signal else {
            panic!("pending argument")
        };
        let continuation = call_args(&pending.continuation).unwrap();
        let Continuation::CallArgs {
            args,
            evaluated_args,
            next_arg_index,
            ..
        } = continuation
        else {
            unreachable!()
        };
        assert_eq!(args.as_ptr(), descriptor_pointer);
        assert_eq!(evaluated_args.as_ptr(), results_pointer);
        assert_eq!(*next_arg_index, count / 2 + 1);
        assert!(
            cost.count <= 2 && cost.bytes < 1024,
            "{count} parameters: {cost:?}"
        );
        eprintln!("argument suspension n={count}: {cost:?}");
    }
}

fn call_args(value: &Continuation) -> Option<&Continuation> {
    match value {
        Continuation::CallArgs { .. } => Some(value),
        Continuation::Chain { inner, outer } => call_args(inner).or_else(|| call_args(outer)),
        _ => None,
    }
}
