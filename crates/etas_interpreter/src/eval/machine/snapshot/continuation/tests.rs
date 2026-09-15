use crate::{
    control::Continuation, orchestration::ContinuationSnapshot, testing::allocation::measure,
};

// Runtime edge destruction is a separate audit. Isolate the capture algorithm.
struct RuntimeTree(Option<Continuation>);

impl Drop for RuntimeTree {
    fn drop(&mut self) {
        let mut pending = Vec::new();
        if let Some(root) = self.0.take() {
            pending.push(root);
        }
        while let Some(node) = pending.pop() {
            match node {
                Continuation::CallBoundary { outer } | Continuation::HandlerDispatch { outer } => {
                    pending.push(*outer)
                }
                Continuation::RestoreModelPolicy { inner, .. }
                | Continuation::ScopedModelPolicy { inner, .. }
                | Continuation::HandleBoundary { inner, .. } => pending.push(*inner),
                Continuation::Chain { inner, outer } => {
                    pending.push(*outer);
                    pending.push(*inner);
                }
                _ => {}
            }
        }
    }
}

#[test]
fn deep_continuation_capture_uses_explicit_traversal_without_stack_overflow() {
    const WORKER: &str = "ETAS_TEST_CONTINUATION_CAPTURE_WORKER";
    if std::env::var_os(WORKER).is_none() {
        let current = std::thread::current();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", current.name().unwrap(), "--nocapture"])
            .env(WORKER, "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "capture subprocess failed: {}\n{}\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
        eprint!("{}", String::from_utf8_lossy(&output.stderr));
        return;
    }
    for depth in [1000, 4000, 30_000] {
        for mode in 0..3 {
            let mut node = Continuation::Return;
            for _ in 0..depth {
                node = match mode {
                    0 => Continuation::CallBoundary {
                        outer: Box::new(node),
                    },
                    1 => Continuation::Chain {
                        inner: Box::new(node),
                        outer: Box::new(Continuation::Finish),
                    },
                    _ => Continuation::Chain {
                        inner: Box::new(Continuation::Resume),
                        outer: Box::new(node),
                    },
                };
            }
            let runtime = RuntimeTree(Some(node));
            let (snapshot, cost) =
                measure(|| ContinuationSnapshot::capture(runtime.0.as_ref().unwrap()).unwrap());
            eprintln!("continuation capture depth={depth} mode={mode}: {cost:?}");
            let edges = depth * if mode == 0 { 1 } else { 2 };
            let edge_bytes = size_of::<ContinuationSnapshot>() + 2 * size_of::<usize>();
            assert!(cost.count <= edges + 32, "extra snapshot tree: {cost:?}");
            assert!(
                cost.bytes <= edges * edge_bytes + depth * 64 + 4096,
                "copied tree instead of bounded DFS frontier: {cost:?}"
            );
            let mut cursor = &snapshot;
            for _ in 0..depth {
                cursor = match (mode, cursor) {
                    (0, ContinuationSnapshot::CallBoundary { outer }) => outer,
                    (1, ContinuationSnapshot::Chain { inner, outer }) => {
                        assert!(matches!(**outer, ContinuationSnapshot::Finish));
                        inner
                    }
                    (2, ContinuationSnapshot::Chain { inner, outer }) => {
                        assert!(matches!(**inner, ContinuationSnapshot::Resume));
                        outer
                    }
                    _ => panic!("wrong captured continuation topology"),
                };
            }
            assert!(matches!(cursor, ContinuationSnapshot::Return));
            drop(snapshot);
            drop(runtime);
        }
    }
}

fn span() -> etas_core::Span {
    etas_core::Span::empty(etas_core::SourceId(7), etas_core::TextSize::ZERO)
}

#[test]
fn machine_capture_preserves_its_deep_runtime_continuation_stack() {
    use crate::{
        eval::machine::{frame::EvalFrame, state::EvalMachine},
        orchestration::MachineFrameSnapshot,
    };
    for depth in [1000, 4000, 30_000] {
        let mut root = Continuation::Return;
        for _ in 0..depth {
            root = Continuation::CallBoundary {
                outer: Box::new(root),
            };
        }
        let mut runtime = RuntimeTree(None);
        let mut machine = EvalMachine::new();
        machine.push_frame(EvalFrame::from_continuation(root));
        let result = machine.snapshot();
        let frame_count = machine.frames().len();
        runtime.0 = Some(machine.pop_frame().unwrap().into_continuation());
        assert_eq!(frame_count, 1);
        let snapshot = result.unwrap();
        let MachineFrameSnapshot::Continuation { continuation } = &snapshot.frames[0] else {
            panic!("wrong machine frame kind")
        };
        let mut cursor = continuation;
        for _ in 0..depth {
            let ContinuationSnapshot::CallBoundary { outer } = cursor else {
                panic!("missing captured edge")
            };
            cursor = outer;
        }
        assert!(matches!(cursor, ContinuationSnapshot::Return));
    }
}

#[test]
fn mixed_capture_preserves_wrapper_metadata_and_detaches_live_frames() {
    use crate::{api::ModelExecutionPolicy, control::Frame, orchestration::HandlerScopeId};
    use etas_hir::SymbolId;
    let symbol = SymbolId(2);
    let mut frame =
        Frame::from_snapshot(vec![(symbol, crate::value::InterpValue::i32(7))]).unwrap();
    let mut node = Continuation::Resume;
    let depth = 30_000;
    for i in 0..depth {
        node = match i % 5 {
            0 => Continuation::CallBoundary {
                outer: Box::new(node),
            },
            1 => Continuation::HandlerDispatch {
                outer: Box::new(node),
            },
            2 => Continuation::HandleBoundary {
                scope_id: HandlerScopeId(i as u32),
                inner: Box::new(node),
                handlers: vec![],
                span: span(),
                frame: frame.clone(),
            },
            3 => Continuation::RestoreModelPolicy {
                previous: Box::new(ModelExecutionPolicy {
                    max_tool_rounds: i,
                    ..Default::default()
                }),
                inner: Box::new(node),
            },
            _ => Continuation::ScopedModelPolicy {
                policy: Box::new(ModelExecutionPolicy {
                    max_tool_rounds: i,
                    ..Default::default()
                }),
                inner: Box::new(node),
            },
        };
    }
    let runtime = RuntimeTree(Some(node));
    let snapshot = ContinuationSnapshot::capture(runtime.0.as_ref().unwrap()).unwrap();
    assert!(frame.set(symbol, crate::value::InterpValue::i32(9)));
    let mut cursor = &snapshot;
    for i in (0..depth).rev() {
        cursor = match (i % 5, cursor) {
            (0, ContinuationSnapshot::CallBoundary { outer })
            | (1, ContinuationSnapshot::HandlerDispatch { outer }) => outer,
            (
                2,
                ContinuationSnapshot::HandleBoundary {
                    scope_id,
                    inner,
                    handlers,
                    span: actual,
                    frame: saved,
                },
            ) => {
                assert_eq!(*scope_id, HandlerScopeId(i as u32));
                assert_eq!(*actual, span());
                assert!(handlers.is_empty());
                assert_eq!(saved.id, frame.snapshot_id());
                assert_eq!(saved.locals[0].0, symbol);
                assert_eq!(
                    saved.locals[0].1,
                    crate::orchestration::ValueSnapshot::capture(&crate::value::InterpValue::i32(
                        7
                    ))
                    .unwrap()
                );
                inner
            }
            (3, ContinuationSnapshot::RestoreModelPolicy { previous, inner }) => {
                assert_eq!(previous.max_tool_rounds, i);
                inner
            }
            (4, ContinuationSnapshot::ScopedModelPolicy { policy, inner }) => {
                assert_eq!(policy.max_tool_rounds, i);
                inner
            }
            _ => panic!("changed captured wrapper"),
        };
    }
    assert!(matches!(cursor, ContinuationSnapshot::Resume));
}

#[test]
fn capture_rejects_late_live_handles_and_releases_completed_deep_subtrees() {
    use crate::{
        control::Frame,
        orchestration::HandlerScopeId,
        value::{HostHandleValue, InterpValue},
    };
    use etas_hir::SymbolId;
    for in_parent in [false, true] {
        let mut inner = Continuation::Return;
        for _ in 0..30_000 {
            inner = Continuation::CallBoundary {
                outer: Box::new(inner),
            };
        }
        let handle = InterpValue::HostHandle(HostHandleValue::browser_session(
            etas_types::TypeId(1),
            "live-session".into(),
        ));
        let root = if in_parent {
            Continuation::HandleBoundary {
                scope_id: HandlerScopeId(1),
                inner: Box::new(inner),
                handlers: vec![],
                span: span(),
                frame: Frame::from_snapshot(vec![(SymbolId(1), handle)]).unwrap(),
            }
        } else {
            Continuation::Chain {
                inner: Box::new(inner),
                outer: Box::new(Continuation::PipelineTarget {
                    input: handle,
                    span: span(),
                }),
            }
        };
        let runtime = RuntimeTree(Some(root));
        for _ in 0..2 {
            let error = ContinuationSnapshot::capture(runtime.0.as_ref().unwrap()).unwrap_err();
            assert!(error.contains("host handles cannot be captured"), "{error}");
        }
    }
}
