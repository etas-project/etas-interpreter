use super::*;
use crate::testing::allocation::measure;

fn deep_continuation(depth: usize) -> ContinuationSnapshot {
    let mut continuation = ContinuationSnapshot::Return;
    for _ in 0..depth {
        continuation = ContinuationSnapshot::CallBoundary {
            caller: None,
            span: crate::testing::synthetic_span(),
            outer: continuation.into(),
        };
    }
    continuation
}

#[test]
fn machine_encoding_budget_is_shared_across_frames_and_releases_completed_frames() {
    let machine = MachineSnapshot {
        auxiliary: Vec::new(),
        input: None,
        program_owners: Default::default(),
        suspended_programs: Vec::new(),
        frames: vec![
            MachineFrameSnapshot::Continuation {
                continuation: deep_continuation(30_000),
            },
            MachineFrameSnapshot::Continuation {
                continuation: ContinuationSnapshot::Return,
            },
        ],
        residual: Default::default(),
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
    let frame = ModelRequestFrameSnapshot {
        boundary_request_id: etas_host::HostRequestId(0),
        pending: PendingModelSnapshot {
            request: ModelRequestSnapshot {
                action: etas_host::ModelAction::Infer,
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
            span,
            continuation: deep_continuation(30_000),
        },
        boundary_key: etas_host::replay::ReplayFingerprint::from_bytes([1; 32]),
        outer_continuation: ContinuationSnapshot::Return,
    };
    let (_, cost) = measure(|| {
        assert!(
            model_request_frame_json(&frame, &mut snapshot::EncodingBudget::new(30_001)).is_err()
        );
    });
    assert_eq!(
        cost.bytes, cost.released_bytes,
        "pending continuation leaked: {cost:?}"
    );
    let wire = CheckpointDocument::from_value(
        model_request_frame_json(&frame, &mut snapshot::EncodingBudget::default()).unwrap(),
    );
    assert_eq!(wire["outer_continuation"]["kind"], "return");
}
