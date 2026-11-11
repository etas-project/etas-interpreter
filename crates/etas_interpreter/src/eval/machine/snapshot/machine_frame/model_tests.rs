use etas_core::{SourceId, Span, TextSize};
use etas_host::{HostRequestId, ModelContent, ModelMessage, ModelName, ModelRequest, ModelRole};

use crate::{
    api::HostExecutionContext,
    control::{Continuation, ModelDecode, PendingModel},
    eval::machine::{
        frame::{EvalFrame, ModelRequestFrame},
        state::EvalMachine,
    },
    orchestration::{MachineFrameSnapshot, ModelRequestFrameSnapshot},
    testing::allocation::measure,
};

fn model_frame(width: usize) -> ModelRequestFrame {
    let host = HostExecutionContext::default();
    ModelRequestFrame {
        boundary_request_id: etas_host::HostRequestId(0),
        pending: PendingModel {
            request: ModelRequest {
                action: etas_host::ModelAction::Infer,
                id: HostRequestId(7),
                provider: None,
                model: ModelName("test-model".into()),
                messages: (0..width)
                    .map(|_| ModelMessage {
                        provider_data: Vec::new(),
                        role: ModelRole::User,
                        content: vec![ModelContent::Text("x".repeat(1024))],
                        tool_call_id: None,
                        tool_calls: vec![],
                    })
                    .collect(),
                tools: vec![],
                tool_choice: Default::default(),
                response_schema: None,
                policy_ref: None,
                options: Default::default(),
                authority: host.authority,
                trace: host.trace,
                budget: host.budget,
            },
            decode: ModelDecode::String,
            span: Span::empty(SourceId(7), TextSize::ZERO),
            continuation: Continuation::Return,
        },
        boundary_key: etas_host::replay::ReplayFingerprint::from_bytes([1; 32]),
        outer_continuation: Continuation::Finish,
    }
}

fn assert_model(actual: &ModelRequestFrameSnapshot, expected: &ModelRequestFrameSnapshot) {
    assert_eq!(
        actual.pending.request.messages,
        expected.pending.request.messages
    );
    assert_eq!(actual.pending.request.id, expected.pending.request.id);
    assert_eq!(actual.pending.request.model, expected.pending.request.model);
    assert_eq!(actual.boundary_key, expected.boundary_key);
    assert!(matches!(
        actual.pending.continuation,
        crate::orchestration::ContinuationSnapshot::Return
    ));
    assert!(matches!(
        actual.outer_continuation,
        crate::orchestration::ContinuationSnapshot::Finish
    ));
}

#[test]
fn model_request_capture_shares_pending_outer_and_machine_locals() {
    use crate::{
        control::Frame, eval::machine::frame::ExprFrame, orchestration::ContinuationSnapshot,
        value::InterpValue,
    };
    use etas_hir::{HirBlockId, SymbolId};
    {
        let mut locals = Frame::from_snapshot(vec![(SymbolId(0), InterpValue::i32(7))]).unwrap();
        let continuation = || Continuation::ContinueBlock {
            block: HirBlockId(1),
            next_stmt_index: 2,
            frame: locals.clone(),
        };
        let mut model = model_frame(1);
        model.pending.continuation = continuation();
        model.outer_continuation = continuation();
        let mut machine = EvalMachine::new();
        machine.push_frame(EvalFrame::Expr(ExprFrame {
            continuation: continuation(),
        }));
        machine.push_frame(EvalFrame::ModelRequest(Box::new(model)));
        let saved = machine.snapshot().unwrap();
        let MachineFrameSnapshot::Expr {
            continuation: ContinuationSnapshot::ContinueBlock { frame: first, .. },
        } = &saved.frames[0]
        else {
            panic!("expr")
        };
        let model = match &saved.frames[1] {
            MachineFrameSnapshot::ModelRequest(model) => &**model,
            _ => panic!("model"),
        };
        for continuation in [&model.pending.continuation, &model.outer_continuation] {
            let ContinuationSnapshot::ContinueBlock { frame, .. } = continuation else {
                panic!("block")
            };
            assert_eq!(frame.id, first.id);
            assert!(std::rc::Rc::ptr_eq(&frame.locals, &first.locals));
        }
        assert!(locals.set(SymbolId(0), InterpValue::i32(99)));
        assert!(first.locals[0].1.clone().restore().unwrap() == InterpValue::i32(7));
    }
}

#[test]
fn machine_snapshot_copies_model_payload_only_into_durable_output() {
    for width in [1000, 2000, 4000] {
        let model = model_frame(width);
        let (expected, direct) = measure(|| {
            ModelRequestFrameSnapshot::capture(&model, &mut super::super::CaptureContext::default())
                .unwrap()
        });
        let mut machine = EvalMachine::new();
        machine.push_frame(EvalFrame::ModelRequest(Box::new(model)));
        let (snapshot, cost) = measure(|| machine.snapshot().unwrap());
        let MachineFrameSnapshot::ModelRequest(actual) = &snapshot.frames[0] else {
            panic!("model frame")
        };
        assert_model(actual, &expected);
        eprintln!("machine model n={width}: {cost:?}, direct={direct:?}");
        assert!(cost.count <= direct.count + 2);
        assert!(
            cost.bytes
                <= direct.bytes
                    + size_of::<ModelRequestFrameSnapshot>()
                    + size_of::<MachineFrameSnapshot>()
        );
        let EvalFrame::ModelRequest(mut runtime) = machine.pop_frame().unwrap() else {
            panic!("model frame")
        };
        runtime.pending.request.messages[0].content = vec![ModelContent::Text("changed".into())];
        assert_model(actual, &expected);
    }
}
