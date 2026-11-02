use super::super::*;
use etas_host::HostError;

#[tokio::test(flavor = "current_thread")]
async fn incomplete_model_output_settles_usage_and_never_advances_or_records_success() {
    let checked = checked_project(
        r#"
module app.main;
agent Planner() -> string ![] { return perform infer(Prompt.new()); }
agent Child(input: string) -> string ![] { return perform infer(Prompt.new().user(Public(input))); }
flow main() -> string ![Agentic] {
    let plan = Planner.run();
    return Child.run(plan);
}
"#,
    );
    let host = FakeHost::new(availability(&[HostRequirementKind::Agentic]));
    host.seed_model_completion_failure(
        HostError::new(HostErrorCode::IncompleteResponse, "generation truncated"),
        etas_host::ModelUsage {
            input_tokens: 10,
            output_tokens: 4,
            cost: Some(etas_host::ModelCostUsage {
                micros: 20,
                currency: "USD".into(),
            }),
        },
    );
    host.seed_model_response_text("must not run");
    let mut options = RunOptions::default();
    options
        .host_context
        .authority
        .grants
        .push(HostActionGrant::allow("Agentic", "infer"));
    let budget = etas_host::ExecutionBudget::start(Budget {
        tokens: Some(TokenBudget { max_tokens: 100 }),
        cost: Some(etas_host::CostBudget {
            max_micros: 1000,
            currency: "USD".into(),
        }),
        ..Budget::default()
    });
    options.host_context.budget = budget.clone();
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            vec![],
            &host,
            options,
        )
        .await
        .unwrap();
    assert!(
        result
            .diagnostics
            .iter()
            .any(|d| d.message.contains("IncompleteResponse")),
        "{:?}",
        result.diagnostics
    );
    assert!(result.value().is_none());
    assert_eq!(host.model_call_count(), 1);
    assert!(host.tool_requests().is_empty());
    let progress = budget.snapshot().unwrap();
    assert_eq!(progress.consumed_tokens, 14);
    assert_eq!(progress.reserved_tokens, 0);
    assert_eq!(progress.consumed_cost_micros, 20);
    assert_eq!(progress.reserved_cost_micros, 0);
    assert!(result.events.iter().any(|event| matches!(event,
        crate::orchestration::WorkflowEvent::HostTrace(etas_host::TraceEvent::HostRequestFinished {outcome:etas_host::HostOutcome::Failed(error),..})
        if error.code==HostErrorCode::IncompleteResponse)));
}
