use super::*;
use crate::{
    api::RunOptions,
    control::Continuation,
    eval::{EvalContextInput, machine::frame::EvalFrame},
    testing::allocation::measure,
    value::InterpValue,
};

#[test]
fn retry_lookup_does_not_copy_unrelated_payloads_or_each_scanned_frame() {
    let checked = crate::testing::project::checked_project(
        "module app.main; flow main() -> unit { return; }",
    );
    let plan = crate::Interpreter
        .plan(&checked, crate::api::PlanOptions)
        .plan
        .unwrap();
    let options = RunOptions::default();
    let span = crate::diagnostics::item_span(&checked, checked.entry.unwrap());
    let mut eval = EvalContext::new(EvalContextInput {
        storage_limits: Default::default(),
        event_observer: None,
        execution: etas_host::execution::ExecutionScope::new_owned(),
        checked: &checked,
        plan: &plan,
        host_context: options.host_context,
        model_policy: options.model_policy,
        execution_limits: options.execution_limits,
        consumed_steps: 0,
        current_session: None,
        entry_item: checked.entry.unwrap(),
        entry_args: &[],
    });
    for count in [1000, 2000, 4000] {
        for wide_payload in [true, false] {
            let mut machine = EvalMachine::new();
            if wide_payload {
                machine.push_frame(EvalFrame::from_continuation(
                    Continuation::MemoryClearDeleteNext {
                        region_stable_id: "region".into(),
                        path: vec!["entry".into()],
                        remaining_keys: (0..count)
                            .map(|_| InterpValue::String("x".repeat(1024).into()))
                            .collect(),
                        next_index: 0,
                        span,
                    },
                ));
            } else {
                for _ in 0..count {
                    machine.push_frame(EvalFrame::from_continuation(Continuation::Return));
                }
            }
            let message = "provider failed".to_owned();
            let (result, cost) = measure(|| {
                machine.retry_boundary_failure(&mut eval, Continuation::BlockValue, span, message)
            });
            assert!(result.is_none());
            assert_eq!(machine.frames().len(), if wide_payload { 1 } else { count });
            eprintln!("retry scan payload={wide_payload} n={count}: {cost:?}");
            assert!(
                cost.bytes < 4096 && cost.count < 8,
                "retry lookup copied unrelated state: {cost:?}"
            );
        }
    }
}

fn retry(id: u32) -> Continuation {
    Continuation::RetryAttempt {
        retry: crate::orchestration::RetryAttemptRecord {
            id: crate::orchestration::RetryAttemptId(id),
            ordinal: 2,
        },
        body: etas_hir::HirBlockId(3),
        attempts: 5,
        next_attempt: 3,
        block: etas_hir::HirBlockId(4),
        next_stmt_index: 7,
        frame: crate::control::Frame::from_snapshot(vec![]).unwrap(),
    }
}

fn chain(inner: Continuation, outer: Continuation) -> Continuation {
    Continuation::Chain {
        inner: Box::new(inner),
        outer: Box::new(outer),
    }
}

fn selected_id(frame: &EvalFrame) -> Option<u32> {
    frame.retry_continuation().map(|selected| {
        let Continuation::RetryAttempt {
            retry,
            body,
            attempts,
            next_attempt,
            block,
            next_stmt_index,
            ..
        } = selected
        else {
            panic!("only retry attempts can be selected")
        };
        assert_eq!(
            (body.0, attempts, next_attempt, block.0, next_stmt_index),
            (3, 5, 3, 4, 7)
        );
        assert_eq!(retry.ordinal, 2);
        retry.id.0
    })
}

#[test]
fn retry_lookup_preserves_chain_order_and_boundary_opacity() {
    for (continuation, expected) in [
        (retry(1), Some(1)),
        (chain(retry(1), retry(2)), Some(1)),
        (
            chain(chain(Continuation::Return, retry(2)), retry(3)),
            Some(2),
        ),
        (
            chain(
                Continuation::CallBoundary {
                    outer: Box::new(retry(1)),
                },
                retry(2),
            ),
            Some(2),
        ),
        (
            chain(
                Continuation::HandlerDispatch {
                    outer: Box::new(retry(1)),
                },
                retry(2),
            ),
            Some(2),
        ),
        (
            Continuation::RestoreModelPolicy {
                previous: Box::default(),
                inner: Box::new(retry(1)),
            },
            None,
        ),
        (
            Continuation::ScopedModelPolicy {
                policy: Box::default(),
                inner: Box::new(retry(1)),
            },
            None,
        ),
        (
            Continuation::HandleBoundary {
                scope_id: crate::orchestration::HandlerScopeId(7),
                inner: Box::new(retry(1)),
                handlers: vec![],
                span: Span::empty(etas_core::SourceId(7), etas_core::TextSize::ZERO),
                frame: crate::control::Frame::from_snapshot(vec![]).unwrap(),
            },
            None,
        ),
    ] {
        let frame = EvalFrame::from_continuation(continuation);
        assert_eq!(selected_id(&frame), expected);
        assert_eq!(
            selected_id(&frame),
            expected,
            "lookup must not consume the frame"
        );
    }
}

#[test]
fn retry_lookup_walks_deep_chains_without_cloning_or_recursing() {
    for depth in [1000, 4000, 30_000] {
        for left_nested in [false, true] {
            let mut continuation = retry(42);
            for _ in 0..depth {
                continuation = if left_nested {
                    chain(continuation, Continuation::Return)
                } else {
                    chain(Continuation::Return, continuation)
                };
            }
            let frame = EvalFrame::from_continuation(continuation);
            let (id, cost) = measure(|| selected_id(&frame));
            assert_eq!(id, Some(42));
            eprintln!("retry chain depth={depth} left_nested={left_nested}: {cost:?}");
            let frontier_budget = if left_nested {
                depth * size_of::<&Continuation>() * 4
            } else {
                0
            };
            assert!(
                cost.bytes < 4096 + frontier_budget && cost.count < 32,
                "copied deep continuation: {cost:?}"
            );
            // Isolate lookup: recursive Continuation drop is a separate outstanding audit.
            let mut pending = vec![frame.into_continuation()];
            while let Some(continuation) = pending.pop() {
                if let Continuation::Chain { inner, outer } = continuation {
                    pending.push(*outer);
                    pending.push(*inner);
                }
            }
        }
    }
}

#[test]
fn retry_lookup_preserves_selected_locals_in_each_execution_frame_kind() {
    use crate::eval::machine::frame::{BlockFrame, CallFrame, ExprFrame};
    use etas_hir::SymbolId;
    for kind in 0..4 {
        let symbol = SymbolId(5);
        let mut locals =
            crate::control::Frame::from_snapshot(vec![(symbol, InterpValue::i32(7))]).unwrap();
        let mut attempt = retry(8);
        let Continuation::RetryAttempt { frame, .. } = &mut attempt else {
            unreachable!()
        };
        *frame = locals.clone();
        let continuation = chain(Continuation::Return, attempt);
        let frame = match kind {
            0 => EvalFrame::Block(BlockFrame { continuation }),
            1 => EvalFrame::Call(CallFrame {
                continuation,
                span: Span::empty(etas_core::SourceId(7), etas_core::TextSize::ZERO),
            }),
            2 => EvalFrame::Expr(ExprFrame { continuation }),
            _ => EvalFrame::from_continuation(continuation),
        };
        let Continuation::RetryAttempt {
            frame: selected, ..
        } = frame.retry_continuation().unwrap()
        else {
            unreachable!()
        };
        assert_eq!(selected.get(symbol), Some(InterpValue::i32(7)));
        assert_eq!(selected.snapshot_id(), locals.snapshot_id());
        assert!(locals.set(symbol, InterpValue::i32(9)));
        assert_eq!(selected.get(symbol), Some(InterpValue::i32(9)));
        assert_eq!(selected_id(&frame), Some(8));
    }
}

#[test]
fn retry_lookup_only_inspects_model_and_source_tool_outer_continuations() {
    use crate::control::{ModelDecode, PendingModel, SourceToolBinding};
    use crate::eval::machine::frame::{ModelLoopFrame, SourceToolReturnFrame};
    use etas_host::{HostRequestId, HostValue, ModelName, ModelRequest};

    for count in [1000, 2000, 4000] {
        let host = crate::api::HostExecutionContext::default();
        let model = ModelLoopFrame {
            pending: PendingModel {
                request: ModelRequest {
                    id: HostRequestId(1),
                    provider: None,
                    model: ModelName("test-model".into()),
                    messages: vec![],
                    tools: vec![],
                    tool_choice: Default::default(),
                    response_schema: None,
                    policy_ref: Some(HostValue::String("x".repeat(count * 1024))),
                    options: Default::default(),
                    authority: host.authority,
                    trace: host.trace,
                    budget: host.budget,
                },
                decode: ModelDecode::String,
                max_tool_rounds: 5,
                source_tools: vec![],
                span: Span::empty(etas_core::SourceId(7), etas_core::TextSize::ZERO),
                continuation: retry(99),
            },
            round: 1,
            repair: Default::default(),
            last_tool_error: None,
            remaining_tool_calls: vec![],
            completed_tool_result: false,
            current_host_tool: None,
            boundary_key: "boundary".into(),
            outer_continuation: Continuation::Return,
        };
        let mut frame = EvalFrame::ModelLoop(Box::new(model));
        let (id, cost) = measure(|| selected_id(&frame));
        assert_eq!(
            id, None,
            "pending model continuation is not the outer continuation"
        );
        assert_eq!(cost.count, 0, "copied unrelated model payload: {cost:?}");
        let EvalFrame::ModelLoop(model) = &mut frame else {
            unreachable!()
        };
        model.outer_continuation = retry(7);
        let (id, cost) = measure(|| selected_id(&frame));
        assert_eq!(id, Some(7));
        assert_eq!(cost.count, 0);
        let EvalFrame::ModelLoop(model) = frame else {
            unreachable!()
        };
        let frame = EvalFrame::SourceToolReturn(SourceToolReturnFrame {
            tool_call_id: "call".into(),
            tool_name: "tool".into(),
            binding: SourceToolBinding {
                name: "tool".into(),
                qualified_name: None,
                item: etas_hir::HirItemId(1),
            },
            args: HostValue::String("y".repeat(count * 1024)),
            boundary_key: "tool-boundary".into(),
            output_schema: None,
            model_loop: model,
        });
        let (id, cost) = measure(|| selected_id(&frame));
        assert_eq!(id, Some(7));
        assert_eq!(cost.count, 0, "copied unrelated tool payload: {cost:?}");
    }
}
