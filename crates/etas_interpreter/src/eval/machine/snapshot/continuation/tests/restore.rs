use super::{Continuation, ContinuationSnapshot, RuntimeTree, measure};
use crate::eval::machine::snapshot::RestoreContext;
use crate::{
    orchestration::{HandlerScopeId, LocalsSnapshot, ModelExecutionPolicySnapshot, ValueSnapshot},
    value::InterpValue,
};

#[test]
fn deep_continuation_restore_uses_explicit_traversal_without_stack_overflow() {
    const WORKER: &str = "ETAS_TEST_CONTINUATION_RESTORE_WORKER";
    if std::env::var_os(WORKER).is_none() {
        let current = std::thread::current();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", current.name().unwrap(), "--nocapture"])
            .env(WORKER, "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "restore subprocess failed: {}\n{}\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
        eprint!("{}", String::from_utf8_lossy(&output.stderr));
        return;
    }
    for depth in [1000, 4000, 30_000] {
        for mode in 0..3 {
            for shared in [false, true] {
                let mut snapshot = ContinuationSnapshot::Return;
                for _ in 0..depth {
                    snapshot = match mode {
                        0 => ContinuationSnapshot::CallBoundary {
                            outer: snapshot.into(),
                        },
                        1 => ContinuationSnapshot::Chain {
                            inner: snapshot.into(),
                            outer: ContinuationSnapshot::Finish.into(),
                        },
                        _ => ContinuationSnapshot::Chain {
                            inner: ContinuationSnapshot::Resume.into(),
                            outer: snapshot.into(),
                        },
                    };
                }
                let retained = shared.then(|| snapshot.clone());
                let (restored, cost) = measure(|| {
                    snapshot
                        .restore_with(&mut RestoreContext::default())
                        .unwrap()
                });
                let runtime = RuntimeTree(Some(restored));
                eprintln!(
                    "continuation restore depth={depth} mode={mode} shared={shared}: {cost:?}"
                );
                let edges = depth * if mode == 0 { 1 } else { 2 };
                assert!(cost.count <= edges + 32, "copied snapshot graph: {cost:?}");
                assert!(cost.bytes <= edges * size_of::<Continuation>() + depth * 512 + 4096);
                let mut cursor = runtime.0.as_ref().unwrap();
                for _ in 0..depth {
                    cursor = match (mode, cursor) {
                        (0, Continuation::CallBoundary { outer }) => outer,
                        (1, Continuation::Chain { inner, outer }) => {
                            assert!(matches!(**outer, Continuation::Finish));
                            inner
                        }
                        (2, Continuation::Chain { inner, outer }) => {
                            assert!(matches!(**inner, Continuation::Resume));
                            outer
                        }
                        _ => panic!("wrong restored continuation topology"),
                    };
                }
                assert!(matches!(cursor, Continuation::Return));
                if let Some(retained) = retained {
                    let mut cursor = &retained;
                    for _ in 0..depth {
                        cursor = match (mode, cursor) {
                            (0, ContinuationSnapshot::CallBoundary { outer }) => outer,
                            (1, ContinuationSnapshot::Chain { inner, .. }) => inner,
                            (2, ContinuationSnapshot::Chain { outer, .. }) => outer,
                            _ => panic!("changed retained snapshot"),
                        };
                    }
                    assert!(matches!(cursor, ContinuationSnapshot::Return));
                }
            }
        }
    }
}

fn frame(id: u64) -> LocalsSnapshot {
    LocalsSnapshot {
        id,
        locals: std::rc::Rc::new(vec![(
            etas_hir::SymbolId(1),
            ValueSnapshot::capture(&InterpValue::i32(7)).unwrap(),
        )]),
        type_bindings: vec![],
    }
}

fn policy(rounds: usize) -> Box<ModelExecutionPolicySnapshot> {
    Box::new(super::super::capture_model_policy(
        &crate::api::ModelExecutionPolicy {
            max_tool_rounds: rounds,
            ..Default::default()
        },
    ))
}

#[test]
fn mixed_restore_preserves_metadata_frame_aliases_and_retained_definitions() {
    for same_frame in [false, true] {
        let saved = frame(7);
        let mut snapshot = ContinuationSnapshot::ContinueBlock {
            block: etas_hir::HirBlockId(3),
            next_stmt_index: 5,
            frame: if same_frame { saved.clone() } else { frame(8) },
        };
        let depth = 4000;
        for i in 0..depth {
            snapshot = match i % 5 {
                0 => ContinuationSnapshot::CallBoundary {
                    outer: snapshot.into(),
                },
                1 => ContinuationSnapshot::HandlerDispatch {
                    outer: snapshot.into(),
                },
                2 => ContinuationSnapshot::HandleBoundary {
                    scope_id: HandlerScopeId(i as u32),
                    inner: snapshot.into(),
                    handlers: vec![],
                    span: super::span(),
                    frame: saved.clone(),
                },
                3 => ContinuationSnapshot::RestoreModelPolicy {
                    previous: policy(i),
                    inner: snapshot.into(),
                },
                _ => ContinuationSnapshot::ScopedModelPolicy {
                    policy: policy(i),
                    inner: snapshot.into(),
                },
            };
        }
        let retained = snapshot.clone();
        let runtime = RuntimeTree(Some(
            snapshot
                .restore_with(&mut RestoreContext::default())
                .unwrap(),
        ));
        let mut cursor = runtime.0.as_ref().unwrap();
        let mut outer_frame = None;
        for i in (0..depth).rev() {
            cursor = match (i % 5, cursor) {
                (0, Continuation::CallBoundary { outer })
                | (1, Continuation::HandlerDispatch { outer }) => outer,
                (
                    2,
                    Continuation::HandleBoundary {
                        scope_id,
                        inner,
                        handlers,
                        span,
                        frame,
                    },
                ) => {
                    assert_eq!(*scope_id, HandlerScopeId(i as u32));
                    assert_eq!(*span, super::span());
                    assert!(handlers.is_empty());
                    outer_frame.get_or_insert_with(|| frame.clone());
                    inner
                }
                (3, Continuation::RestoreModelPolicy { previous, inner }) => {
                    assert_eq!(previous.max_tool_rounds, i);
                    inner
                }
                (4, Continuation::ScopedModelPolicy { policy, inner }) => {
                    assert_eq!(policy.max_tool_rounds, i);
                    inner
                }
                _ => panic!("wrong restored wrapper"),
            };
        }
        let Continuation::ContinueBlock {
            block,
            next_stmt_index,
            frame: leaf,
        } = cursor
        else {
            panic!("missing restored block")
        };
        assert_eq!(*block, etas_hir::HirBlockId(3));
        assert_eq!(*next_stmt_index, 5);
        assert!(
            outer_frame
                .unwrap()
                .set(etas_hir::SymbolId(1), InterpValue::i32(9))
        );
        assert_eq!(
            leaf.get(etas_hir::SymbolId(1)),
            Some(InterpValue::i32(if same_frame { 9 } else { 7 }))
        );
        // Runtime mutations must not alter any retained snapshot definition.
        let mut cursor = &retained;
        loop {
            cursor = match cursor {
                ContinuationSnapshot::CallBoundary { outer }
                | ContinuationSnapshot::HandlerDispatch { outer } => outer,
                ContinuationSnapshot::RestoreModelPolicy { inner, .. }
                | ContinuationSnapshot::ScopedModelPolicy { inner, .. } => inner,
                ContinuationSnapshot::HandleBoundary { inner, frame, .. } => {
                    assert_eq!(frame, &saved);
                    inner
                }
                ContinuationSnapshot::ContinueBlock { frame, .. } => {
                    assert_eq!(frame.locals, saved.locals);
                    break;
                }
                _ => panic!("changed retained snapshot"),
            };
        }
    }
}

#[test]
fn restore_rejects_late_invalid_frames_and_releases_completed_deep_branches() {
    for bad_parent in [false, true] {
        for conflicting in [false, true] {
            let mut inner = ContinuationSnapshot::ContinueBlock {
                block: etas_hir::HirBlockId(1),
                next_stmt_index: 0,
                frame: frame(7),
            };
            let shared_frame = frame(7);
            for i in 0..30_000 {
                inner = match i % 6 {
                    0 => ContinuationSnapshot::CallBoundary {
                        outer: inner.into(),
                    },
                    1 => ContinuationSnapshot::HandlerDispatch {
                        outer: inner.into(),
                    },
                    2 => ContinuationSnapshot::HandleBoundary {
                        scope_id: HandlerScopeId(i as u32 + 2),
                        inner: inner.into(),
                        handlers: vec![],
                        span: super::span(),
                        frame: shared_frame.clone(),
                    },
                    3 => ContinuationSnapshot::RestoreModelPolicy {
                        previous: policy(i),
                        inner: inner.into(),
                    },
                    4 => ContinuationSnapshot::ScopedModelPolicy {
                        policy: policy(i),
                        inner: inner.into(),
                    },
                    _ => ContinuationSnapshot::Chain {
                        inner: inner.into(),
                        outer: ContinuationSnapshot::BlockValue.into(),
                    },
                };
            }
            let mut bad = frame(if conflicting { 7 } else { 0 });
            if conflicting {
                std::rc::Rc::make_mut(&mut bad.locals)[0].1 = ValueSnapshot::Unit;
            }
            let snapshot = if bad_parent {
                ContinuationSnapshot::HandleBoundary {
                    scope_id: HandlerScopeId(1),
                    inner: inner.into(),
                    handlers: vec![],
                    span: super::span(),
                    frame: bad,
                }
            } else {
                ContinuationSnapshot::Chain {
                    inner: inner.into(),
                    outer: ContinuationSnapshot::ContinueBlock {
                        block: etas_hir::HirBlockId(1),
                        next_stmt_index: 0,
                        frame: bad,
                    }
                    .into(),
                }
            };
            for _ in 0..2 {
                let result = snapshot
                    .clone()
                    .restore_with(&mut RestoreContext::default());
                let error = match result {
                    Ok(value) => {
                        drop(RuntimeTree(Some(value)));
                        panic!("accepted malformed frame definition")
                    }
                    Err(error) => error,
                };
                let expected = if conflicting {
                    "conflicting definitions"
                } else {
                    "nonzero"
                };
                assert!(error.contains(expected), "{error}");
            }
        }
    }
}
