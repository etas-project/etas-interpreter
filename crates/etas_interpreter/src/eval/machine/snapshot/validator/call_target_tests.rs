use super::*;

// Owning these targets also exercises the production snapshot edge release path.
struct TargetTree(Option<CallTargetSnapshot>);

fn tree(mut leaf: CallTargetSnapshot, depth: usize, mode: usize) -> TargetTree {
    for _ in 0..depth {
        leaf = match mode {
            0 => CallTargetSnapshot::Limited {
                target: leaf.into(),
                limits: vec![],
            },
            1 => CallTargetSnapshot::Specialized {
                target: leaf.into(),
                type_bindings: vec![],
            },
            2 => CallTargetSnapshot::Composed(
                (vec![leaf, CallTargetSnapshot::Composed((vec![]).into())]).into(),
            ),
            _ => CallTargetSnapshot::Composed(
                (vec![CallTargetSnapshot::Composed((vec![]).into()), leaf]).into(),
            ),
        };
    }
    TargetTree(Some(leaf))
}

#[test]
fn deep_call_target_validation_borrows_graph_without_recursive_descent() {
    const WORKER: &str = "ETAS_TEST_CALL_TARGET_VALIDATION_WORKER";
    if std::env::var_os(WORKER).is_none() {
        let current = std::thread::current();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", current.name().unwrap(), "--nocapture"])
            .env(WORKER, "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "call target validator subprocess failed: {}\n{}\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        eprint!("{}", String::from_utf8_lossy(&output.stderr));
        return;
    }
    let checked = crate::testing::project::checked_project(
        "module app.main; flow main() -> unit { return; }",
    );
    let plan = crate::Interpreter
        .plan(&checked, crate::api::PlanOptions)
        .plan
        .unwrap();
    let limits = etas_host::StorageLimits::default();
    let validator = SnapshotValidator::new(
        &checked,
        &plan.slots,
        &plan.dispatch,
        &plan.closures,
        &limits,
    );
    for depth in [1000, 4000, 30_000] {
        for mode in 0..4 {
            let root = tree(
                CallTargetSnapshot::FlowItem(checked.entry.unwrap()),
                depth,
                mode,
            );
            let (result, cost) = crate::testing::allocation::measure(|| {
                validator.call_target(root.0.as_ref().unwrap(), "call target regression")
            });
            result.unwrap();
            eprintln!("call target validation depth={depth} mode={mode}: {cost:?}");
            assert!(cost.count < 32, "per-node allocation: {cost:?}");
            assert!(
                cost.bytes < 4096 + depth * 128,
                "unexpected frontier cost: {cost:?}"
            );
            assert_eq!(
                cost.bytes, cost.released_bytes,
                "retained validation allocation"
            );
        }
    }
}

#[test]
fn cloning_checkpoint_call_target_does_not_copy_all_descendants() {
    for depth in [32, 1000, 4000, 30_000] {
        let root = tree(CallTargetSnapshot::FlowItem(HirItemId(1)), depth, 0);
        let (retained, cost) =
            crate::testing::allocation::measure(|| root.0.as_ref().unwrap().clone());
        let retained = TargetTree(Some(retained));
        assert_eq!(
            cost.count, 0,
            "copied target graph at depth {depth}: {cost:?}"
        );
        drop(root);
        let (_, release) = crate::testing::allocation::measure(|| drop(retained));
        assert!(
            release.count <= 1,
            "unary release allocated per node: {release:?}"
        );
    }
}

#[test]
fn call_target_validation_preserves_child_binding_and_sibling_error_order() {
    let checked = crate::testing::project::checked_project(
        "module app.main; flow main() -> unit { return; }",
    );
    let plan = crate::Interpreter
        .plan(&checked, crate::api::PlanOptions)
        .plan
        .unwrap();
    let limits = etas_host::StorageLimits::default();
    let validator = SnapshotValidator::new(
        &checked,
        &plan.slots,
        &plan.dispatch,
        &plan.closures,
        &limits,
    );
    let good = || CallTargetSnapshot::FlowItem(checked.entry.unwrap());
    let bad = || CallTargetSnapshot::FlowItem(HirItemId(u32::MAX));
    let specialize = |target: CallTargetSnapshot, name: &str| CallTargetSnapshot::Specialized {
        target: target.into(),
        type_bindings: vec![(name.to_owned(), TypeId(u32::MAX))],
    };
    for (node, message) in [
        (specialize(bad(), ""), "missing HIR item"),
        (
            specialize(good(), ""),
            "invalid or duplicate type parameter",
        ),
        (specialize(good(), "T"), "missing checked type"),
        (
            CallTargetSnapshot::Composed((vec![bad(), specialize(good(), "")]).into()),
            "missing HIR item",
        ),
        (
            CallTargetSnapshot::Composed((vec![specialize(good(), ""), bad()]).into()),
            "invalid or duplicate type parameter",
        ),
    ] {
        let root = tree(node, 30_000, 0);
        let (error, cost) = crate::testing::allocation::measure(|| {
            validator
                .call_target(root.0.as_ref().unwrap(), "target order")
                .unwrap_err()
        });
        assert!(error.contains(message), "expected {message}: {error}");
        assert!(
            cost.count < 32,
            "unexpected error frontier allocation: {cost:?}"
        );
    }
}

#[test]
fn wide_composed_call_target_validation_does_not_copy_or_queue_all_siblings() {
    let checked = crate::testing::project::checked_project(
        "module app.main; flow main() -> unit { return; }",
    );
    let plan = crate::Interpreter
        .plan(&checked, crate::api::PlanOptions)
        .plan
        .unwrap();
    let limits = etas_host::StorageLimits::default();
    let validator = SnapshotValidator::new(
        &checked,
        &plan.slots,
        &plan.dispatch,
        &plan.closures,
        &limits,
    );
    for width in [1000, 4000, 30_000] {
        let root = TargetTree(Some(CallTargetSnapshot::Composed(
            ((0..width)
                .map(|_| CallTargetSnapshot::FlowItem(checked.entry.unwrap()))
                .collect::<Vec<_>>())
            .into(),
        )));
        let (result, cost) = crate::testing::allocation::measure(|| {
            validator.call_target(root.0.as_ref().unwrap(), "wide target")
        });
        result.unwrap();
        eprintln!("call target validation width={width}: {cost:?}");
        assert!(
            cost.count <= 1 && cost.bytes <= 256,
            "queued or copied every sibling: {cost:?}"
        );
    }
}

#[test]
fn deep_specialized_target_validates_every_nonempty_binding_after_its_child() {
    let checked = crate::testing::project::checked_project(
        "module app.main; flow main() -> unit { return; }",
    );
    let plan = crate::Interpreter
        .plan(&checked, crate::api::PlanOptions)
        .plan
        .unwrap();
    let limits = etas_host::StorageLimits::default();
    let validator = SnapshotValidator::new(
        &checked,
        &plan.slots,
        &plan.dispatch,
        &plan.closures,
        &limits,
    );
    let ty = checked.type_store.iter().next().unwrap().0;
    for invalid_at in [None, Some(0), Some(14_999), Some(29_999)] {
        let mut target = CallTargetSnapshot::FlowItem(checked.entry.unwrap());
        for index in 0..30_000 {
            target = CallTargetSnapshot::Specialized {
                target: target.into(),
                type_bindings: if invalid_at == Some(index) {
                    vec![("T".into(), ty), ("T".into(), ty)]
                } else {
                    vec![("T".into(), ty)]
                },
            };
        }
        let root = TargetTree(Some(target));
        let result = validator.call_target(root.0.as_ref().unwrap(), "specialized target");
        if invalid_at.is_some() {
            assert!(
                result
                    .unwrap_err()
                    .contains("invalid or duplicate type parameter")
            );
        } else {
            result.unwrap();
        }
    }
}

#[test]
fn machine_validation_checks_deep_call_target_before_argument_frame() {
    let checked = crate::testing::project::checked_project(
        "module app.main; flow main() -> unit { return; }",
    );
    let plan = crate::Interpreter
        .plan(&checked, crate::api::PlanOptions)
        .plan
        .unwrap();
    let limits = etas_host::StorageLimits::default();
    let span = checked.hir.blocks.iter().next().unwrap().1.span;
    for (valid_target, frame_id) in [(true, 1), (true, 0), (false, 0)] {
        let mut root = tree(
            CallTargetSnapshot::FlowItem(if valid_target {
                checked.entry.unwrap()
            } else {
                HirItemId(u32::MAX)
            }),
            30_000,
            2,
        );
        let mut machine = MachineSnapshot {
            frames: vec![MachineFrameSnapshot::Continuation {
                continuation: ContinuationSnapshot::CallArgs {
                    target: root.0.take().unwrap(),
                    args: vec![].into(),
                    next_arg_index: 0,
                    evaluated_args: vec![],
                    span,
                    frame: LocalsSnapshot {
                        id: frame_id,
                        locals: Default::default(),
                        type_bindings: vec![],
                    },
                },
            }],
        };
        let result = SnapshotValidator::new(
            &checked,
            &plan.slots,
            &plan.dispatch,
            &plan.closures,
            &limits,
        )
        .validate_machine(&machine);
        // Return the target to its owning test value before checking the result.
        let MachineFrameSnapshot::Continuation {
            continuation: ContinuationSnapshot::CallArgs { target, .. },
        } = machine.frames.pop().unwrap()
        else {
            unreachable!()
        };
        root.0 = Some(target);
        match (valid_target, frame_id) {
            (true, 1) => result.unwrap(),
            (true, _) => assert!(result.unwrap_err().contains("zero frame identity")),
            (false, _) => assert!(result.unwrap_err().contains("missing HIR item")),
        }
    }
}
