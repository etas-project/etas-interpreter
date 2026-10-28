use super::super::*;
use std::collections::BTreeSet;
mod callables;
mod probes;

struct Case {
    symbol: &'static str,
    setup: &'static str,
    assertion: &'static str,
}

fn cases() -> Vec<Case> {
    [
        ("len", "let a = [1, 2]; let l = [1; 2]; let s = a[0, 1); let m = {1 => 2};",
            "a.len() == 2 && l.len() == 2 && s.len() == 1 && m.len() == 1 && std.collections.len(\"abc\") == 3"),
        ("is_empty", "let a: Array<i32> = []; let l: List<i32> = []; let m: Map<i32, i32> = {};",
            "a.is_empty() && l.is_empty() && a[0, 0).is_empty() && m.is_empty() && std.collections.is_empty(\"\")"),
        ("contains_key", "let m = {1 => 2};", "m.contains_key(1) && !m.contains_key(3)"),
        ("array_get", "let a = [4, 5];", "a.get(1) == Some(5) && a.get(-1) == None && a.get(2) == None"),
        ("list_get", "let a = [4; 5];", "a.get(1) == Some(5) && a.get(-1) == None && a.get(2) == None"),
        ("array_at", "let a = [4, 5];", "a.at(1)? == Ok(5) && match a.at(2)? { Ok(_) => false, Err(_) => true }"),
        ("list_at", "let a = [4; 5];", "a.at(1)? == Ok(5) && match a.at(2)? { Ok(_) => false, Err(_) => true }"),
        ("list_head", "let a = [4; 5]; let empty: List<i32> = [];", "a.head() == Some(4) && empty.head() == None"),
        ("list_tail", "let a = [4; 5]; let empty: List<i32> = [];", "a.tail() == Some([5;]) && empty.tail() == None && a == [4; 5]"),
        ("array_push", "let a = [1, 2]; let alias = a;", "a.push(3) == [1, 2, 3] && a == alias && alias == [1, 2]"),
        ("list_push", "let a = [1; 2]; let alias = a;", "a.push(3) == [3; 1; 2] && a == alias && alias == [1; 2]"),
        ("array_pop", "let a = [1, 2]; let empty: Array<i32> = [];", "a.pop() == ([1], Some(2)) && empty.pop() == (empty, None) && a == [1, 2]"),
        ("list_pop", "let a = [1; 2]; let empty: List<i32> = [];", "a.pop() == ([2;], Some(1)) && empty.pop() == (empty, None) && a == [1; 2]"),
        ("array_extend", "let a = [1, 2]; let b = [3, 4];", "a.extend(b) == [1, 2, 3, 4] && a == [1, 2] && b == [3, 4]"),
        ("list_extend", "let a = [1; 2]; let b = [3; 4];", "a.extend(b) == [3; 4; 1; 2] && a == [1; 2] && b == [3; 4]"),
        ("slice_get", "let a = [1, 2, 3][1, 3);", "a.get(0) == Some(2) && a.get(-1) == None && a.get(2) == None"),
        ("slice_at", "let a = [1, 2, 3][1, 3);", "a.at(0)? == Ok(2) && match a.at(2)? { Ok(_) => false, Err(_) => true }"),
        ("to_array", "let a = [1, 2, 3][1, 3);", "a.to_array() == [2, 3]"),
        ("map_get", "let a = {1 => 2, 3 => 4};", "a.get(1) == Some(2) && a.get(9) == None"),
        ("closed", "var n = 0; for i in Range.closed(1, 3) limit Iterations(4) { n = n * 10 + i; }", "n == 123"),
        ("open", "var n = 0; for i in Range.open(1, 3) limit Iterations(4) { n = n * 10 + i; }", "n == 2"),
        ("deque_new", "var n = 0; for _ in Deque.new<i32>() limit Iterations(1) { n = n + 1; }", "n == 0"),
        ("deque_push_front", "let a = Deque.new<i32>().push_back(2); let b = a.push_front(1); var n = 0; for i in b limit Iterations(3) { n = n * 10 + i; }", "n == 12 && second(a.pop_front()) == Some(2)"),
        ("deque_push_back", "let a = Deque.new<i32>().push_front(1); let b = a.push_back(2); var n = 0; for i in b limit Iterations(3) { n = n * 10 + i; }", "n == 12 && second(a.pop_back()) == Some(1)"),
        ("deque_pop_front", "let a = Deque.new<i32>().push_back(1).push_back(2); let (rest, item) = a.pop_front();", "item == Some(1) && second(rest.pop_front()) == Some(2) && second(a.pop_front()) == item && second(Deque.new<i32>().pop_front()) == None"),
        ("deque_pop_back", "let a = Deque.new<i32>().push_back(1).push_back(2); let (rest, item) = a.pop_back();", "item == Some(2) && second(rest.pop_back()) == Some(1) && second(a.pop_back()) == item && second(Deque.new<i32>().pop_back()) == None"),
        ("queue_new", "var n = 0; for _ in Queue.new<i32>() limit Iterations(1) { n = n + 1; }", "n == 0"),
        ("queue_push", "let a = Queue.new<i32>().push(1); let b = a.push(2); var n = 0; for i in b limit Iterations(3) { n = n * 10 + i; }", "n == 12 && second(a.pop()) == Some(1)"),
        ("queue_pop", "let a = Queue.new<i32>().push(1).push(2); let (rest, item) = a.pop();", "item == Some(1) && second(rest.pop()) == Some(2) && second(a.pop()) == item && second(Queue.new<i32>().pop()) == None"),
        ("stack_new", "var n = 0; for _ in Stack.new<i32>() limit Iterations(1) { n = n + 1; }", "n == 0"),
        ("stack_push", "let a = Stack.new<i32>().push(1); let b = a.push(2); var n = 0; for i in b limit Iterations(3) { n = n * 10 + i; }", "n == 12 && second(a.pop()) == Some(1)"),
        ("stack_pop", "let a = Stack.new<i32>().push(1).push(2); let (rest, item) = a.pop();", "item == Some(2) && second(rest.pop()) == Some(1) && second(a.pop()) == item && second(Stack.new<i32>().pop()) == None"),
        ("priority_queue_new", "var n = 0; for _ in PriorityQueue.new<i32, i32>() limit Iterations(1) { n = n + 1; }", "n == 0"),
        ("priority_queue_push", "let a = PriorityQueue.new<i32, i32>().push(1, 10); let b = a.push(2, 20).push(3, 20);", "second(b.pop()) == Some(2) && second(a.pop()) == Some(1)"),
        ("priority_queue_pop", "let a = PriorityQueue.new<i32, i32>().push(1, 10).push(2, 20).push(3, 20); let (rest, item) = a.pop();", "item == Some(2) && second(rest.pop()) == Some(3) && second(a.pop()) == item && second(PriorityQueue.new<i32, i32>().pop()) == None"),
        ("ordered_map_new", "var n = 0; for _ in OrderedMap.new<i32, string>() limit Iterations(1) { n = n + 1; }", "n == 0"),
        ("ordered_map_get", "let a = OrderedMap.new<i32, string>().insert(1, \"one\");", "a.get(1) == Some(\"one\") && a.get(2) == None"),
        ("ordered_map_insert", "let a = OrderedMap.new<i32, i32>().insert(2, 20); let b = a.insert(1, 10).insert(2, 22); var n = 0; for (k, v) in b limit Iterations(3) { n = n * 100 + k * 10 + v; }", "n == 2042 && a.get(2) == Some(20)"),
        ("ordered_map_contains_key", "let a = OrderedMap.new<i32, string>().insert(1, \"one\");", "a.contains_key(1) && !a.contains_key(2)"),
        ("ordered_set_new", "var n = 0; for _ in OrderedSet.new<i32>() limit Iterations(1) { n = n + 1; }", "n == 0"),
        ("ordered_set_insert", "let a = OrderedSet.new<i32>().insert(2); let b = a.insert(1).insert(2); var n = 0; for i in b limit Iterations(3) { n = n * 10 + i; }", "n == 12 && !a.contains(1)"),
        ("ordered_set_contains", "let a = OrderedSet.new<i32>().insert(1);", "a.contains(1) && !a.contains(2)"),
        ("priority_queue_with_compare", "let a = PriorityQueue.new<i32, i32>(reverse).push(1, 10).push(2, 20);", "second(a.pop()) == Some(1)"),
        ("ordered_map_with_compare", "let a = OrderedMap.new<i32, i32>(reverse).insert(1, 10).insert(2, 20); var n = 0; for (k, _) in a limit Iterations(3) { n = n * 10 + k; }", "n == 21"),
        ("ordered_set_with_compare", "let a = OrderedSet.new<i32>(reverse).insert(1).insert(2); var n = 0; for i in a limit Iterations(3) { n = n * 10 + i; }", "n == 21"),
    ]
    .into_iter()
    .map(|(symbol, setup, assertion)| Case { symbol, setup, assertion })
    .collect()
}

#[test]
fn collection_operation_cases_cover_the_exact_registry_surface() {
    let registry = etas_std::standard_registry();
    let declared = registry
        .symbols()
        .filter(|symbol| {
            symbol.qualified_path[..symbol.qualified_path.len() - 1] == ["std", "collections"]
                && matches!(symbol.decl, etas_std::StdDecl::Flow(_))
        })
        .map(|symbol| symbol.name.as_str())
        .collect::<BTreeSet<_>>();
    let cases = cases();
    let covered = cases
        .iter()
        .map(|case| case.symbol)
        .collect::<BTreeSet<_>>();
    assert_eq!(covered.len(), cases.len(), "duplicate conformance case");
    assert_eq!(covered, declared);
}

#[tokio::test(flavor = "current_thread")]
async fn registry_collection_operations_execute_checked_source() {
    for case in cases() {
        eprintln!("collection conformance: {}", case.symbol);
        let source = format!(
            "module app.main; import std.collections; flow reverse(a: i32, b: i32) -> i32 {{ return b - a; }} flow second<A, B>(pair: (A, B)) -> B {{ let (_, value) = pair; return value; }} flow main() -> bool {{ {} return {}; }}",
            case.setup, case.assertion
        );
        let checked = checked_project(&source);
        let result = Interpreter
            .run_checked(
                &checked,
                EntryPoint {
                    item: checked.entry.unwrap(),
                },
                vec![],
                &FakeHost::new(availability(&[])),
                RunOptions::default(),
            )
            .await
            .unwrap();
        assert!(
            result.diagnostics.is_empty(),
            "{}: {:?}",
            case.symbol,
            result.diagnostics
        );
        assert_eq!(
            result.value(),
            Some(&InterpValue::Bool(true)),
            "{}",
            case.symbol
        );
    }
}
