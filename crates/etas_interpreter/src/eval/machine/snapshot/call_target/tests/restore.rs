use super::*;

fn snapshot_tree(mut node: CallTargetSnapshot, depth: usize, mode: usize) -> CallTargetSnapshot {
    for _ in 0..depth {
        node = match mode {
            0 => CallTargetSnapshot::Limited {
                target: node.into(),
                limits: vec![],
            },
            1 => CallTargetSnapshot::Specialized {
                target: node.into(),
                type_bindings: vec![],
            },
            2 => CallTargetSnapshot::Composed(
                vec![node, CallTargetSnapshot::FlowItem(HirItemId(2))].into(),
            ),
            _ => CallTargetSnapshot::Composed(
                vec![CallTargetSnapshot::FlowItem(HirItemId(2)), node].into(),
            ),
        };
    }
    node
}

#[test]
fn deep_call_target_restore_consumes_snapshot_edges_without_recursive_descent() {
    const WORKER: &str = "ETAS_TEST_CALL_TARGET_RESTORE_WORKER";
    if std::env::var_os(WORKER).is_none() {
        let current = std::thread::current();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", current.name().unwrap(), "--nocapture"])
            .env(WORKER, "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "call target restore subprocess failed: {}\n{}\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        eprint!("{}", String::from_utf8_lossy(&output.stderr));
        return;
    }
    for depth in [1000, 4000, 30_000] {
        for mode in 0..4 {
            for shared in [false, true] {
                let saved = snapshot_tree(CallTargetSnapshot::FlowItem(HirItemId(1)), depth, mode);
                let retained = shared.then(|| saved.clone());
                let mut context = RestoreContext::default();
                let (runtime, cost) = measure(|| {
                    RuntimeTree(Some(restore_call_target(saved, &mut context).unwrap()))
                });
                eprintln!(
                    "call target restore depth={depth} mode={mode} shared={shared}: {cost:?}"
                );
                let tables = if mode >= 2 {
                    2 + usize::from(shared)
                } else {
                    1
                };
                assert!(
                    cost.count <= depth * tables + 32,
                    "intermediate graph allocation: {cost:?}"
                );
                assert!(
                    cost.bytes <= depth * (384 + usize::from(shared) * 256) + 4096,
                    "unexpected output/frontier/aliased-identity cache cost: {cost:?}"
                );
                let mut cursor = runtime.0.as_ref().unwrap();
                for _ in 0..depth {
                    cursor = match (mode, cursor) {
                        (0, CallTarget::Limited { target, limits }) => {
                            assert!(limits.is_empty());
                            target
                        }
                        (
                            1,
                            CallTarget::Specialized {
                                target,
                                type_bindings,
                            },
                        ) => {
                            assert!(type_bindings.is_empty());
                            target
                        }
                        (2, CallTarget::Composed(targets)) => {
                            assert_eq!(targets.len(), 2);
                            assert!(matches!(targets[1], CallTarget::FlowItem(HirItemId(2))));
                            &targets[0]
                        }
                        (3, CallTarget::Composed(targets)) => {
                            assert_eq!(targets.len(), 2);
                            assert!(matches!(targets[0], CallTarget::FlowItem(HirItemId(2))));
                            &targets[1]
                        }
                        _ => panic!("restored target topology changed"),
                    };
                }
                assert!(matches!(cursor, CallTarget::FlowItem(HirItemId(1))));
                if let Some(retained) = retained {
                    let recaptured = capture_call_target(runtime.0.as_ref().unwrap()).unwrap();
                    assert!(retained == recaptured);
                }
            }
        }
    }
}

fn lambda(id: u64, value: i32) -> CallTargetSnapshot {
    use crate::{
        orchestration::{LocalsSnapshot, ValueSnapshot},
        value::InterpValue,
    };
    use etas_hir::{HirExprId, SymbolId};
    CallTargetSnapshot::Lambda {
        expr: HirExprId(4),
        captured: LocalsSnapshot {
            id,
            locals: std::rc::Rc::new(vec![(
                SymbolId(2),
                ValueSnapshot::capture(&InterpValue::i32(value)).unwrap(),
            )]),
            type_bindings: vec![("T".into(), etas_types::TypeId(3))],
        },
    }
}

#[test]
fn late_call_target_restore_errors_release_completed_runtime_subtrees() {
    use crate::orchestration::ValueSnapshot;
    use etas_hir::SymbolId;
    for mode in 0..4 {
        for error_kind in 0..3 {
            let mut bad = match error_kind {
                0 => lambda(0, 7),
                1 => lambda(1, 9),
                _ => lambda(2, 7),
            };
            if error_kind == 2 {
                let CallTargetSnapshot::Lambda { captured, .. } = &mut bad else {
                    unreachable!()
                };
                std::rc::Rc::make_mut(&mut captured.locals)
                    .push((SymbolId(2), ValueSnapshot::Unit));
            }
            let saved = snapshot_tree(
                CallTargetSnapshot::Composed(
                    vec![
                        lambda(1, 7),
                        snapshot_tree(CallTargetSnapshot::FlowItem(HirItemId(1)), 30_000, mode),
                        bad,
                        lambda(0, 9),
                    ]
                    .into(),
                ),
                30_000,
                0,
            );
            for _ in 0..2 {
                let (_, cost) = measure(|| {
                    let mut context = RestoreContext::default();
                    let error = match restore_call_target(saved.clone(), &mut context) {
                        Err(error) => error,
                        Ok(target) => {
                            drop(RuntimeTree(Some(target)));
                            panic!("invalid frame unexpectedly restored");
                        }
                    };
                    let expected = match error_kind {
                        0 => "snapshot frame identity must be nonzero",
                        1 => "conflicting definitions for snapshot frame identity",
                        _ => "snapshot frame contains duplicate local symbol",
                    };
                    assert!(error.contains(expected), "{error}");
                });
                assert_eq!(
                    cost.bytes, cost.released_bytes,
                    "late restore error retained temporary state: {cost:?}"
                );
            }
        }
    }
}

#[test]
fn restored_call_target_metadata_and_shared_frames_keep_their_identity() {
    use crate::{
        eval::limit::{RuntimeLimit, RuntimeLimitValue},
        value::InterpValue,
    };
    use etas_hir::SymbolId;
    use etas_types::TypeId;
    let depth = 4000;
    let mut saved =
        CallTargetSnapshot::Composed(vec![lambda(1, 7), lambda(1, 7), lambda(2, 7)].into());
    for i in 0..depth {
        saved = if i % 2 == 0 {
            CallTargetSnapshot::Specialized {
                target: saved.into(),
                type_bindings: vec![(format!("T{i}"), TypeId(i as u32))],
            }
        } else {
            CallTargetSnapshot::Limited {
                target: saved.into(),
                limits: vec![RuntimeLimit {
                    kind: etas_std::StdLimitKind::Attempts,
                    value: RuntimeLimitValue::Count(i as u64),
                    span: span(),
                }],
            }
        };
    }
    let mut context = RestoreContext::default();
    let runtime = RuntimeTree(Some(
        restore_call_target(saved.clone(), &mut context).unwrap(),
    ));
    let check_recaptured = |runtime: &CallTarget| {
        let recaptured = capture_call_target(runtime).unwrap();
        let (mut expected, mut actual) = (&saved, &recaptured);
        for _ in 0..depth {
            (expected, actual) = match (expected, actual) {
                (
                    CallTargetSnapshot::Specialized {
                        target: a,
                        type_bindings: ab,
                    },
                    CallTargetSnapshot::Specialized {
                        target: b,
                        type_bindings: bb,
                    },
                ) => {
                    assert_eq!(ab, bb);
                    (a, b)
                }
                (
                    CallTargetSnapshot::Limited {
                        target: a,
                        limits: al,
                    },
                    CallTargetSnapshot::Limited {
                        target: b,
                        limits: bl,
                    },
                ) => {
                    assert_eq!(al, bl);
                    (a, b)
                }
                _ => panic!("changed wrapper metadata"),
            };
        }
        let (CallTargetSnapshot::Composed(a), CallTargetSnapshot::Composed(b)) = (expected, actual)
        else {
            panic!("composed targets")
        };
        assert_eq!(a.len(), b.len());
        let mut ids = Vec::new();
        for (a, b) in a.iter().zip(b.iter()) {
            let (
                CallTargetSnapshot::Lambda {
                    expr: ae,
                    captured: ac,
                },
                CallTargetSnapshot::Lambda {
                    expr: be,
                    captured: bc,
                },
            ) = (a, b)
            else {
                panic!("lambda targets")
            };
            assert_eq!(ae, be);
            assert_eq!(ac.locals, bc.locals);
            assert_eq!(ac.type_bindings, bc.type_bindings);
            ids.push(bc.id);
        }
        // Restored frames get new runtime IDs, but alias topology must survive.
        assert_eq!(ids[0], ids[1]);
        assert_ne!(ids[0], ids[2]);
    };
    check_recaptured(runtime.0.as_ref().unwrap());
    let mut cursor = runtime.0.as_ref().unwrap();
    for _ in 0..depth {
        cursor = match cursor {
            CallTarget::Specialized { target, .. } | CallTarget::Limited { target, .. } => target,
            _ => panic!("restored wrapper"),
        };
    }
    let CallTarget::Composed(targets) = cursor else {
        panic!("composed target")
    };
    let frame = |index| match &targets[index] {
        CallTarget::Lambda { captured, .. } => captured.clone(),
        _ => panic!("lambda target"),
    };
    let mut first = frame(0);
    let alias = frame(1);
    let distinct = frame(2);
    assert!(first.set(SymbolId(2), InterpValue::i32(9)));
    assert_eq!(alias.get(SymbolId(2)), Some(InterpValue::i32(9)));
    assert_eq!(distinct.get(SymbolId(2)), Some(InterpValue::i32(7)));
    assert_eq!(first.type_bindings().get("T"), Some(&TypeId(3)));
    let mut independent_context = RestoreContext::default();
    let independent = RuntimeTree(Some(
        restore_call_target(saved.clone(), &mut independent_context).unwrap(),
    ));
    check_recaptured(independent.0.as_ref().unwrap());
}

#[test]
fn wide_call_target_restore_allocates_only_output_and_shared_header_copy() {
    for width in [1000, 4000, 30_000] {
        for shared in [false, true] {
            let saved = CallTargetSnapshot::Composed(
                (0..width)
                    .map(|i| CallTargetSnapshot::FlowItem(HirItemId(i as u32)))
                    .collect::<Vec<_>>()
                    .into(),
            );
            let retained = shared.then(|| saved.clone());
            let mut context = RestoreContext::default();
            let (runtime, cost) =
                measure(|| RuntimeTree(Some(restore_call_target(saved, &mut context).unwrap())));
            eprintln!("wide call target restore width={width} shared={shared}: {cost:?}");
            assert_eq!(cost.count, 3 + 2 * usize::from(shared));
            assert!(
                cost.bytes
                    <= width
                        * (size_of::<CallTarget>()
                            + usize::from(shared) * size_of::<CallTargetSnapshot>())
                        + 512
            );
            let CallTarget::Composed(targets) = runtime.0.as_ref().unwrap() else {
                panic!("composed target")
            };
            assert_eq!(targets.len(), width);
            for (i, target) in targets.iter().enumerate() {
                assert!(matches!(target, CallTarget::FlowItem(HirItemId(id)) if *id == i as u32));
            }
            drop(retained);
        }
    }
    let empty = restore_call_target(
        CallTargetSnapshot::Composed(vec![].into()),
        &mut RestoreContext::default(),
    )
    .unwrap();
    assert!(matches!(empty, CallTarget::Composed(targets) if targets.is_empty()));
}

#[test]
fn deep_call_targets_restore_through_checked_machine_boundary() {
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
    let limits = etas_host::StorageLimits::default();
    let current = crate::api::HostExecutionContext::default();
    let span = checked.hir.blocks.iter().next().unwrap().1.span;
    for depth in [1000, 4000, 30_000] {
        for invalid in [false, true] {
            let mut target = CallTargetSnapshot::FlowItem(if invalid {
                HirItemId(u32::MAX)
            } else {
                checked.entry.unwrap()
            });
            for _ in 0..depth {
                target = CallTargetSnapshot::Composed(
                    vec![target, CallTargetSnapshot::FlowItem(checked.entry.unwrap())].into(),
                );
            }
            let saved = MachineSnapshot {
                frames: vec![MachineFrameSnapshot::Continuation {
                    continuation: ContinuationSnapshot::CallArgs {
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
                    },
                }],
            };
            let result = EvalMachine::from_snapshot(
                &saved,
                &checked,
                plan.slots.clone(),
                &plan.dispatch,
                &plan.closures,
                &current,
                &limits,
            );
            match result {
                Ok(mut machine) => {
                    assert_eq!(machine.frames().len(), 1);
                    let Continuation::CallArgs { target, .. } =
                        machine.pop_frame().unwrap().into_continuation()
                    else {
                        panic!("call args frame")
                    };
                    let runtime = RuntimeTree(Some(target));
                    assert!(!invalid, "unchecked item was restored");
                    let MachineFrameSnapshot::Continuation {
                        continuation: ContinuationSnapshot::CallArgs { target, .. },
                    } = &saved.frames[0]
                    else {
                        panic!("saved call args frame")
                    };
                    assert!(capture_call_target(runtime.0.as_ref().unwrap()).unwrap() == *target);
                }
                Err(error) => {
                    assert!(invalid, "{error}");
                    assert!(error.contains("missing HIR item"), "{error}");
                }
            }
        }
    }
}
