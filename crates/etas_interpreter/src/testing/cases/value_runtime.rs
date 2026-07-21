use super::super::*;

#[tokio::test(flavor = "current_thread")]
async fn run_checked_preserves_wide_integer_and_float_runtime_types() {
    let checked = checked_project(
        r#"
module app.main;

flow main() -> bool {
  let signed: i128 = 170141183460469231731687303715884105727;
  let unsigned: u128 = 340282366920938463463374607431768211455;
  let wide: u64 = 18446744073709551615;
  let small: f32 = 1.5;
  let precise: f64 = -4.0;
  return signed > 0
    && unsigned > 0
    && wide > 0
    && small + 2.25 == 3.75
    && precise / -2.0 == 2.0;
}
"#,
    );

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &FakeHost::new(HostServiceAvailability::default()),
            RunOptions::default(),
        )
        .await;

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value, Some(value::InterpValue::Bool(true)));
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_preserves_scalar_nominal_constructor_identity() {
    let checked = checked_project(
        r#"
module app.main;

type UserId = string;

flow main() -> UserId {
  return UserId("user-42");
}
"#,
    );

    let expected_type = checked
        .entry
        .and_then(|item| checked.types.item_signatures.get(&item))
        .and_then(|signature| match signature {
            etas_types::ItemSignature::Flow(flow) => Some(flow.output),
            _ => None,
        })
        .expect("checked nominal flow output");
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &FakeHost::new(HostServiceAvailability::default()),
            RunOptions::default(),
        )
        .await;

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.value,
        Some(value::InterpValue::Nominal {
            ty: expected_type,
            value: Box::new(value::InterpValue::String("user-42".to_owned())),
        })
    );
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_reports_integer_overflow_without_panicking() {
    let checked = checked_project(
        r#"
module app.main;

flow main() -> i8 {
  let max: i8 = 127;
  return max + 1;
}
"#,
    );

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &FakeHost::new(HostServiceAvailability::default()),
            RunOptions::default(),
        )
        .await;

    assert!(result.value.is_none());
    assert!(result.diagnostics.iter().any(|diagnostic| {
        diagnostic.message.contains("numeric addition failed")
            && diagnostic.message.contains("overflows")
    }));
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_reports_integer_division_by_zero_without_panicking() {
    let checked = checked_project(
        r#"
module app.main;

flow main() -> i32 {
  let one: i32 = 1;
  return one / 0;
}
"#,
    );

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &FakeHost::new(HostServiceAvailability::default()),
            RunOptions::default(),
        )
        .await;

    assert!(result.value.is_none());
    assert!(result.diagnostics.iter().any(|diagnostic| {
        diagnostic.message.contains("numeric division failed")
            && diagnostic.message.contains("divisor is zero")
    }));
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_concatenates_arrays_and_lists_without_mutating_operands() {
    let checked = checked_project(
        r#"
module app.main;

flow main() -> i32 {
  let left_array = [1, 2];
  let joined_array = left_array + [3, 4];
  let left_list: List<i32> = [5; 6];
  let joined_list = left_list + [7; 8];
  return left_array[0] * 1000
      + joined_array[3] * 100
      + left_list[0] * 10
      + joined_list[3];
}
"#,
    );

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &FakeHost::new(HostServiceAvailability::default()),
            RunOptions::default(),
        )
        .await;

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value, Some(value::InterpValue::i32(1458)));
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_preserves_range_literal_runtime_values() {
    let checked = checked_project(
        r#"
module app.main;

flow main() -> Range<i32> {
  return [0, 3);
}
"#,
    );

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &FakeHost::new(HostServiceAvailability::default()),
            RunOptions::default(),
        )
        .await;

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.value,
        Some(value::InterpValue::Range(value::RangeValue {
            start: Box::new(value::InterpValue::i32(0)),
            end: Box::new(value::InterpValue::i32(3)),
            bounds: value::RangeBounds::ClosedOpen,
        }))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_executes_range_for_iteration_without_host() {
    let checked = checked_project(
        r#"
module app.main;

flow main() -> i32 {
  var sum = 0;

  for value in (0, 4] limit Iterations(8) {
    sum = sum + value;
  }

  return sum;
}
"#,
    );

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &FakeHost::new(HostServiceAvailability::default()),
            RunOptions::default(),
        )
        .await;

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value, Some(value::InterpValue::i32(10)));
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_executes_list_set_and_slice_for_iteration_without_host() {
    let checked = checked_project(
        r#"
module app.main;

flow main() -> i32 {
  var sum = 0;
  let list: List<i32> = [1; 2; 3];
  for value in list limit Iterations(16) {
    sum = sum + value;
  }

  let set: Set<i32> = #{4, 5};
  for value in set limit Iterations(16) {
    sum = sum + value;
  }

  let array = [6, 7, 8];
  let window = array[1, 3);
  for value in window limit Iterations(16) {
    sum = sum + value;
  }

  return sum;
}
"#,
    );

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &FakeHost::new(HostServiceAvailability::default()),
            RunOptions::default(),
        )
        .await;

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value, Some(value::InterpValue::i32(30)));
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_executes_array_slice_without_host() {
    let checked = checked_project(
        r#"
module app.main;

flow main() -> i32 {
  let values = [1, 2, 3, 4];
  let left = values[1, 3);
  let right = values(0, 2];
  var sum = 0;

  for value in left limit Iterations(8) {
    sum = sum + value;
  }

  return left[0] + right[1] + sum;
}
"#,
    );

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &FakeHost::new(HostServiceAvailability::default()),
            RunOptions::default(),
        )
        .await;

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value, Some(value::InterpValue::i32(10)));
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_executes_range_slice_without_host() {
    let checked = checked_project(
        r#"
module app.main;

flow main() -> i32 {
  let range: Range<i32> = [0, 10);
  let window = range[2, 5);
  var sum = 0;

  for value in window limit Iterations(8) {
    sum = sum + value;
  }

  return sum;
}
"#,
    );

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &FakeHost::new(HostServiceAvailability::default()),
            RunOptions::default(),
        )
        .await;

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value, Some(value::InterpValue::i32(9)));
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_keeps_value_semantics_across_flow_calls() {
    let checked = checked_project(
        r#"
module app.main;

flow bump(values: Array<i32>) -> i32 {
  values[0] = values[0] + 1;
  return values[0];
}

flow main() -> i32 {
  var values = [4, 7];
  let seen = bump(values);
  return seen * 10 + values[0];
}
"#,
    );

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &FakeHost::new(HostServiceAvailability::default()),
            RunOptions::default(),
        )
        .await;

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value, Some(value::InterpValue::i32(54)));
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_executes_index_and_nested_field_assignment_without_host() {
    let checked = checked_project(
        r#"
module app.main;

type Interval = {
  start: i32,
  end: i32,
};

flow main() -> i32 {
  var out = [Interval { start = 1, end = 2 }];
  out[0].end = 9;
  return out[0].end;
}
"#,
    );

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &FakeHost::new(HostServiceAvailability::default()),
            RunOptions::default(),
        )
        .await;

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value, Some(value::InterpValue::i32(9)));
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_executes_lambda_and_closure_without_host() {
    let checked = checked_project(
        r#"
module app.main;

flow make_adder(delta: i32) -> i32 -> i32 {
  return (value: i32) => value + delta;
}

flow main() -> i32 {
  let add = make_adder(5);
  return add(7);
}
"#,
    );

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &FakeHost::new(HostServiceAvailability::default()),
            RunOptions::default(),
        )
        .await;

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value, Some(value::InterpValue::i32(12)));
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_resumes_callee_expression_after_host_boundary() {
    let checked = checked_project(
        r#"
module app.main;
import std.io.read_line;

flow choose(_kind: string) -> i32 -> i32 {
  return (value: i32) => value + 1;
}

flow main() -> i32 ![Error<IOError>] {
  return choose(read_line())(6);
}
"#,
    );

    let host = FakeHost::new(availability(&[HostRequirementKind::Console]));
    host.seed_stdin("inc\n");

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &host,
            RunOptions::default(),
        )
        .await;

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value, Some(value::InterpValue::i32(7)));
    assert_eq!(host.console_call_count(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_executes_returned_lambda_composition_without_host() {
    let checked = checked_project(
        r#"
module app.main;

flow compose(f: i32 -> i32, g: i32 -> i32) -> i32 -> i32 {
  return (value: i32) => f(g(value));
}

flow main() -> i32 {
  let double = (value: i32) => value * 2;
  let inc = (value: i32) => value + 1;
  let pipeline = compose(double, inc);
  return pipeline(10);
}
"#,
    );

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &FakeHost::new(HostServiceAvailability::default()),
            RunOptions::default(),
        )
        .await;

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value, Some(value::InterpValue::i32(22)));
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_executes_local_std_callables_without_host() {
    let checked = checked_project(
        r#"
module app.main;

import std.collections.{List, is_empty, len};
import std.text.{len as text_len};

flow main() -> usize {
  let values = [1, 2, 3];
  if is_empty(values) {
    return 0;
  }

  let text = "abc";
  let empty: List<i32> = [];
  if text_len(text) > 0 && is_empty(empty) {
    return len(values) + text_len(text);
  }

  return 0;
}
"#,
    );

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &FakeHost::new(HostServiceAvailability::default()),
            RunOptions::default(),
        )
        .await;

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value, Some(value::InterpValue::usize(6)));
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_executes_json_parse_and_stringify_without_host() {
    let checked = checked_project(
        r#"
module app.main;

import std.json.{JsonValue, parse, stringify};

flow main() -> string {
  return match parse("[true,\"ok\"]") {
    Ok(value) => match stringify(value) {
      Ok(text) => text,
      Err(_) => "stringify-error"
    },
    Err(_) => "parse-error"
  };
}
"#,
    );

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &FakeHost::new(HostServiceAvailability::default()),
            RunOptions::default(),
        )
        .await;

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.value,
        Some(value::InterpValue::String("[true,\"ok\"]".to_owned()))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_rejects_invalid_json_parse_without_host() {
    let checked = checked_project(
        r#"
module app.main;

import std.json.parse;

flow main() -> string {
  return match parse("{not-json}") {
    Ok(_) => "unexpected",
    Err(_) => "invalid-json"
  };
}
"#,
    );

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &FakeHost::new(HostServiceAvailability::default()),
            RunOptions::default(),
        )
        .await;

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.value,
        Some(value::InterpValue::String("invalid-json".to_owned()))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_executes_advanced_collections_without_host() {
    let checked = checked_project(
        r#"
module app.main;

flow main() -> (Option<i32>, Option<i32>, Option<i32>, Option<i32>, bool, Option<string>) {
  let deque = Deque.new<i32>().push_back(2).push_front(1);
  let (_deque_tail, deque_front) = deque.pop_front();

  let queue = Queue.new<i32>().push(3).push(4);
  let (_queue_tail, queue_front) = queue.pop();

  let stack = Stack.new<i32>().push(5).push(6);
  let (_stack_tail, stack_top) = stack.pop();

  let ordered_map = OrderedMap.new<string, i32>().insert("a", 7);
  let ordered_set = OrderedSet.new<string>().insert("x");

  let priority = PriorityQueue.new<string, i32>().push("low", 1).push("high", 2);
  let (_priority_tail, priority_value) = priority.pop();

  return (
    deque_front,
    queue_front,
    stack_top,
    ordered_map.get("a"),
    ordered_set.contains("x"),
    priority_value
  );
}
"#,
    );

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &FakeHost::new(HostServiceAvailability::default()),
            RunOptions::default(),
        )
        .await;

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.value,
        Some(value::InterpValue::Tuple(vec![
            value::InterpValue::OptionSome(Box::new(value::InterpValue::i32(1))),
            value::InterpValue::OptionSome(Box::new(value::InterpValue::i32(3))),
            value::InterpValue::OptionSome(Box::new(value::InterpValue::i32(6))),
            value::InterpValue::OptionSome(Box::new(value::InterpValue::i32(7))),
            value::InterpValue::Bool(true),
            value::InterpValue::OptionSome(Box::new(value::InterpValue::String("high".to_owned()))),
        ]))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_executes_prompt_trust_wrappers_without_host() {
    let checked = checked_project(
        r#"
module app.main;
import std.agent.prompt.Prompt;

flow main(input: string) -> Prompt {
  let trusted: Trusted<string> = Trusted(input);
  return Prompt.new().system(trusted).user(Public("body"));
}
"#,
    );

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            vec![value::InterpValue::String("policy".to_owned())],
            &FakeHost::new(HostServiceAvailability::default()),
            RunOptions::default(),
        )
        .await;

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.value,
        Some(value::InterpValue::Prompt(vec![
            value::PromptMessage {
                role: value::PromptRole::System,
                text: "policy".to_owned(),
                trust: Some(etas_types::TrustWrapper::Trusted),
            },
            value::PromptMessage {
                role: value::PromptRole::User,
                text: "body".to_owned(),
                trust: Some(etas_types::TrustWrapper::Public),
            },
        ]))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_preserves_array_and_list_literal_runtime_values() {
    let array_project = checked_project(
        r#"
module app.main;

flow main() -> Array<i32> {
  return [1, 2];
}
"#,
    );

    let array_result = Interpreter
        .run_checked(
            &array_project,
            EntryPoint {
                item: array_project.entry.expect("entry item"),
            },
            Vec::new(),
            &FakeHost::new(HostServiceAvailability::default()),
            RunOptions::default(),
        )
        .await;

    assert!(
        array_result.diagnostics.is_empty(),
        "{:?}",
        array_result.diagnostics
    );
    assert_eq!(
        array_result.value,
        Some(value::InterpValue::Array(value::ArrayValue::new(vec![
            value::InterpValue::i32(1),
            value::InterpValue::i32(2),
        ])))
    );

    let list_project = checked_project(
        r#"
module app.main;

flow main() -> List<i32> {
  return [1; 2];
}
"#,
    );

    let list_result = Interpreter
        .run_checked(
            &list_project,
            EntryPoint {
                item: list_project.entry.expect("entry item"),
            },
            Vec::new(),
            &FakeHost::new(HostServiceAvailability::default()),
            RunOptions::default(),
        )
        .await;

    assert!(
        list_result.diagnostics.is_empty(),
        "{:?}",
        list_result.diagnostics
    );
    assert_eq!(
        list_result.value,
        Some(value::InterpValue::List(
            vec![value::InterpValue::i32(1), value::InterpValue::i32(2)].into()
        ))
    );

    let cons_project = checked_project(
        r#"
module app.main;

flow main() -> List<i32> {
  return 1 :: 2 :: [];
}
"#,
    );

    let cons_result = Interpreter
        .run_checked(
            &cons_project,
            EntryPoint {
                item: cons_project.entry.expect("entry item"),
            },
            Vec::new(),
            &FakeHost::new(HostServiceAvailability::default()),
            RunOptions::default(),
        )
        .await;

    assert!(
        cons_result.diagnostics.is_empty(),
        "{:?}",
        cons_result.diagnostics
    );
    assert_eq!(
        cons_result.value,
        Some(value::InterpValue::List(
            vec![value::InterpValue::i32(1), value::InterpValue::i32(2)].into()
        ))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_preserves_map_and_set_literal_runtime_values() {
    let map_project = checked_project(
        r#"
module app.main;

flow main() -> Map<string, i32> {
  return { "alice" => 10, "bob" => 8 };
}
"#,
    );

    let map_result = Interpreter
        .run_checked(
            &map_project,
            EntryPoint {
                item: map_project.entry.expect("entry item"),
            },
            Vec::new(),
            &FakeHost::new(HostServiceAvailability::default()),
            RunOptions::default(),
        )
        .await;

    assert!(
        map_result.diagnostics.is_empty(),
        "{:?}",
        map_result.diagnostics
    );
    assert_eq!(
        map_result.value,
        Some(value::InterpValue::Map(
            vec![
                (
                    value::InterpValue::String("alice".to_owned()),
                    value::InterpValue::i32(10)
                ),
                (
                    value::InterpValue::String("bob".to_owned()),
                    value::InterpValue::i32(8)
                ),
            ]
            .into()
        ))
    );

    let set_project = checked_project(
        r#"
module app.main;

flow main() -> Set<i32> {
  return #{1, 2};
}
"#,
    );

    let set_result = Interpreter
        .run_checked(
            &set_project,
            EntryPoint {
                item: set_project.entry.expect("entry item"),
            },
            Vec::new(),
            &FakeHost::new(HostServiceAvailability::default()),
            RunOptions::default(),
        )
        .await;

    assert!(
        set_result.diagnostics.is_empty(),
        "{:?}",
        set_result.diagnostics
    );
    assert_eq!(
        set_result.value,
        Some(value::InterpValue::Set(
            vec![value::InterpValue::i32(1), value::InterpValue::i32(2)].into()
        ))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_resumes_model_boundary_in_assignment_target_index() {
    let checked = checked_project(
        r#"
module app.main;
import std.agent.prompt.Prompt;

@model(model = "local-qwen")
agent Pick(input: string) -> i32 ![] {
  return Prompt.new().user(Public(input));
}

flow main(input: string) -> i32 {
  var values: Array<i32> = [0, 0];
  values[Pick.run(input)] = 7;
  return values[1];
}
"#,
    );

    let host = FakeHost::new(availability(&[HostRequirementKind::Agentic]));
    host.seed_model_response_text("1");
    let options = RunOptions {
        host_context: api::HostExecutionContext {
            authority: AuthorityContext {
                grants: vec![HostActionGrant::allow("Agentic", "infer")],
                approvals: Vec::new(),
                sandbox: SandboxPolicy::deny_all(),
                policy: Default::default(),
            },
            trace: TraceContext::root(TraceId(59)),
            budget: Budget::default(),
        },
        model_policy: api::ModelExecutionPolicy {
            provider_capabilities: Some(full_model_capabilities()),
            ..api::ModelExecutionPolicy::default()
        },
        ..RunOptions::default()
    };
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            vec![value::InterpValue::String("choose one".to_owned())],
            &host,
            options,
        )
        .await;

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value, Some(value::InterpValue::i32(7)));
    assert_eq!(host.model_call_count(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_executes_local_map_index_assign_without_host() {
    let checked = checked_project(
        r#"
module app.main;

flow main(values: Map<i32, i32>) -> i32 {
  values[1] = 40;
  values[2] = values[1] + 2;
  return values[2];
}
"#,
    );

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            vec![value::InterpValue::Map(Vec::new().into())],
            &FakeHost::new(HostServiceAvailability::default()),
            RunOptions::default(),
        )
        .await;

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value, Some(value::InterpValue::i32(42)));
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_executes_imported_source_flow_without_host() {
    let checked = checked_project_sources(vec![
        (
            SourceId(0),
            "src/app/main.es",
            r#"
module app.main;
import app.support.math.twice;

flow main() -> i32 {
  return twice(21);
}
"#,
        ),
        (
            SourceId(1),
            "src/app/support/math.es",
            r#"
module app.support.math;

public flow twice(value: i32) -> i32 {
  return value * 2;
}
"#,
        ),
    ]);

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &FakeHost::new(HostServiceAvailability::default()),
            RunOptions::default(),
        )
        .await;

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value, Some(value::InterpValue::i32(42)));
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_executes_stage_composition_without_host() {
    let checked = checked_project(
        r#"
module app.main;

flow increment(value: i32) -> i32 {
  return value + 1;
}

flow double(value: i32) -> i32 {
  return value * 2;
}

flow main() -> i32 {
  let pipeline = increment | double;
  return pipeline(10);
}
"#,
    );

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &FakeHost::new(HostServiceAvailability::default()),
            RunOptions::default(),
        )
        .await;

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value, Some(value::InterpValue::i32(22)));
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_executes_string_index_and_list_pop_without_host() {
    let checked = checked_project(
        r#"
module app.main;

flow main() -> char {
  var stack = ['a', 'm', 'c'];
  let middle = "roman"[2];
  stack.pop();
  if stack[1] == middle {
    return middle;
  }
  return 'x';
}
"#,
    );

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &FakeHost::new(HostServiceAvailability::default()),
            RunOptions::default(),
        )
        .await;

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.value,
        Some(value::InterpValue::String("m".to_owned()))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_resolves_top_level_const_path() {
    let checked = checked_project(
        r#"
module app.main;

let Greeting = "ok";

flow main() -> string {
  return Greeting;
}
"#,
    );

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &FakeHost::new(HostServiceAvailability::default()),
            RunOptions::default(),
        )
        .await;

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.value,
        Some(value::InterpValue::String("ok".to_owned()))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_resolves_top_level_memory_region_handle_path() {
    let checked = checked_project(
        r#"
module app.main;

alias ProjectMemorySchema = MemoryRegion<{
  Papers: Store<string, string>
}>;

let ProjectMemory =
  std.memory.region<ProjectMemorySchema>(
    stable_id = "project_memory",
    store = "project-main"
  );

flow main() -> unit {
  let memory = ProjectMemory;
  return;
}
"#,
    );

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &FakeHost::new(HostServiceAvailability::default()),
            RunOptions::default(),
        )
        .await;

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value, Some(value::InterpValue::Unit));
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_resolves_memory_store_field_path() {
    let checked = checked_project(
        r#"
module app.main;

alias ProjectMemorySchema = MemoryRegion<{
  Papers: Store<string, string>
}>;

let ProjectMemory =
  std.memory.region<ProjectMemorySchema>(
    stable_id = "project_memory",
    store = "project-main"
  );

flow main() -> unit {
  let papers = ProjectMemory.Papers;
  return;
}
"#,
    );

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &FakeHost::new(HostServiceAvailability::default()),
            RunOptions::default(),
        )
        .await;

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value, Some(value::InterpValue::Unit));
}
