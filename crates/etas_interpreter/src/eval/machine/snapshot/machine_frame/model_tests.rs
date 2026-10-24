use etas_core::{SourceId, Span, TextSize};
use etas_hir::HirItemId;
use etas_host::{
    HostRequestId, HostValue, ModelContent, ModelMessage, ModelName, ModelRequest, ModelRole,
    ModelToolCall,
};

use crate::{
    api::HostExecutionContext,
    control::{Continuation, ModelDecode, PendingModel, SourceToolBinding},
    eval::machine::{
        frame::{EvalFrame, ModelLoopFrame, ModelRepairState, SourceToolReturnFrame},
        state::EvalMachine,
    },
    orchestration::{MachineFrameSnapshot, ModelLoopFrameSnapshot, SourceToolReturnFrameSnapshot},
    testing::allocation::measure,
};

fn model_frame(width: usize) -> ModelLoopFrame {
    let host = HostExecutionContext::default();
    ModelLoopFrame {
        pending: PendingModel {
            request: ModelRequest {
                id: HostRequestId(7),
                provider: None,
                model: ModelName("test-model".into()),
                messages: (0..width)
                    .map(|_| ModelMessage {
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
            max_tool_rounds: 9,
            source_tools: vec![],
            span: Span::empty(SourceId(7), TextSize::ZERO),
            continuation: Continuation::Return,
        },
        round: 3,
        repair: ModelRepairState {
            attempts: 2,
            last_kind: Some("schema".into()),
        },
        last_tool_error: Some("retryable".into()),
        remaining_tool_calls: vec![ModelToolCall {
            id: "next".into(),
            tool: "helper".into(),
            args: HostValue::String("payload".into()),
        }],
        completed_tool_result: true,
        current_host_tool: None,
        boundary_key: "boundary".into(),
        outer_continuation: Continuation::Finish,
    }
}

fn assert_model(actual: &ModelLoopFrameSnapshot, expected: &ModelLoopFrameSnapshot) {
    assert_eq!(
        actual.pending.request.messages,
        expected.pending.request.messages
    );
    assert_eq!(actual.pending.request.id, expected.pending.request.id);
    assert_eq!(actual.pending.request.model, expected.pending.request.model);
    assert_eq!(actual.round, expected.round);
    assert_eq!(actual.repair.attempts, expected.repair.attempts);
    assert_eq!(actual.repair.last_kind, expected.repair.last_kind);
    assert_eq!(actual.last_tool_error, expected.last_tool_error);
    assert_eq!(actual.remaining_tool_calls, expected.remaining_tool_calls);
    assert_eq!(actual.completed_tool_result, expected.completed_tool_result);
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
fn model_and_source_tool_capture_share_pending_outer_and_machine_locals() {
    use crate::{
        control::Frame, eval::machine::frame::ExprFrame, orchestration::ContinuationSnapshot,
        value::InterpValue,
    };
    use etas_hir::{HirBlockId, SymbolId};
    for source_tool in [false, true] {
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
        machine.push_frame(if source_tool {
            EvalFrame::SourceToolReturn(SourceToolReturnFrame {
                tool_call_id: "call-1".into(),
                tool_name: "helper".into(),
                binding: SourceToolBinding {
                    name: "helper".into(),
                    qualified_name: None,
                    item: HirItemId(3),
                },
                args: HostValue::String("input".into()),
                boundary_key: "tool".into(),
                output_schema: None,
                model_loop: Box::new(model),
            })
        } else {
            EvalFrame::ModelLoop(Box::new(model))
        });
        let saved = machine.snapshot().unwrap();
        let MachineFrameSnapshot::Expr {
            continuation: ContinuationSnapshot::ContinueBlock { frame: first, .. },
        } = &saved.frames[0]
        else {
            panic!("expr")
        };
        let model = match &saved.frames[1] {
            MachineFrameSnapshot::ModelLoop(model) => &**model,
            MachineFrameSnapshot::SourceToolReturn(tool) => &*tool.model_loop,
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
            ModelLoopFrameSnapshot::capture(&model, &mut super::super::CaptureContext::default())
                .unwrap()
        });
        let mut machine = EvalMachine::new();
        machine.push_frame(EvalFrame::ModelLoop(Box::new(model)));
        let (snapshot, cost) = measure(|| machine.snapshot().unwrap());
        let MachineFrameSnapshot::ModelLoop(actual) = &snapshot.frames[0] else {
            panic!("model frame")
        };
        assert_model(actual, &expected);
        eprintln!("machine model n={width}: {cost:?}, direct={direct:?}");
        assert!(cost.count <= direct.count + 2);
        assert!(
            cost.bytes
                <= direct.bytes
                    + size_of::<ModelLoopFrameSnapshot>()
                    + size_of::<MachineFrameSnapshot>()
        );
        let EvalFrame::ModelLoop(mut runtime) = machine.pop_frame().unwrap() else {
            panic!("model frame")
        };
        runtime.pending.request.messages[0].content = vec![ModelContent::Text("changed".into())];
        runtime.remaining_tool_calls.clear();
        assert_model(actual, &expected);
    }
}

#[test]
fn machine_snapshot_copies_source_tool_arguments_only_into_durable_output() {
    for width in [1000, 2000, 4000] {
        let tool = SourceToolReturnFrame {
            tool_call_id: "call-1".into(),
            tool_name: "helper".into(),
            binding: SourceToolBinding {
                name: "helper".into(),
                qualified_name: Some("test.helper".into()),
                item: HirItemId(3),
            },
            args: HostValue::List(
                (0..width)
                    .map(|_| HostValue::String("x".repeat(1024)))
                    .collect(),
            ),
            boundary_key: "tool-boundary".into(),
            output_schema: None,
            model_loop: Box::new(model_frame(1)),
        };
        let (expected, direct) = measure(|| {
            SourceToolReturnFrameSnapshot::capture(
                &tool,
                &mut super::super::CaptureContext::default(),
            )
            .unwrap()
        });
        let mut machine = EvalMachine::new();
        machine.push_frame(EvalFrame::SourceToolReturn(tool));
        let (snapshot, cost) = measure(|| machine.snapshot().unwrap());
        let MachineFrameSnapshot::SourceToolReturn(actual) = &snapshot.frames[0] else {
            panic!("tool frame")
        };
        assert_eq!(actual.args, expected.args);
        assert_eq!(actual.tool_call_id, expected.tool_call_id);
        assert_eq!(
            actual.binding.qualified_name,
            expected.binding.qualified_name
        );
        assert_eq!(actual.boundary_key, expected.boundary_key);
        assert_model(&actual.model_loop, &expected.model_loop);
        eprintln!("machine source tool n={width}: {cost:?}, direct={direct:?}");
        assert!(cost.count <= direct.count + 1);
        assert!(cost.bytes <= direct.bytes + size_of::<MachineFrameSnapshot>());
        let EvalFrame::SourceToolReturn(mut runtime) = machine.pop_frame().unwrap() else {
            panic!("tool frame")
        };
        runtime.args = HostValue::String("changed".into());
        assert_eq!(actual.args, expected.args);
    }
}
