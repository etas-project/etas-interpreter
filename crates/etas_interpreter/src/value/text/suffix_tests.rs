use super::*;
use crate::testing::allocation::measure;
use etas_builtin::text::input::TextInput;

fn segmented(prefix: &str, suffix: &str) -> StringValue {
    let prefix = StringValue::from(prefix);
    let alias = prefix.clone();
    let result = prefix.concat(suffix.into()).unwrap();
    drop(alias);
    assert!(result.contiguous().is_none());
    result
}

#[test]
fn unique_concat_appends_segmented_suffix_without_a_payload_copy() {
    let mut results = Vec::new();
    for count in [1000, 2000, 4000] {
        let prefix = "中😀payload".repeat(count);
        let suffix = segmented(&prefix, "tail");
        let mut left = String::with_capacity("left:".len() + suffix.len());
        left.push_str("left:");
        let pointer = left.as_ptr();
        let left = StringValue::from(left);
        let (result, cost) = measure(|| left.concat(suffix.clone()).unwrap());
        let cached = suffix.contiguous().is_some();
        assert_eq!(result.as_ptr(), pointer);
        assert_eq!(result, format!("left:{prefix}tail"));
        results.push((count, suffix.len(), cost, cached));
    }
    eprintln!("unique segmented suffix: {results:?}");
    for (count, bytes, cost, cached) in results {
        assert!(!cached, "n={count}: retained {bytes} B RHS cache");
        assert_eq!(cost.bytes, 0, "n={count}: {cost:?}");
        assert_eq!(cost.count, 0, "n={count}: {cost:?}");
    }
}

#[test]
fn concat_shares_both_segmented_operands_without_flattening_either() {
    let mut results = Vec::new();
    for count in [1000, 2000, 4000] {
        let left_text = "left中😀".repeat(count);
        let right_text = "right中😀".repeat(count);
        let left = segmented(&left_text, "L");
        let right = segmented(&right_text, "R");
        let left_alias = left.clone();
        let (result, cost) = measure(|| left.concat(right.clone()).unwrap());
        let left_cached = left_alias.contiguous().is_some();
        let right_cached = right.contiguous().is_some();
        assert_eq!(result.len(), left_text.len() + right_text.len() + 2);
        let (rendered, output_cost) = measure(|| result.clone().into_string());
        assert_eq!(rendered, format!("{left_text}L{right_text}R"));
        assert_eq!(left_alias, format!("{left_text}L"));
        assert_eq!(right, format!("{right_text}R"));
        results.push((count, cost, output_cost, left_cached, right_cached));
        assert!(left_alias.contiguous().is_none(), "render cached the LHS");
        assert!(right.contiguous().is_none(), "render cached the RHS");
    }
    eprintln!("shared segmented operands: {results:?}");
    for (count, cost, _, left_cached, right_cached) in results {
        assert!(!left_cached && !right_cached, "n={count}: input was cached");
        assert_eq!(cost.count, 1, "n={count}: only one concat node: {cost:?}");
        assert!(cost.bytes < 256, "n={count}: payload was copied: {cost:?}");
    }
}

#[test]
fn unique_small_prefix_shares_the_rhs_instead_of_growing_a_payload_copy() {
    let mut costs = Vec::new();
    for count in [1000, 2000, 4000] {
        for segmented_rhs in [false, true] {
            let payload = "中😀".repeat(count);
            let right = if segmented_rhs {
                segmented(&payload, "tail")
            } else {
                payload.clone().into()
            };
            let right_alias = right.clone();
            let (result, cost) = measure(|| StringValue::from("L").concat(right).unwrap());
            let cached = segmented_rhs && right_alias.contiguous().is_some();
            let expected = if segmented_rhs {
                format!("L{payload}tail")
            } else {
                format!("L{payload}")
            };
            assert_eq!(result.clone().into_string(), expected);
            costs.push((count, segmented_rhs, cost, cached));
        }
    }
    eprintln!("small unique prefix: {costs:?}");
    for (count, segmented_rhs, cost, cached) in costs {
        assert!(!cached, "n={count}: cached RHS");
        assert!(
            cost.bytes < 256,
            "n={count}, segmented={segmented_rhs}: {cost:?}"
        );
    }
}

fn branch(left: StringValue, right: StringValue) -> StringValue {
    let alias = left.clone();
    let result = left.concat(right).unwrap();
    drop(alias);
    result
}

fn shapes(parts: &[&str]) -> Vec<StringValue> {
    if parts.len() == 1 {
        return vec![parts[0].into()];
    }
    let mut output = Vec::new();
    for split in 1..parts.len() {
        let right = shapes(&parts[split..]);
        for left in shapes(&parts[..split]) {
            for right in &right {
                output.push(branch(left.clone(), right.clone()));
            }
        }
    }
    output
}

#[test]
fn binary_text_shapes_preserve_forward_reverse_and_mixed_chunk_order() {
    let parts = ["first", "e\u{301}", "中", "😀", "\0", "last"];
    let expected = parts.concat();
    for text in shapes(&parts) {
        assert_eq!(text.chunks().collect::<Vec<_>>(), parts);
        assert_eq!(
            text.chunks().rev().collect::<Vec<_>>(),
            parts.into_iter().rev().collect::<Vec<_>>()
        );
        let mut chunks = text.chunks();
        assert_eq!(chunks.len(), 6);
        assert_eq!(chunks.next(), Some("first"));
        assert_eq!(chunks.next_back(), Some("last"));
        assert_eq!(chunks.next(), Some("e\u{301}"));
        assert_eq!(chunks.next_back(), Some("\0"));
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks.next(), Some("中"));
        assert_eq!(chunks.next_back(), Some("😀"));
        assert_eq!(chunks.len(), 0);
        assert_eq!(chunks.next(), None);
        assert_eq!(chunks.next_back(), None);
        assert!(text.contiguous().is_none());
        assert_eq!(text.clone().into_string(), expected);
        assert!(text.contiguous().is_none());
    }
}

#[test]
fn binary_text_cursor_and_snapshot_survive_child_caches_and_last_owner_release() {
    let left = segmented("left", "中");
    let right = segmented("😀", "right");
    let Repr::Concat(node) = &right.0 else {
        panic!("segmented right child")
    };
    let weak = Rc::downgrade(node);
    let text = branch(left.clone(), right.clone());
    let mut cursor = text.chunks();
    assert_eq!(left.as_str(), "left中");
    assert_eq!(right.as_str(), "😀right");
    assert_eq!(cursor.next_back(), Some("right"));
    assert_eq!(cursor.next(), Some("left"));
    assert_eq!(cursor.collect::<String>(), "中😀");
    let value = crate::value::InterpValue::String(text.clone());
    let (snapshot, cost) =
        measure(|| crate::orchestration::ValueSnapshot::capture(&value).unwrap());
    assert_eq!(cost.count, 0);
    drop(value);
    drop(text);
    drop(left);
    drop(right);
    assert!(weak.upgrade().is_some());
    let restored = snapshot.restore().unwrap();
    let crate::value::InterpValue::String(restored) = restored else {
        panic!("text snapshot")
    };
    assert_eq!(restored.clone().into_string(), "left中😀right");
    drop(restored);
    assert!(weak.upgrade().is_none());
}

#[test]
fn deep_right_and_branching_text_use_normal_iterative_render_and_drop() {
    if crate::testing::in_subprocess("ETAS_BINARY_TEXT_WORKER") {
        return;
    }
    let piece = StringValue::from("中😀");
    for render in [false, true] {
        let mut text = StringValue::from("tail");
        for _ in 0..30_000 {
            text = piece.clone().concat(text).unwrap();
        }
        let alias = text.clone();
        if render {
            assert_eq!(text.into_string(), format!("{}tail", "中😀".repeat(30_000)));
            assert!(alias.contiguous().is_none());
        } else {
            drop(text);
        }
        let (_, cost) = measure(|| drop(alias));
        assert_eq!(
            cost.count, 0,
            "a right-deep chain needs no release worklist"
        );
    }
    let mut tree = piece.clone();
    for _ in 0..15 {
        tree = branch(tree.clone(), tree);
    }
    assert_eq!(tree.len(), piece.len() * (1 << 15));
    assert_eq!(tree.clone().into_string(), "中😀".repeat(1 << 15));
    assert!(tree.contiguous().is_none());
    drop(tree);
    let mut trees = vec![piece; 1 << 14];
    while trees.len() > 1 {
        trees = trees
            .chunks_exact(2)
            .map(|pair| branch(pair[0].clone(), pair[1].clone()))
            .collect();
    }
    drop(trees);
}

#[test]
fn binary_concat_capacity_failure_preserves_shared_operands_without_rendering() {
    let mut value = StringValue::from("x");
    while value.len() <= isize::MAX as usize / 2 {
        value = branch(value.clone(), value);
    }
    let length = value.len();
    let alias = value.clone();
    assert!(value.concat(alias.clone()).is_err());
    assert_eq!(alias.len(), length);
    assert!(alias.contiguous().is_none());
    let (_, cost) = measure(|| {
        let mut chunks = alias.chunks();
        assert_eq!(chunks.len(), length);
        assert_eq!(chunks.next(), Some("x"));
        assert_eq!(chunks.next_back(), Some("x"));
        assert_eq!(chunks.len(), length - 2);
    });
    assert!(
        cost.bytes < 8192,
        "an early read expanded the shared graph: {cost:?}"
    );
    assert_eq!(cost.released_bytes, cost.bytes);
}
