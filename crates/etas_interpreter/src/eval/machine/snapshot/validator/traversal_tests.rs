use super::*;

fn machine(continuation: ContinuationSnapshot) -> MachineSnapshot {
    MachineSnapshot {
        frames: vec![MachineFrameSnapshot::Continuation { continuation }],
    }
}

fn shared_dag(mut root: ContinuationSnapshot, depth: usize) -> ContinuationSnapshot {
    for _ in 0..depth {
        let child: crate::orchestration::ContinuationSnapshotLink = root.into();
        root = ContinuationSnapshot::Chain {
            inner: child.clone(),
            outer: child,
        };
    }
    root
}

#[test]
fn shared_dag_machine_restore_runs_checked_validation_without_expansion() {
    use crate::{control::Continuation, eval::machine::state::EvalMachine};
    let checked = crate::testing::project::checked_project(
        "module app.main; flow main() -> unit { return; }",
    );
    let plan = crate::Interpreter
        .plan(&checked, crate::api::PlanOptions)
        .plan
        .unwrap();
    let limits = etas_host::StorageLimits::default();
    let host = crate::api::HostExecutionContext::default();
    for depth in [10, 1000, 4000, 30_000] {
        let saved = checkpoint(&checked, shared_dag(ContinuationSnapshot::Return, depth), 0);
        let (_, cost) = crate::testing::allocation::measure(|| {
            SnapshotValidator::new(
                &checked,
                &plan.slots,
                &plan.dispatch,
                &plan.closures,
                &limits,
            )
            .validate_checkpoint(&saved)
            .unwrap();
            let mut live = EvalMachine::from_snapshot(
                &saved.machine,
                &checked,
                plan.slots.clone(),
                &plan.dispatch,
                &plan.closures,
                &host,
                &limits,
            )
            .unwrap();
            let root = live.pop_frame().unwrap().into_continuation();
            let mut cursor = &root;
            for _ in 0..depth {
                let Continuation::Chain { inner, outer } = cursor else {
                    panic!("chain")
                };
                assert!(std::ptr::eq(inner.as_ref(), outer.as_ref()));
                cursor = inner;
            }
            assert!(matches!(cursor, Continuation::Return));
        });
        eprintln!("checked DAG restore depth={depth}: {cost:?}");
        assert!(
            cost.count < depth + 160,
            "repeated snapshot materialization: {cost:?}"
        );
        assert_eq!(cost.bytes, cost.released_bytes);
    }
}

#[test]
fn shared_dag_validation_keeps_duplicate_scopes_and_invalid_leaves_fail_closed() {
    let checked = crate::testing::project::checked_project(
        "module app.main; flow main() -> unit { return; }",
    );
    let plan = crate::Interpreter
        .plan(&checked, crate::api::PlanOptions)
        .plan
        .unwrap();
    let limits = etas_host::StorageLimits::default();
    let scope_free = shared_dag(ContinuationSnapshot::Return, 4000);
    let repeated_scope = shared_dag(boundary(0, scope_free.clone()), 4000);
    let saved = checkpoint(&checked, repeated_scope, 1);
    let error = SnapshotValidator::new(
        &checked,
        &plan.slots,
        &plan.dispatch,
        &plan.closures,
        &limits,
    )
    .validate_checkpoint(&saved)
    .unwrap_err();
    assert!(error.contains("duplicate handle boundary"), "{error}");
    let bad = ContinuationSnapshot::ContinueBlock {
        block: HirBlockId(u32::MAX),
        next_stmt_index: 0,
        frame: LocalsSnapshot {
            id: 1,
            locals: Default::default(),
            type_bindings: vec![],
        },
    };
    for root in [
        shared_dag(bad.clone(), 4000),
        ContinuationSnapshot::Chain {
            inner: scope_free.into(),
            outer: bad.into(),
        },
    ] {
        let error = SnapshotValidator::new(
            &checked,
            &plan.slots,
            &plan.dispatch,
            &plan.closures,
            &limits,
        )
        .validate_machine(&machine(root))
        .unwrap_err();
        assert!(error.contains("block"), "{error}");
    }
}

#[test]
fn deep_continuation_validation_does_not_overflow_or_copy_snapshot_edges() {
    const WORKER: &str = "ETAS_TEST_CONTINUATION_VALIDATION_WORKER";
    if std::env::var_os(WORKER).is_none() {
        let current = std::thread::current();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", current.name().unwrap(), "--nocapture"])
            .env(WORKER, "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "validator subprocess failed: {}\n{}\n{}",
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
    for depth in [1000, 4000, 30_000] {
        for mode in 0..3 {
            let mut root = ContinuationSnapshot::Return;
            for _ in 0..depth {
                root = match mode {
                    0 => ContinuationSnapshot::CallBoundary { outer: root.into() },
                    1 => ContinuationSnapshot::Chain {
                        inner: root.into(),
                        outer: ContinuationSnapshot::Finish.into(),
                    },
                    _ => ContinuationSnapshot::Chain {
                        inner: ContinuationSnapshot::Resume.into(),
                        outer: root.into(),
                    },
                };
            }
            let snapshot = machine(root);
            let validator = SnapshotValidator::new(
                &checked,
                &plan.slots,
                &plan.dispatch,
                &plan.closures,
                &limits,
            );
            let (result, cost) =
                crate::testing::allocation::measure(|| validator.validate_machine(&snapshot));
            result.unwrap();
            eprintln!("continuation validation depth={depth} mode={mode}: {cost:?}");
            assert!(
                cost.count < 40,
                "snapshot nodes copied during validation: {cost:?}"
            );
            assert!(cost.bytes < 4096 + depth * 128);
        }
    }
}

fn span() -> etas_core::Span {
    etas_core::Span::empty(etas_core::SourceId(0), etas_core::TextSize::ZERO)
}

fn boundary(scope_id: u32, inner: ContinuationSnapshot) -> ContinuationSnapshot {
    ContinuationSnapshot::HandleBoundary {
        scope_id: HandlerScopeId(scope_id),
        inner: inner.into(),
        handlers: vec![],
        span: span(),
        frame: LocalsSnapshot {
            id: 1,
            locals: Default::default(),
            type_bindings: vec![],
        },
    }
}

fn checkpoint(
    checked: &CheckedProject,
    root: ContinuationSnapshot,
    scopes: usize,
) -> InterpreterCheckpoint {
    use crate::orchestration::*;
    let entry = checked.entry.unwrap();
    InterpreterCheckpoint {
        id: CheckpointId(1),
        label: None,
        compilation: CheckpointCompilationIdentity::for_project(checked, entry).unwrap(),
        entry_item: entry,
        args: vec![],
        machine: machine(root),
        handlers: HandlerSnapshot {
            handlers: (0..scopes)
                .map(|id| ActiveHandlerRecord {
                    id: HandlerScopeId(id as u32),
                    handled_actions: vec![],
                    handlers: vec![],
                    span: span(),
                })
                .collect(),
        },
        retry_state: Default::default(),
        trace: Default::default(),
        execution_progress: ExecutionProgressSnapshot {
            consumed_steps: 0,
            original_limits: Default::default(),
        },
        host_state: CheckpointHostState {
            trace: etas_host::TraceContext::root(etas_host::TraceId(1)),
            budget: CheckpointBudgetSnapshot::capture(&etas_host::ExecutionBudget::default())
                .unwrap(),
        },
        storage: StorageSnapshot {
            identity: etas_host::StorageOperationKey::new(std::time::Duration::from_secs(3600))
                .unwrap(),
            operations: Default::default(),
            writes: vec![],
        },
        current_session: None,
        resource_versions: Default::default(),
        completed_host_boundaries: Default::default(),
    }
}

fn wrap(mut root: ContinuationSnapshot, depth: usize) -> ContinuationSnapshot {
    for _ in 0..depth {
        root = ContinuationSnapshot::CallBoundary { outer: root.into() };
    }
    root
}

#[test]
fn full_checkpoint_validation_keeps_deep_ordered_handler_topology() {
    let checked = crate::testing::project::checked_project(
        "module app.main; flow main() -> unit { return; }",
    );
    let plan = crate::Interpreter
        .plan(&checked, crate::api::PlanOptions)
        .plan
        .unwrap();
    let limits = etas_host::StorageLimits::default();
    for depth in [1000, 4000, 30_000] {
        let mut root = ContinuationSnapshot::Return;
        for id in (0..depth).rev() {
            root = boundary(id as u32, root);
        }
        let saved = checkpoint(&checked, root, depth);
        let validator = SnapshotValidator::new(
            &checked,
            &plan.slots,
            &plan.dispatch,
            &plan.closures,
            &limits,
        );
        validator.validate_checkpoint(&saved).unwrap();
        assert_eq!(
            validator.handler_scope_unwind_order(&saved.machine),
            (0..depth)
                .rev()
                .map(|id| HandlerScopeId(id as u32))
                .collect::<Vec<_>>()
        );
    }
    // Same ID set, wrong actual unwind order. Deep wrappers must not hide it.
    let reordered = ContinuationSnapshot::Chain {
        inner: boundary(0, ContinuationSnapshot::Return).into(),
        outer: boundary(1, ContinuationSnapshot::Return).into(),
    };
    let reordered = checkpoint(&checked, wrap(reordered, 30_000), 2);
    let error = SnapshotValidator::new(
        &checked,
        &plan.slots,
        &plan.dispatch,
        &plan.closures,
        &limits,
    )
    .validate_checkpoint(&reordered)
    .unwrap_err();
    assert!(error.contains("topology mismatch"), "{error}");
    let duplicated = checkpoint(
        &checked,
        wrap(
            boundary(0, boundary(0, ContinuationSnapshot::Return)),
            30_000,
        ),
        1,
    );
    let error = SnapshotValidator::new(
        &checked,
        &plan.slots,
        &plan.dispatch,
        &plan.closures,
        &limits,
    )
    .validate_checkpoint(&duplicated)
    .unwrap_err();
    assert!(error.contains("duplicate handle boundary"), "{error}");
}

#[test]
fn deep_validator_preserves_leaf_checks_and_handler_validation_order() {
    let checked = crate::testing::project::checked_project(
        "module app.main; flow main() -> unit { return; }",
    );
    let plan = crate::Interpreter
        .plan(&checked, crate::api::PlanOptions)
        .plan
        .unwrap();
    let limits = etas_host::StorageLimits::default();
    let validate = |node| {
        SnapshotValidator::new(
            &checked,
            &plan.slots,
            &plan.dispatch,
            &plan.closures,
            &limits,
        )
        .validate_machine(&machine(wrap(node, 30_000)))
        .unwrap_err()
    };
    let bad_block = || ContinuationSnapshot::ContinueBlock {
        block: HirBlockId(u32::MAX),
        next_stmt_index: 0,
        frame: LocalsSnapshot {
            id: 1,
            locals: Default::default(),
            type_bindings: vec![],
        },
    };
    let error = validate(ContinuationSnapshot::Chain {
        inner: ContinuationSnapshot::Return.into(),
        outer: bad_block().into(),
    });
    assert!(error.contains("block"), "{error}");
    let error = validate(boundary(0, boundary(0, bad_block())));
    assert!(error.contains("duplicate handle boundary"), "{error}");
    for bad_inner in [true, false] {
        let mut node = boundary(
            0,
            if bad_inner {
                bad_block()
            } else {
                ContinuationSnapshot::Return
            },
        );
        let ContinuationSnapshot::HandleBoundary { frame, .. } = &mut node else {
            unreachable!()
        };
        frame.id = 0;
        let error = validate(node);
        if bad_inner {
            assert!(error.contains("block"), "{error}");
        } else {
            assert!(error.contains("zero frame identity"), "{error}");
        }
    }
}

#[test]
fn deep_machine_snapshot_passes_checked_validation_then_restores() {
    use crate::{control::Continuation, eval::machine::state::EvalMachine};
    let checked = crate::testing::project::checked_project(
        "module app.main; flow main() -> unit { return; }",
    );
    let plan = crate::Interpreter
        .plan(&checked, crate::api::PlanOptions)
        .plan
        .unwrap();
    let limits = etas_host::StorageLimits::default();
    let host = crate::api::HostExecutionContext::default();
    for depth in [1000, 4000, 30_000] {
        let snapshot = machine(wrap(ContinuationSnapshot::Return, depth));
        let mut restored = EvalMachine::from_snapshot(
            &snapshot,
            &checked,
            plan.slots.clone(),
            &plan.dispatch,
            &plan.closures,
            &host,
            &limits,
        )
        .unwrap();
        assert_eq!(restored.frames().len(), 1);
        let root = restored.pop_frame().unwrap().into_continuation();
        let mut node = &root;
        let mut edges = 0;
        while let Continuation::CallBoundary { outer } = node {
            node = outer;
            edges += 1;
        }
        assert_eq!(edges, depth);
        assert!(matches!(node, Continuation::Return));
        drop(root);
    }
}
