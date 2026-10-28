use super::super::*;

const NESTED: &str = r#"
module app.main;
import std.agent.prompt.Prompt;
import std.runtime.checkpoint;
import std.runtime.limits.Tokens;
agent Writer(input: string) -> string ![] {
    return perform infer(Prompt.new().user(Public(input)));
}
flow inner(input: string) -> string {
    checkpoint("inner");
    return Writer(input);
}
flow outer(input: string) -> string {
    let result = input ~> inner limit Tokens(11);
    checkpoint("outer");
    return Writer(result);
}
flow main() -> string { return "hello" ~> outer limit Tokens(23); }
"#;

fn model_host() -> FakeHost {
    let host = FakeHost::new(availability(&[
        HostRequirementKind::Agentic,
        HostRequirementKind::Checkpoint,
    ]));
    host.seed_model_response_text("draft");
    host.seed_model_response_text("final");
    host
}

fn token_limits(host: &FakeHost) -> Vec<Option<u64>> {
    host.model_requests()
        .iter()
        .map(|request| request.options.max_output_tokens)
        .collect()
}

#[tokio::test(flavor = "current_thread")]
async fn checkpoint_rejects_non_finite_policy_instead_of_serializing_it_as_null() {
    let checked = checked_project(NESTED);
    let host = model_host();
    let mut options = RunOptions::default();
    options.model_policy.options.temperature = Some(f32::NAN);
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
        result.diagnostics.iter().any(|diagnostic| diagnostic
            .message
            .contains("checkpoint model temperature must be finite")),
        "{:?}",
        result.diagnostics
    );
    assert!(result.checkpoints.is_empty());
    assert!(host.model_requests().is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn checkpoint_inside_nested_limited_stages_preserves_the_active_and_outer_scopes() {
    let checked = checked_project(NESTED);
    let host = model_host();
    let first = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            vec![],
            &host,
            RunOptions::default(),
        )
        .await
        .unwrap();
    assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);
    assert_eq!(token_limits(&host), vec![Some(11), Some(23)]);
    assert_eq!(first.checkpoints.len(), 2);
    for (index, expected) in [(0, vec![Some(11), Some(23)]), (1, vec![Some(23)])] {
        for portable in [false, true] {
            let checkpoint = if portable {
                let document =
                    api::codec::checkpoint_artifact_json(&[], "main", &first.checkpoints[index])
                        .unwrap();
                api::codec::checkpoint_from_json(&document, &checked).unwrap()
            } else {
                first.checkpoints[index].clone()
            };
            let resumed_host = model_host();
            let resumed = Interpreter
                .resume_checkpoint(&checked, &checkpoint, &resumed_host, RunOptions::default())
                .await
                .unwrap();
            assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
            assert_eq!(
                token_limits(&resumed_host),
                expected,
                "checkpoint {index}, portable={portable}"
            );
            let limited_host = model_host();
            let mut options = RunOptions::default();
            options.host_context.budget = etas_host::ExecutionBudget::start(etas_host::Budget {
                tokens: Some(TokenBudget { max_tokens: 7 }),
                ..Default::default()
            });
            let resumed = Interpreter
                .resume_checkpoint(&checked, &checkpoint, &limited_host, options)
                .await
                .unwrap();
            assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
            assert_eq!(token_limits(&limited_host), vec![Some(7); expected.len()]);
            assert!(limited_host.model_requests().iter().all(|request| {
                request.budget.limits().tokens == Some(TokenBudget { max_tokens: 7 })
            }));
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn resumed_model_scopes_keep_the_current_explicit_model_override() {
    let checked = checked_project(NESTED);
    let first = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            vec![],
            &model_host(),
            RunOptions::default(),
        )
        .await
        .unwrap();
    assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);
    let host = model_host();
    let mut options = RunOptions::default();
    options.model_policy.model = etas_host::ModelName("current-override".into());
    options.model_policy.model_locked = true;
    let resumed = Interpreter
        .resume_checkpoint(&checked, &first.checkpoints[0], &host, options)
        .await
        .unwrap();
    assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
    assert_eq!(host.model_requests().len(), 2);
    assert!(
        host.model_requests()
            .iter()
            .all(|request| request.model.0 == "current-override")
    );
}
