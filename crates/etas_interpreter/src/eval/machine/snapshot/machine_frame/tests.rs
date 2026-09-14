use etas_core::{SourceId, Span, TextSize};
use etas_hir::HirExprId;

use crate::{
    control::{Continuation, Frame},
    eval::machine::{
        frame::{BlockFrame, CallFrame, EvalFrame, ExprFrame},
        state::EvalMachine,
    },
    orchestration::{ContinuationSnapshot, HandlerScopeId, MachineFrameSnapshot},
    testing::allocation::measure,
    value::InterpValue,
};

fn span() -> Span {
    Span::empty(SourceId(7), TextSize::ZERO)
}

fn record_continuation(width: usize) -> Continuation {
    Continuation::RecordField {
        expr: HirExprId(3),
        nominal_type: None,
        variant_symbol: None,
        next_index: width,
        values: (0..width)
            .map(|i| {
                (
                    format!("field_{i}"),
                    InterpValue::Bytes(vec![7; 1024].into()),
                )
            })
            .collect(),
        frame: Frame::from_snapshot(vec![]).unwrap(),
    }
}

fn record_snapshot(
    snapshot: &ContinuationSnapshot,
) -> &[(String, crate::orchestration::ValueSnapshot)] {
    match snapshot {
        ContinuationSnapshot::RecordField { values, .. } => values,
        ContinuationSnapshot::HandleBoundary { inner, .. } => record_snapshot(inner),
        _ => panic!("expected a record continuation"),
    }
}

#[test]
fn machine_snapshot_borrows_wide_continuations_without_cloning_runtime_frames() {
    for kind in ["block", "expr", "call", "continuation", "handler"] {
        for width in [1000, 2000, 4000] {
            let mut continuation = record_continuation(width);
            if kind == "handler" {
                continuation = Continuation::HandleBoundary {
                    scope_id: HandlerScopeId(11),
                    inner: Box::new(continuation),
                    handlers: vec![],
                    span: span(),
                    frame: Frame::from_snapshot(vec![]).unwrap(),
                };
            }
            let (expected, direct_cost) =
                measure(|| ContinuationSnapshot::capture(&continuation).unwrap());
            let frame = match kind {
                "block" => EvalFrame::Block(BlockFrame { continuation }),
                "expr" => EvalFrame::Expr(ExprFrame { continuation }),
                "call" => EvalFrame::Call(CallFrame {
                    continuation,
                    span: span(),
                }),
                _ => EvalFrame::from_continuation(continuation),
            };
            let mut machine = EvalMachine::new();
            machine.push_frame(frame);
            let (snapshot, cost) = measure(|| machine.snapshot().unwrap());
            let captured = match &snapshot.frames[0] {
                MachineFrameSnapshot::Block { continuation }
                | MachineFrameSnapshot::Expr { continuation }
                | MachineFrameSnapshot::Call { continuation, .. }
                | MachineFrameSnapshot::Continuation { continuation }
                | MachineFrameSnapshot::Handler { continuation } => continuation,
                _ => panic!("unexpected machine frame"),
            };
            assert_eq!(record_snapshot(captured), record_snapshot(&expected));
            assert_eq!(machine.frames().len(), 1);
            assert_eq!(machine.active_call_depth(), u32::from(kind == "call"));
            eprintln!("machine capture {kind} n={width}: {cost:?}, direct={direct_cost:?}");
            assert!(
                cost.count <= direct_cost.count + 1,
                "runtime frame was cloned: {cost:?}"
            );
            assert!(
                cost.bytes <= direct_cost.bytes + size_of::<MachineFrameSnapshot>(),
                "intermediate runtime frame allocated: {cost:?}"
            );
        }
    }
}

#[test]
fn handler_retry_capture_preserves_metadata_and_detaches_shared_live_locals() {
    use super::super::{RestoreContext, frame::restore_frame};
    use crate::orchestration::{ActiveHandlerArmRecord, RetryAttemptId, RetryAttemptRecord};
    use crate::value::ArrayValue;
    use etas_hir::{HirBlockId, ScopeId, SymbolId};

    let symbol = SymbolId(4);
    let original = InterpValue::Array(ArrayValue::new(vec![InterpValue::i32(7)]));
    let mut locals = Frame::from_snapshot(vec![(symbol, original.clone())]).unwrap();
    let arm = ActiveHandlerArmRecord {
        effect_segments: vec!["Test".into()],
        action: "probe".into(),
        action_symbol: Some(SymbolId(8)),
        type_args: vec![],
        effect_type_args: vec![],
        patterns: vec![],
        body: HirBlockId(9),
        scope: ScopeId(2),
        span: span(),
    };
    let retry = RetryAttemptRecord {
        id: RetryAttemptId(21),
        ordinal: 3,
    };
    let mut machine = EvalMachine::new();
    machine.push_frame(EvalFrame::from_continuation(Continuation::HandleBoundary {
        scope_id: HandlerScopeId(11),
        inner: Box::new(Continuation::Return),
        handlers: vec![arm.clone()],
        span: span(),
        frame: locals.clone(),
    }));
    machine.push_frame(EvalFrame::from_continuation(Continuation::RetryAttempt {
        retry: retry.clone(),
        body: HirBlockId(5),
        attempts: 8,
        next_attempt: 4,
        block: HirBlockId(6),
        next_stmt_index: 7,
        frame: locals.clone(),
    }));
    let snapshot = machine.snapshot().unwrap();
    let MachineFrameSnapshot::Handler {
        continuation:
            ContinuationSnapshot::HandleBoundary {
                scope_id,
                inner,
                handlers,
                span: saved_span,
                frame: handler_locals,
            },
    } = &snapshot.frames[0]
    else {
        panic!("handler snapshot")
    };
    assert_eq!(*scope_id, HandlerScopeId(11));
    assert!(matches!(**inner, ContinuationSnapshot::Return));
    assert_eq!(handlers, &[arm]);
    assert_eq!(*saved_span, span());
    let MachineFrameSnapshot::Retry {
        continuation:
            ContinuationSnapshot::RetryAttempt {
                retry: saved_retry,
                body,
                attempts,
                next_attempt,
                block,
                next_stmt_index,
                frame: retry_locals,
            },
    } = &snapshot.frames[1]
    else {
        panic!("retry snapshot")
    };
    assert_eq!(*saved_retry, retry);
    assert_eq!(
        (*body, *attempts, *next_attempt, *block, *next_stmt_index),
        (HirBlockId(5), 8, 4, HirBlockId(6), 7)
    );
    assert_eq!(handler_locals.id, retry_locals.id);
    assert_eq!(handler_locals, retry_locals);
    locals
        .with_local_mut(symbol, |value| {
            let InterpValue::Array(values) = value else {
                panic!("array")
            };
            values.borrow_mut().push(InterpValue::i32(8));
        })
        .unwrap();
    let mut context = RestoreContext::default();
    let mut handler = restore_frame(handler_locals.clone(), &mut context).unwrap();
    let restored_retry = restore_frame(retry_locals.clone(), &mut context).unwrap();
    assert_eq!(handler.get(symbol), Some(original.clone()));
    assert_eq!(restored_retry.get(symbol), Some(original));
    assert_ne!(handler.get(symbol), locals.get(symbol));
    assert!(handler.set(symbol, InterpValue::i32(99)));
    assert_eq!(restored_retry.get(symbol), Some(InterpValue::i32(99)));
    assert_ne!(locals.get(symbol), Some(InterpValue::i32(99)));
}

#[test]
fn borrowed_machine_capture_rejects_live_handles_without_consuming_runtime_stack() {
    let mut continuation = record_continuation(1);
    let Continuation::RecordField { values, .. } = &mut continuation else {
        unreachable!()
    };
    values[0].1 = InterpValue::HostHandle(crate::value::HostHandleValue::browser_session(
        etas_types::TypeId(1),
        "test-session".into(),
    ));
    let mut machine = EvalMachine::new();
    machine.push_frame(EvalFrame::Call(CallFrame {
        continuation,
        span: span(),
    }));
    for _ in 0..2 {
        assert!(
            machine
                .snapshot()
                .unwrap_err()
                .contains("host handles cannot be captured")
        );
        assert_eq!(machine.frames().len(), 1);
        assert_eq!(machine.active_call_depth(), 1);
    }
    let EvalFrame::Call(frame) = machine.pop_frame().unwrap() else {
        panic!("call frame")
    };
    let Continuation::RecordField { values, .. } = frame.continuation else {
        panic!("record")
    };
    assert!(matches!(values[0].1, InterpValue::HostHandle(_)));
}

#[test]
fn machine_capture_allocates_exactly_one_output_table_for_known_stack_length() {
    for depth in [0, 1, 1000, 2000, 4000] {
        let mut machine = EvalMachine::new();
        for i in 0..depth {
            let continuation = match i % 3 {
                0 => Continuation::Return,
                1 => Continuation::Resume,
                _ => Continuation::Finish,
            };
            machine.push_frame(EvalFrame::from_continuation(continuation));
        }
        let (snapshot, cost) = measure(|| machine.snapshot().unwrap());
        eprintln!("machine frame table n={depth}: {cost:?}");
        assert_eq!(cost.count, usize::from(depth != 0));
        assert_eq!(cost.bytes, depth * size_of::<MachineFrameSnapshot>());
        assert_eq!(snapshot.frames.len(), depth);
        for (i, frame) in snapshot.frames.iter().enumerate() {
            let MachineFrameSnapshot::Continuation { continuation } = frame else {
                panic!("continuation frame")
            };
            assert!(matches!(
                (i % 3, continuation),
                (0, ContinuationSnapshot::Return)
                    | (1, ContinuationSnapshot::Resume)
                    | (2, ContinuationSnapshot::Finish)
            ));
        }
    }
}
