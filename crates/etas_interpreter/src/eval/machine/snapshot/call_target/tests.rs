use super::*;
use crate::testing::allocation::measure;
use etas_hir::HirItemId;

// Runtime CallTarget destruction remains a separate audit. This guard isolates
// borrowed capture; saved targets use their normal production release path.
struct RuntimeTree(Option<CallTarget>);

impl Drop for RuntimeTree {
    fn drop(&mut self) {
        let mut pending: Vec<_> = self.0.take().into_iter().collect();
        while let Some(node) = pending.pop() {
            match node {
                CallTarget::Specialized { target, .. } | CallTarget::Limited { target, .. } => {
                    pending.push(*target)
                }
                CallTarget::Composed(targets) => pending.extend(targets),
                _ => {}
            }
        }
    }
}

fn tree(mut node: CallTarget, depth: usize, mode: usize) -> RuntimeTree {
    for _ in 0..depth {
        node = match mode {
            0 => CallTarget::Limited {
                target: Box::new(node),
                limits: vec![],
            },
            1 => CallTarget::Specialized {
                target: Box::new(node),
                type_bindings: vec![],
            },
            2 => CallTarget::Composed(vec![node, CallTarget::FlowItem(HirItemId(2))]),
            _ => CallTarget::Composed(vec![CallTarget::FlowItem(HirItemId(2)), node]),
        };
    }
    RuntimeTree(Some(node))
}

#[test]
fn deep_call_target_capture_is_stack_safe_and_allocates_only_snapshot_and_frontier() {
    const WORKER: &str = "ETAS_TEST_CALL_TARGET_CAPTURE_WORKER";
    if std::env::var_os(WORKER).is_none() {
        let current = std::thread::current();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", current.name().unwrap(), "--nocapture"])
            .env(WORKER, "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "call target capture subprocess failed: {}\n{}\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        eprint!("{}", String::from_utf8_lossy(&output.stderr));
        return;
    }
    for depth in [1000, 4000, 30_000] {
        for mode in 0..4 {
            let runtime = tree(CallTarget::FlowItem(HirItemId(1)), depth, mode);
            let (saved, cost) =
                measure(|| capture_call_target(runtime.0.as_ref().unwrap()).unwrap());
            let nodes = depth * if mode < 2 { 1 } else { 2 };
            eprintln!("call target capture depth={depth} mode={mode}: {cost:?}");
            assert!(
                cost.count <= nodes + 32,
                "intermediate graph allocation: {cost:?}"
            );
            assert!(
                cost.bytes <= depth * 320 + 4096,
                "unbounded frontier or intermediate nodes: {cost:?}"
            );
            let mut cursor = &saved;
            for _ in 0..depth {
                cursor = match (mode, cursor) {
                    (0, CallTargetSnapshot::Limited { target, limits }) => {
                        assert!(limits.is_empty());
                        target
                    }
                    (
                        1,
                        CallTargetSnapshot::Specialized {
                            target,
                            type_bindings,
                        },
                    ) => {
                        assert!(type_bindings.is_empty());
                        target
                    }
                    (2, CallTargetSnapshot::Composed(targets)) => {
                        assert_eq!(targets.len(), 2);
                        assert!(matches!(
                            targets[1],
                            CallTargetSnapshot::FlowItem(HirItemId(2))
                        ));
                        &targets[0]
                    }
                    (3, CallTargetSnapshot::Composed(targets)) => {
                        assert_eq!(targets.len(), 2);
                        assert!(matches!(
                            targets[0],
                            CallTargetSnapshot::FlowItem(HirItemId(2))
                        ));
                        &targets[1]
                    }
                    _ => panic!("changed capture topology"),
                };
            }
            assert!(matches!(cursor, CallTargetSnapshot::FlowItem(HirItemId(1))));
            drop(saved);
        }
    }
}

#[test]
fn wide_call_target_capture_allocates_one_final_child_table() {
    for width in [1000, 4000, 30_000] {
        let runtime = RuntimeTree(Some(CallTarget::Composed(
            (0..width)
                .map(|i| CallTarget::FlowItem(HirItemId(i as u32)))
                .collect(),
        )));
        let (snapshot, cost) =
            measure(|| capture_call_target(runtime.0.as_ref().unwrap()).unwrap());
        let CallTargetSnapshot::Composed(targets) = snapshot else {
            panic!("composed target")
        };
        assert_eq!(targets.len(), width);
        for (i, target) in targets.iter().enumerate() {
            assert!(
                matches!(target, CallTargetSnapshot::FlowItem(HirItemId(id)) if *id == i as u32)
            );
        }
        eprintln!("wide call target capture width={width}: {cost:?}");
        assert_eq!(
            cost.count, 3,
            "only output table, shared header, frontier: {cost:?}"
        );
        assert!(cost.bytes <= width * size_of::<CallTargetSnapshot>() + 512);
    }
    let empty = capture_call_target(&CallTarget::Composed(vec![])).unwrap();
    assert!(matches!(empty, CallTargetSnapshot::Composed(targets) if targets.is_empty()));
}

fn span() -> etas_core::Span {
    etas_core::Span::empty(etas_core::SourceId(7), etas_core::TextSize::ZERO)
}

#[test]
fn mixed_call_target_capture_preserves_metadata_and_detaches_live_frame() {
    use crate::{
        control::Frame,
        eval::limit::{RuntimeLimit, RuntimeLimitValue},
        value::InterpValue,
    };
    use etas_hir::{HirExprId, SymbolId};
    use etas_types::TypeId;
    let symbol = SymbolId(2);
    let mut frame = Frame::from_snapshot_with_type_bindings(
        vec![(symbol, InterpValue::i32(7))],
        [("T".into(), TypeId(3))].into_iter().collect(),
    )
    .unwrap();
    let mut node = CallTarget::Lambda {
        expr: HirExprId(4),
        captured: frame.clone(),
    };
    let depth = 30_000;
    for i in 0..depth {
        node = if i % 2 == 0 {
            CallTarget::Specialized {
                target: Box::new(node),
                type_bindings: vec![(format!("T{i}"), TypeId(i as u32))],
            }
        } else {
            CallTarget::Limited {
                target: Box::new(node),
                limits: vec![RuntimeLimit {
                    kind: etas_std::StdLimitKind::Attempts,
                    value: RuntimeLimitValue::Count(i as u64),
                    span: span(),
                }],
            }
        };
    }
    let runtime = RuntimeTree(Some(node));
    let snapshot = capture_call_target(runtime.0.as_ref().unwrap()).unwrap();
    assert!(frame.set(symbol, InterpValue::i32(9)));
    let mut cursor = &snapshot;
    for i in (0..depth).rev() {
        cursor = match (i % 2, cursor) {
            (
                0,
                CallTargetSnapshot::Specialized {
                    target,
                    type_bindings,
                },
            ) => {
                assert_eq!(type_bindings, &vec![(format!("T{i}"), TypeId(i as u32))]);
                target
            }
            (1, CallTargetSnapshot::Limited { target, limits }) => {
                assert_eq!(limits.len(), 1);
                assert_eq!(limits[0].kind, etas_std::StdLimitKind::Attempts);
                assert_eq!(limits[0].value, RuntimeLimitValue::Count(i as u64));
                assert_eq!(limits[0].span, span());
                target
            }
            _ => panic!("changed wrapper metadata"),
        };
    }
    let CallTargetSnapshot::Lambda { expr, captured } = cursor else {
        panic!("lambda")
    };
    assert_eq!(*expr, HirExprId(4));
    assert_eq!(captured.id, frame.snapshot_id());
    assert_eq!(captured.type_bindings, vec![("T".into(), TypeId(3))]);
    assert_eq!(captured.locals[0].0, symbol);
    assert_eq!(
        captured.locals[0].1,
        crate::orchestration::ValueSnapshot::capture(&InterpValue::i32(7)).unwrap()
    );
}

#[test]
fn late_call_target_capture_failure_releases_deep_siblings_and_preserves_error_order() {
    use crate::{
        control::Frame,
        value::{HostHandleValue, InterpValue},
    };
    use etas_hir::{HirExprId, SymbolId};
    use etas_types::TypeId;
    let bad = |browser| {
        let handle = if browser {
            HostHandleValue::browser_session(TypeId(1), "live-session".into())
        } else {
            HostHandleValue::tcp_stream(
                TypeId(1),
                etas_host::TcpStreamRef::issued(
                    etas_host::StreamHandleRef::issued("capture-test", 0),
                    etas_host::ByteStreamOrigin::Tcp {
                        host: "example.test".into(),
                        port: 443,
                    },
                ),
            )
        };
        CallTarget::Lambda {
            expr: HirExprId(1),
            captured: Frame::from_snapshot(vec![(SymbolId(1), InterpValue::HostHandle(handle))])
                .unwrap(),
        }
    };
    for mode in 0..4 {
        for browser_first in [false, true] {
            let mut completed = tree(CallTarget::FlowItem(HirItemId(1)), 30_000, mode);
            let runtime = tree(
                CallTarget::Composed(vec![
                    completed.0.take().unwrap(),
                    bad(browser_first),
                    bad(!browser_first),
                ]),
                30_000,
                0,
            );
            for _ in 0..2 {
                let (_, cost) = measure(|| {
                    let error = match capture_call_target(runtime.0.as_ref().unwrap()) {
                        Err(error) => error,
                        Ok(snapshot) => {
                            drop(snapshot);
                            panic!("live handle capture unexpectedly succeeded");
                        }
                    };
                    assert!(
                        error.contains(if browser_first {
                            "browser_session"
                        } else {
                            "tcp_stream"
                        }),
                        "{error}"
                    );
                    assert!(
                        error.contains("cannot be captured in a checkpoint"),
                        "{error}"
                    );
                });
                assert_eq!(
                    cost.bytes, cost.released_bytes,
                    "partial capture leaked: {cost:?}"
                );
            }
        }
    }
}

#[test]
fn machine_capture_reaches_deep_call_targets_without_cloning_runtime_frames() {
    use crate::{
        control::{Continuation, Frame},
        eval::machine::{frame::EvalFrame, state::EvalMachine},
        orchestration::{ContinuationSnapshot, MachineFrameSnapshot},
    };
    let checked = crate::testing::project::checked_project(
        "module app.main; flow main() -> unit { return; }",
    );
    let block_span = checked.hir.blocks.iter().next().unwrap().1.span;
    for depth in [1000, 4000, 30_000] {
        let mut runtime = tree(CallTarget::FlowItem(checked.entry.unwrap()), depth, 2);
        let mut machine = EvalMachine::new();
        machine.push_frame(EvalFrame::from_continuation(Continuation::CallArgs {
            target: runtime.0.take().unwrap(),
            args: vec![].into(),
            next_arg_index: 0,
            evaluated_args: vec![],
            span: block_span,
            frame: Frame::from_snapshot(vec![]).unwrap(),
        }));
        let result = machine.snapshot();
        assert_eq!(machine.frames().len(), 1);
        let Continuation::CallArgs { target, .. } =
            machine.pop_frame().unwrap().into_continuation()
        else {
            panic!("call args frame")
        };
        runtime.0 = Some(target);
        let saved = result.unwrap();
        let MachineFrameSnapshot::Continuation {
            continuation:
                ContinuationSnapshot::CallArgs {
                    target,
                    args,
                    next_arg_index,
                    ..
                },
        } = &saved.frames[0]
        else {
            panic!("captured call args frame")
        };
        assert!(args.is_empty());
        assert_eq!(*next_arg_index, 0);
        let mut cursor = target;
        for _ in 0..depth {
            let CallTargetSnapshot::Composed(targets) = cursor else {
                panic!("composed target")
            };
            cursor = &targets[0];
        }
        assert!(
            matches!(cursor, CallTargetSnapshot::FlowItem(item) if *item == checked.entry.unwrap())
        );
    }
}
