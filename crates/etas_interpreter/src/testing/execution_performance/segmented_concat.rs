use super::*;
use crate::value::StringValue;
use etas_builtin::text::input::TextInput;

fn cold(prefix: String) -> StringValue {
    let prefix = StringValue::from(prefix);
    let alias = prefix.clone();
    let result = prefix.concat("tail".into()).unwrap();
    drop(alias);
    assert!(result.contiguous().is_none());
    result
}

#[test]
fn checked_binary_text_concat_allocates_no_payload_sized_intermediate() {
    let program = PreparedExecution::new(
        r#"
module app.main;
flow main(left: string, right: string) -> string { return left + right; }
"#,
    );
    // Initialize executor-local bookkeeping with unrelated input. Every measured
    // operand below is newly constructed and still has no contiguous cache.
    program.run(&[
        InterpValue::String("a".into()),
        InterpValue::String("b".into()),
    ]);
    let mut costs = Vec::new();
    for count in [1000, 2000, 4000] {
        let left = cold("left中😀".repeat(count));
        let right = cold("right中😀".repeat(count));
        let args = [
            InterpValue::String(left.clone()),
            InterpValue::String(right.clone()),
        ];
        let (value, cost, elapsed) = program.run(&args);
        let cached = left.contiguous().is_some() || right.contiguous().is_some();
        let InterpValue::String(value) = value else {
            panic!("text result")
        };
        let (output, output_cost) = measure(|| value.into_string());
        assert_eq!(
            output,
            format!(
                "{}tail{}tail",
                "left中😀".repeat(count),
                "right中😀".repeat(count)
            )
        );
        assert!(left.contiguous().is_none() && right.contiguous().is_none());
        eprintln!(
            "checked binary concat n={count}: execution={cost:?}, output={output_cost:?}, cached={cached}, {elapsed:?}"
        );
        costs.push((cost, cached));
    }
    for (cost, cached) in &costs {
        assert!(!cached, "input cache retained during execution");
        assert_eq!(cost.count, costs[0].0.count);
        assert_eq!(
            cost.bytes, costs[0].0.bytes,
            "input size changed execution allocation: {costs:?}"
        );
    }
}

#[test]
fn checked_text_prepend_has_linear_execution_allocation_cost() {
    let program = PreparedExecution::new(
        r#"
module app.main;
flow main(input: Array<i32>) -> string {
    var text = "tail";
    for _ in input limit Iterations(5000) { text = "中😀" + text; }
    return text;
}
"#,
    );
    let mut costs = Vec::new();
    for count in [1000, 2000, 4000] {
        let args = [InterpValue::Array(
            (0..count)
                .map(|_| InterpValue::i32(0))
                .collect::<Vec<_>>()
                .into(),
        )];
        let (value, cost, elapsed) = program.run(&args);
        let InterpValue::String(value) = value else {
            panic!("text result")
        };
        let (output, output_cost) = measure(|| value.into_string());
        assert_eq!(output, format!("{}tail", "中😀".repeat(count)));
        eprintln!(
            "checked prepend n={count}: execution={cost:?}, output={output_cost:?}, {elapsed:?}"
        );
        costs.push(cost);
    }
    for pair in costs.windows(2) {
        assert!(
            pair[1].bytes <= pair[0].bytes * 5 / 2,
            "prepend copied growing payloads: {pair:?}"
        );
    }
}
