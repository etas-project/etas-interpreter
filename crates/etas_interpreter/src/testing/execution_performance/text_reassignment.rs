use super::*;

#[test]
fn checked_text_reassignment_has_linear_execution_allocation_cost() {
    for expression in ["text + piece", "append(text, piece)"] {
        let program = PreparedExecution::new(&format!(
            r#"
module app.main;
flow append(left: string, right: string) -> string {{ return left + right; }}
flow main(input: Array<i32>, piece: string) -> string {{
    var text = "";
    for _ in input limit Iterations(5000) {{ text = {expression}; }}
    return text;
}}
"#
        ));
        let mut costs = Vec::new();
        for count in [1000, 2000, 4000] {
            let args = [
                InterpValue::Array(
                    (0..count)
                        .map(|_| InterpValue::i32(0))
                        .collect::<Vec<_>>()
                        .into(),
                ),
                InterpValue::String("x".repeat(256).into()),
            ];
            let (value, cost, elapsed) = program.run(&args);
            let InterpValue::String(text) = value else {
                panic!("text result")
            };
            let (output, output_cost) = measure(|| text.into_string());
            assert_eq!(output, "x".repeat(count * 256));
            eprintln!(
                "text reassignment {expression} n={count}: execution={cost:?}, output={output_cost:?}, {elapsed:?}"
            );
            costs.push(cost);
        }
        for pair in costs.windows(2) {
            assert!(
                pair[1].bytes <= pair[0].bytes * 5 / 2,
                "doubling the input copied cumulative text prefixes: {pair:?}"
            );
        }
    }
}
