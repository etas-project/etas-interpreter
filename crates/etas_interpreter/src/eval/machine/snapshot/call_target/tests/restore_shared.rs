use super::*;

fn shared_target(depth: usize, composed: bool) -> CallTargetSnapshot {
    let mut target = CallTargetSnapshot::FlowItem(HirItemId(1));
    for _ in 0..depth {
        target = if composed {
            CallTargetSnapshot::Composed(vec![target].into())
        } else {
            CallTargetSnapshot::Limited {
                target: target.into(),
                limits: vec![],
            }
        };
    }
    target
}

fn same_backing(a: &CallTarget, b: &CallTarget) -> bool {
    match (a, b) {
        (CallTarget::Limited { target: a, .. }, CallTarget::Limited { target: b, .. }) => {
            std::ptr::eq(&**a, &**b)
        }
        (CallTarget::Composed(a), CallTarget::Composed(b)) => a.as_ptr() == b.as_ptr(),
        _ => panic!("unexpected callable shape"),
    }
}

#[test]
fn shared_callable_restore_reuses_graph_across_values_in_one_context() {
    use crate::{orchestration::ValueSnapshot, value::InterpValue};
    for composed in [false, true] {
        for count in [1000, 2000, 4000] {
            let saved = ValueSnapshot::Callable(shared_target(64, composed));
            let ((context, values), cost) = measure(|| {
                let mut context = RestoreContext::default();
                let values = (0..count)
                    .map(|_| saved.clone().restore_with(&mut context).unwrap())
                    .collect::<Vec<_>>();
                (context, values)
            });
            eprintln!("shared callable restore count={count} composed={composed}: {cost:?}");
            assert!(cost.count < 512, "repeated graph reconstruction: {cost:?}");
            let InterpValue::Callable(first) = &values[0] else {
                panic!("callable")
            };
            for value in &values[1..] {
                let InterpValue::Callable(target) = value else {
                    panic!("callable")
                };
                assert!(same_backing(first, target));
            }
            let InterpValue::Callable(independent) = saved
                .clone()
                .restore_with(&mut RestoreContext::default())
                .unwrap()
            else {
                panic!("independent callable")
            };
            assert!(!same_backing(first, &independent));
            assert!(first == &independent);
            drop((context, values, independent));
            let (_, released) = measure(|| {
                let mut context = RestoreContext::default();
                for _ in 0..count {
                    drop(saved.clone().restore_with(&mut context).unwrap());
                }
            });
            assert_eq!(released.bytes, released.released_bytes, "{released:?}");
        }
    }
}

#[test]
fn callable_restore_cache_crosses_captured_frames_without_merging_distinct_graphs() {
    use crate::{
        orchestration::{LocalsSnapshot, ValueSnapshot},
        value::InterpValue,
    };
    use etas_hir::{HirExprId, SymbolId};
    for composed in [false, true] {
        let saved = shared_target(64, composed);
        let retained = saved.clone();
        let lambda = CallTargetSnapshot::Lambda {
            expr: HirExprId(4),
            captured: LocalsSnapshot {
                id: 1,
                locals: std::rc::Rc::new(vec![(
                    SymbolId(2),
                    ValueSnapshot::Callable(saved.clone()),
                )]),
                type_bindings: vec![],
            },
        };
        let mut context = RestoreContext::default();
        let CallTarget::Lambda { captured, .. } =
            restore_call_target(lambda, &mut context).unwrap()
        else {
            panic!("lambda")
        };
        let InterpValue::Callable(alias) = captured.get(SymbolId(2)).unwrap() else {
            panic!("captured callable")
        };
        let mut direct = restore_call_target(saved, &mut context).unwrap();
        assert!(same_backing(&direct, &alias));
        let distinct = restore_call_target(shared_target(64, composed), &mut context).unwrap();
        assert!(!same_backing(&direct, &distinct));
        assert!(direct == distinct);
        match &mut direct {
            CallTarget::Limited { target, .. } => **target = CallTarget::FlowItem(HirItemId(9)),
            CallTarget::Composed(targets) => targets[0] = CallTarget::FlowItem(HirItemId(9)),
            _ => unreachable!(),
        }
        assert!(!same_backing(&direct, &alias));
        assert!(capture_call_target(&alias).unwrap() == retained);
        assert!(capture_call_target(&direct).unwrap() != retained);
        let restored_again = restore_call_target(retained.clone(), &mut context).unwrap();
        assert!(same_backing(&restored_again, &alias));
        assert!(capture_call_target(&restored_again).unwrap() == retained);
    }
}

#[test]
fn callable_restore_cache_does_not_hide_conflicting_frames_and_releases_failed_graphs() {
    use crate::{
        orchestration::{LocalsSnapshot, ValueSnapshot},
        value::InterpValue,
    };
    use etas_hir::{HirExprId, SymbolId};
    for composed in [false, true] {
        let saved = shared_target(64, composed);
        let lambda = |value| CallTargetSnapshot::Lambda {
            expr: HirExprId(4),
            captured: LocalsSnapshot {
                id: 1,
                locals: std::rc::Rc::new(vec![(
                    SymbolId(2),
                    ValueSnapshot::capture(&InterpValue::i32(value)).unwrap(),
                )]),
                type_bindings: vec![],
            },
        };
        let bad = CallTargetSnapshot::Composed(
            vec![saved.clone(), lambda(1), saved.clone(), lambda(2)].into(),
        );
        for _ in 0..2 {
            let (_, cost) = measure(|| {
                let mut context = RestoreContext::default();
                drop(restore_call_target(saved.clone(), &mut context).unwrap());
                let error = restore_call_target(bad.clone(), &mut context).unwrap_err();
                assert!(
                    error.contains("conflicting definitions for snapshot frame identity"),
                    "{error}"
                );
            });
            assert_eq!(cost.bytes, cost.released_bytes, "{cost:?}");
        }
    }
}

#[test]
fn checked_machine_restore_shares_callables_between_continuations() {
    use crate::{
        control::Continuation,
        eval::machine::state::EvalMachine,
        orchestration::{
            ContinuationSnapshot, LocalsSnapshot, MachineFrameSnapshot, MachineSnapshot,
        },
    };
    let checked = crate::testing::project::checked_project(
        "module app.main; flow main() -> unit { return; }",
    );
    let plan = crate::Interpreter
        .plan(&checked, crate::api::PlanOptions)
        .plan
        .unwrap();
    let span = checked.hir.blocks.iter().next().unwrap().1.span;
    for count in [1000, 2000, 4000] {
        let mut target = CallTargetSnapshot::FlowItem(checked.entry.unwrap());
        for _ in 0..64 {
            target = CallTargetSnapshot::Composed(vec![target].into());
        }
        let continuation = ContinuationSnapshot::CallArgs {
            target,
            args: vec![].into(),
            next_arg_index: 0,
            evaluated_args: vec![],
            span,
            frame: LocalsSnapshot {
                id: 1,
                locals: Default::default(),
                type_bindings: vec![],
            },
        };
        let mut saved = MachineSnapshot {
            frames: (0..count)
                .map(|_| MachineFrameSnapshot::Continuation {
                    continuation: continuation.clone(),
                })
                .collect(),
        };
        let restore = |saved: &MachineSnapshot| {
            EvalMachine::from_snapshot(
                saved,
                &checked,
                plan.slots.clone(),
                &plan.dispatch,
                &plan.closures,
                &crate::api::HostExecutionContext::default(),
                &etas_host::StorageLimits::default(),
            )
        };
        let (mut machine, cost) = measure(|| restore(&saved).unwrap());
        eprintln!("checked machine shared callable restore count={count}: {cost:?}");
        assert!(
            cost.count <= count * 3 + 512,
            "reconstructed shared graph: {cost:?}"
        );
        let Continuation::CallArgs { target: first, .. } =
            machine.pop_frame().unwrap().into_continuation()
        else {
            panic!("call args")
        };
        while let Some(frame) = machine.pop_frame() {
            let Continuation::CallArgs { target, .. } = frame.into_continuation() else {
                panic!("call args")
            };
            assert!(same_backing(&first, &target));
        }
        let MachineFrameSnapshot::Continuation {
            continuation: ContinuationSnapshot::CallArgs { target, .. },
        } = saved.frames.last_mut().unwrap()
        else {
            panic!("call args")
        };
        *target = CallTargetSnapshot::FlowItem(HirItemId(u32::MAX));
        match restore(&saved) {
            Ok(_) => panic!("invalid callable restored after valid shared targets"),
            Err(error) => assert!(error.contains("missing HIR item"), "{error}"),
        }
    }
}
