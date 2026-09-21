use super::*;
use crate::value::{ArrayValue, SliceValue};

#[derive(Clone, Copy, Debug)]
enum Family {
    Array,
    List,
    Slice,
    Set,
    Map,
    Deque,
    Queue,
    Stack,
}

impl Family {
    fn ty(self) -> &'static str {
        match self {
            Self::Array => "Array<string>",
            Self::List => "List<string>",
            Self::Slice => "Slice<string>",
            Self::Set => "Set<string>",
            Self::Map => "Map<i32, string>",
            Self::Deque => "Deque<string>",
            Self::Queue => "Queue<string>",
            Self::Stack => "Stack<string>",
        }
    }

    fn pattern(self) -> &'static str {
        if matches!(self, Self::Map) {
            "(_, element)"
        } else {
            "element"
        }
    }

    fn input(self, count: usize) -> InterpValue {
        let values: Vec<_> = (0..count)
            .map(|i| InterpValue::String(format!("{i:08}{}", "x".repeat(128)).into()))
            .collect();
        match self {
            Self::Array => InterpValue::Array(values.into()),
            Self::List => InterpValue::List(values.into()),
            Self::Slice => InterpValue::Slice(
                SliceValue::from_array(ArrayValue::new(values), 0..count).unwrap(),
            ),
            Self::Set => InterpValue::Set(values.into()),
            Self::Map => InterpValue::Map(
                values
                    .into_iter()
                    .enumerate()
                    .map(|(i, v)| (InterpValue::i32(i as i32), v))
                    .collect::<Vec<_>>()
                    .into(),
            ),
            Self::Deque => InterpValue::Deque(values.into()),
            Self::Queue => InterpValue::Queue(values.into()),
            Self::Stack => InterpValue::Stack(values.into()),
        }
    }
}

const FAMILIES: [Family; 8] = [
    Family::Array,
    Family::List,
    Family::Slice,
    Family::Set,
    Family::Map,
    Family::Deque,
    Family::Queue,
    Family::Stack,
];

#[test]
fn checked_early_break_execution_does_not_scale_with_unvisited_payload() {
    for family in FAMILIES {
        let program = PreparedExecution::new(&format!(
            "module app.main; flow main(input: {}) -> string {{ for {} in input limit Iterations(5000) {{ return element; }} return \"empty\"; }}",
            family.ty(),
            family.pattern(),
        ));
        let mut baseline = None;
        for count in [1000, 2000, 4000] {
            let args = [family.input(count)];
            let (result, cost, elapsed) = program.run(&args);
            assert_eq!(
                result,
                InterpValue::String(format!("00000000{}", "x".repeat(128)).into())
            );
            eprintln!("checked early traversal {family:?} n={count}: {cost:?}, {elapsed:?}");
            if let Some((allocs, bytes)) = baseline {
                assert_eq!(
                    (cost.count, cost.bytes),
                    (allocs, bytes),
                    "unvisited input was materialized for {family:?}"
                );
            }
            baseline = Some((cost.count, cost.bytes));
        }
    }
}

#[test]
fn checked_full_iteration_execution_has_linear_allocation_scaling() {
    for family in FAMILIES {
        let program = PreparedExecution::new(&format!(
            "module app.main; flow main(input: {}) -> i32 {{ var count = 0; for {} in input limit Iterations(5000) {{ count = count + 1; }} return count; }}",
            family.ty(),
            family.pattern(),
        ));
        let mut previous: Option<Allocations> = None;
        for count in [1000, 2000, 4000] {
            let args = [family.input(count)];
            let (result, cost, elapsed) = program.run(&args);
            assert_eq!(result, InterpValue::i32(count as i32));
            eprintln!("checked full traversal {family:?} n={count}: {cost:?}, {elapsed:?}");
            // Includes the real evaluator/driver, not only the collection cursor.
            // Eager continuation flattening used 45 allocations per element (47 for Map).
            assert!(
                cost.count <= 20 * count + 100,
                "per-element scheduling allocations regressed for {family:?}: {cost:?}"
            );
            if let Some(previous) = previous {
                assert!(
                    cost.count <= previous.count * 2,
                    "superlinear allocation count: {cost:?}"
                );
                assert!(
                    cost.bytes <= previous.bytes * 2,
                    "superlinear allocated bytes: {cost:?}"
                );
            }
            previous = Some(cost);
        }
    }
}

#[test]
fn checked_sequential_collection_iteration_keeps_the_evaluated_version() {
    for (constructor, push, pop, first, second, order) in [
        (
            "Deque.new<i32>().push_back(1).push_front(2)",
            "push_back",
            "pop_front",
            2,
            1,
            21,
        ),
        ("Queue.new<i32>().push(1).push(2)", "push", "pop", 1, 2, 12),
        ("Stack.new<i32>().push(1).push(2)", "push", "pop", 2, 1, 12),
    ] {
        let program = PreparedExecution::new(&format!(
            r#"
module app.main;
flow main() -> bool {{
    var values = {constructor};
    let before = values;
    var seen = 0;
    for element in values limit Iterations(4) {{
        seen = seen * 10 + element;
        values = values.{push}(9);
    }}
    let (tail, a) = before.{pop}();
    let (empty, b) = tail.{pop}();
    let (_, c) = empty.{pop}();
    var updated_count = 0;
    for element in values limit Iterations(8) {{ updated_count = updated_count + 1; }}
    return seen == {order} && updated_count == 4 && a == Some({first}) && b == Some({second}) && c == None;
}}
"#
        ));
        assert_eq!(program.run(&[]).0, InterpValue::Bool(true));
    }
}

#[test]
fn checked_huge_range_early_exit_never_materializes_the_original_interval() {
    use crate::value::{NumericValue, RangeBounds, RangeValue};
    let program = PreparedExecution::new(
        "module app.main; flow main(input: Range<u128>) -> u128 { for n in input limit Iterations(4) { return n; } return 0; }",
    );
    let mut baseline = None;
    for end in [1000, 2000, 4000, u128::MAX] {
        let input = InterpValue::Range(RangeValue {
            start: Box::new(InterpValue::Number(NumericValue::U128(7))),
            end: Box::new(InterpValue::Number(NumericValue::U128(end))),
            bounds: RangeBounds::ClosedClosed,
        });
        let (result, cost, elapsed) = program.run(&[input]);
        assert_eq!(result, InterpValue::Number(NumericValue::U128(7)));
        eprintln!("checked early range end={end}: {cost:?}, {elapsed:?}");
        if let Some(expected) = baseline {
            assert_eq!((cost.count, cost.bytes), expected);
        }
        baseline = Some((cost.count, cost.bytes));
    }
}
