use super::*;
use std::cell::Cell;

thread_local! { static COPIES: Cell<usize> = const { Cell::new(0) }; }
pub(super) fn record_copy() {
    COPIES.with(|count| count.set(count.get() + 1));
}

fn run(tree: OrderedTree, key: i32, value: Option<i32>) -> (SearchResult, usize, usize) {
    execute(tree.search(InterpValue::i32(key), value.map(InterpValue::i32)))
}

fn execute(mut search: Search) -> (SearchResult, usize, usize) {
    COPIES.with(|count| count.set(0));
    let mut order = None;
    let mut comparisons = 0;
    loop {
        match search.advance(order.take()).unwrap() {
            SearchStep::Continue => (),
            SearchStep::Complete => break,
            SearchStep::Compare => {
                comparisons += 1;
                let (InterpValue::Number(a), InterpValue::Number(b)) = search.comparison().unwrap()
                else {
                    panic!("integer test keys")
                };
                order = a.partial_cmp_same(*b).unwrap();
            }
        }
    }
    (
        search.finish().unwrap(),
        comparisons,
        COPIES.with(Cell::get),
    )
}

fn validate(tree: &OrderedTree) {
    let mut pending = tree.root.iter().collect::<Vec<_>>();
    while let Some(node) = pending.pop() {
        assert!(node.balance().abs() <= 1);
        assert_eq!(node.height, 1 + height(&node.left).max(height(&node.right)));
        assert_eq!(node.count, 1 + count(&node.left) + count(&node.right));
        pending.extend(node.left.iter());
        pending.extend(node.right.iter());
    }
}

#[test]
fn ordered_tree_balances_every_insertion_permutation() {
    for keys in [
        [1, 2, 3],
        [1, 3, 2],
        [2, 1, 3],
        [2, 3, 1],
        [3, 1, 2],
        [3, 2, 1],
    ] {
        let mut tree = OrderedTree::default();
        for key in keys {
            tree = run(tree, key, Some(key * 10)).0.tree;
            validate(&tree);
        }
        assert_eq!(
            tree.iter().map(|(key, _)| key.clone()).collect::<Vec<_>>(),
            vec![
                InterpValue::i32(1),
                InterpValue::i32(2),
                InterpValue::i32(3)
            ]
        );
        assert_eq!(
            tree.cursor().collect::<Vec<_>>(),
            tree.iter().cloned().collect::<Vec<_>>()
        );
    }
}

#[test]
fn ordered_tree_aliases_copy_only_the_updated_path_and_keep_old_values() {
    let mut tree = OrderedTree::default();
    for key in 0..4096 {
        tree = run(tree, key, Some(key)).0.tree;
    }
    let old = tree.clone();
    let (updated, comparisons, copies) = run(tree, 2048, Some(-1));
    assert!(comparisons <= 14);
    assert!(copies <= 14, "copied {copies} nodes instead of an AVL path");
    assert_eq!(
        run(old.clone(), 2048, None).0.found,
        Some(InterpValue::i32(2048))
    );
    assert_eq!(
        run(updated.tree.clone(), 2048, None).0.found,
        Some(InterpValue::i32(-1))
    );
    assert_eq!(updated.tree.len(), 4096);
    validate(&old);
    validate(&updated.tree);
}

#[test]
fn ordered_tree_lookup_and_unique_updates_do_not_copy_nodes() {
    for count in [1000, 2000, 4000] {
        let mut tree = OrderedTree::default();
        let mut insert_comparisons = 0;
        for key in (0..count).rev() {
            let (updated, comparisons, copies) = run(tree, key, Some(key));
            assert_eq!(copies, 0);
            insert_comparisons += comparisons;
            tree = updated.tree;
        }
        let (result, comparisons, copies) = run(tree.clone(), count / 2, None);
        assert_eq!(copies, 0);
        assert!(comparisons <= 14);
        assert_eq!(result.found, Some(InterpValue::i32(count / 2)));
        let (shared, _, shared_copies) = run(tree, count / 2, Some(-1));
        assert!(shared_copies <= 14);
        drop(result);
        let (updated, _, unique_copies) = run(shared.tree, count / 2, Some(-2));
        assert_eq!(unique_copies, 0);
        validate(&updated.tree);
        eprintln!(
            "ordered count={count} insert_comparisons={insert_comparisons} lookup_comparisons={comparisons} lookup_copies=0 shared_update_copies={shared_copies} unique_update_copies={unique_copies}"
        );
    }
}

#[test]
fn ordered_tree_copy_on_write_shares_payload_bytes() {
    let payload = InterpValue::String("payload".repeat(10000).into());
    let mut tree = OrderedTree::default();
    for key in 0..1000 {
        tree = execute(tree.search(InterpValue::i32(key), Some(payload.clone())))
            .0
            .tree;
    }
    let original = tree.clone();
    let ((changed, comparisons, copies), allocation) =
        crate::testing::allocation::measure(|| run(tree, 500, Some(-1)));
    assert!(comparisons <= 14 && copies <= 14);
    assert!(
        allocation.bytes < 16384,
        "copied payload bytes: {allocation:?}"
    );
    let (InterpValue::String(a), InterpValue::String(b)) = (
        &original.entry(499).unwrap().1,
        &changed.tree.entry(499).unwrap().1,
    ) else {
        panic!("retained payload")
    };
    assert_eq!(a.as_ptr(), b.as_ptr());
}

#[test]
fn ordered_tree_cursor_is_linear_and_does_not_materialize_a_value_snapshot() {
    for count in [1000, 2000, 4000] {
        let mut tree = OrderedTree::default();
        for key in 0..count {
            tree = run(tree, key, Some(key)).0.tree;
        }
        let (mut cursor, setup) = crate::testing::allocation::measure(|| tree.cursor());
        assert_eq!(setup.count, 0);
        let (visited, cost) = crate::testing::allocation::measure(|| {
            let mut visited = 0;
            for (key, value) in &mut cursor {
                assert_eq!(key, InterpValue::i32(visited));
                assert_eq!(value, key);
                visited += 1;
            }
            visited
        });
        assert_eq!(visited, count);
        assert!(cost.bytes < 1024, "materialized ordered entries: {cost:?}");
        eprintln!(
            "ordered traversal count={count} allocations={} bytes={} snapshot_entries=0",
            cost.count, cost.bytes
        );
    }
}

#[test]
fn ordered_tree_release_is_stack_safe_for_deeply_nested_collections() {
    for explicit_release in [false, true] {
        let mut value = InterpValue::Unit;
        for _ in 0..4000 {
            let tree = execute(OrderedTree::default().search(InterpValue::i32(0), Some(value)))
                .0
                .tree;
            value = InterpValue::OrderedMap(OrderedMapValue::from_tree(tree, None));
        }
        if explicit_release {
            crate::value::shared::release_value(value);
        } else {
            drop(value);
        }
    }
}
