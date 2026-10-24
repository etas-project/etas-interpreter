use super::*;
use crate::testing::allocation::measure;

fn deep_continuation(depth: usize) -> ContinuationSnapshot {
    let mut continuation = ContinuationSnapshot::Return;
    for _ in 0..depth {
        continuation = ContinuationSnapshot::CallBoundary {
            outer: continuation.into(),
        };
    }
    continuation
}

#[test]
fn machine_encoding_budget_is_shared_across_frames_and_releases_completed_frames() {
    let machine = MachineSnapshot {
        frames: vec![
            MachineFrameSnapshot::Continuation {
                continuation: deep_continuation(30_000),
            },
            MachineFrameSnapshot::Continuation {
                continuation: ContinuationSnapshot::Return,
            },
        ],
    };
    let (_, cost) = measure(|| {
        let error = machine_json_with_budget(&machine, &mut snapshot::EncodingBudget::new(30_001))
            .unwrap_err();
        assert_eq!(
            error.message(),
            "checkpoint snapshot expansion exceeds node budget"
        );
    });
    assert_eq!(
        cost.bytes, cost.released_bytes,
        "completed frame leaked: {cost:?}"
    );
    let wire = CheckpointDocument::from_value(machine_json(&machine).unwrap());
    assert_eq!(wire["frames"].as_array().unwrap().len(), 2);
    assert_eq!(wire["frames"][1]["continuation"]["kind"], "return");
    eprintln!("multi-frame expansion budget failure: {cost:?}");
}

#[test]
fn model_pending_and_outer_continuations_share_the_machine_encoding_budget() {
    use crate::orchestration::*;
    let checked = crate::testing::project::checked_project(
        "module app.main; flow main() -> unit { return; }",
    );
    let span = checked.hir.blocks.iter().next().unwrap().1.span;
    let frame = ModelLoopFrameSnapshot {
        pending: PendingModelSnapshot {
            request: ModelRequestSnapshot {
                id: HostRequestId(1),
                provider: None,
                model: ModelName("test".into()),
                messages: vec![],
                tools: vec![],
                tool_choice: etas_host::ModelToolChoice::Auto,
                response_schema: None,
                policy_ref: None,
                options: Default::default(),
                budget_limits: Budget::default(),
            },
            decode: ModelDecodeSnapshot::String,
            max_tool_rounds: 1,
            source_tools: vec![],
            span,
            continuation: deep_continuation(30_000),
        },
        round: 0,
        repair: Default::default(),
        last_tool_error: None,
        remaining_tool_calls: vec![],
        completed_tool_result: false,
        current_host_tool: None,
        boundary_key: "model".into(),
        outer_continuation: ContinuationSnapshot::Return,
    };
    let (_, cost) = measure(|| {
        assert!(model_loop_frame_json(&frame, &mut snapshot::EncodingBudget::new(30_001)).is_err());
    });
    assert_eq!(
        cost.bytes, cost.released_bytes,
        "pending continuation leaked: {cost:?}"
    );
    let wire = CheckpointDocument::from_value(
        model_loop_frame_json(&frame, &mut snapshot::EncodingBudget::default()).unwrap(),
    );
    assert_eq!(wire["outer_continuation"]["kind"], "return");
}
