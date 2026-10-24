use super::*;
use crate::testing::allocation::measure;

#[test]
fn scheduling_a_leaf_continuation_reuses_the_machine_stack() {
    let mut machine = EvalMachine::new();
    machine.push_continuation_frames(Continuation::Return);
    machine.pop_frame().unwrap();
    let (_, allocations) = measure(|| {
        for _ in 0..1000 {
            machine.push_continuation_frames(Continuation::Return);
            assert!(matches!(
                machine.pop_frame().unwrap().into_continuation(),
                Continuation::Return
            ));
            machine.push_continuation_frames(Continuation::BlockValue);
            assert!(matches!(
                machine.push_outer_continuation_frames(Continuation::Return),
                Some(Continuation::Return)
            ));
            assert!(matches!(
                machine.push_outer_continuation_frames(Continuation::BlockValue),
                Some(Continuation::BlockValue)
            ));
        }
    });
    assert!(machine.frames().is_empty());
    assert_eq!(allocations.count, 0, "{allocations:?}");
    assert_eq!(allocations.bytes, 0, "{allocations:?}");
}

fn marker(index: u32) -> Continuation {
    Continuation::TryExpr {
        expr: etas_hir::HirExprId(index),
        span: etas_core::Span::empty(etas_core::SourceId(0), etas_core::TextSize::ZERO),
    }
}

fn chain(inner: Continuation, outer: Continuation) -> Continuation {
    Continuation::Chain {
        inner: inner.into(),
        outer: outer.into(),
    }
}

#[test]
fn nested_shared_continuations_keep_inner_before_outer_order() {
    let shared: crate::control::ContinuationLink = chain(marker(1), marker(2)).into();
    let tree = Continuation::Chain {
        inner: chain(marker(0), shared.clone().into_value()).into(),
        outer: chain(shared.clone().into_value(), marker(3)).into(),
    };
    let mut machine = EvalMachine::new();
    machine.push_continuation_frames(tree);
    for expected in [0, 1, 2, 1, 2, 3] {
        let Continuation::TryExpr { expr, .. } = machine.pop_frame().unwrap().into_continuation()
        else {
            panic!("wrong continuation kind");
        };
        assert_eq!(expr.0, expected);
    }
    assert!(machine.frames().is_empty());
    assert!(matches!(&*shared, Continuation::Chain { .. }));
}

#[test]
fn apply_keeps_the_first_leaf_and_preserves_outer_block_value_frames() {
    let mut machine = EvalMachine::new();
    machine.push_continuation_frames(marker(99));
    let first = machine.push_outer_continuation_frames(chain(
        chain(Continuation::BlockValue, marker(0)),
        chain(Continuation::BlockValue, marker(1)),
    ));
    assert!(matches!(first, Some(Continuation::BlockValue)));
    for expected in [Some(0), None, Some(1), Some(99)] {
        match (machine.pop_frame().unwrap().into_continuation(), expected) {
            (Continuation::TryExpr { expr, .. }, Some(expected)) => assert_eq!(expr.0, expected),
            (Continuation::BlockValue, None) => {}
            _ => panic!("Apply changed continuation order or skipped a leaf"),
        }
    }
    assert!(machine.frames().is_empty());
}

#[test]
fn deep_continuation_chains_schedule_without_rust_recursion() {
    for outer_nested in [false, true] {
        let mut tree = marker(0);
        for index in 1..10000 {
            tree = if outer_nested {
                chain(marker(index), tree)
            } else {
                chain(tree, marker(index))
            };
        }
        let mut machine = EvalMachine::new();
        machine.push_continuation_frames(tree);
        for index in 0..10000 {
            let Continuation::TryExpr { expr, .. } =
                machine.pop_frame().unwrap().into_continuation()
            else {
                panic!("wrong continuation kind");
            };
            assert_eq!(expr.0, if outer_nested { 9999 - index } else { index });
        }
        assert!(machine.frames().is_empty());
    }
}
