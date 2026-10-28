use super::*;
use crate::{
    control::{CallTarget, Frame},
    value::{InterpValue, RecordValue},
};

fn captured_dag(depth: usize, reverse: bool, leaf: bool) -> V {
    let value = (0..depth).fold(InterpValue::Bool(leaf), |child, _| {
        let mut fields = vec![("a".into(), child.clone()), ("b".into(), child)];
        if reverse {
            fields.reverse();
        }
        InterpValue::Callable(CallTarget::Lambda {
            expr: etas_hir::HirExprId(7),
            captured: Frame::from_snapshot(vec![(
                etas_hir::SymbolId(1),
                InterpValue::Record(RecordValue::new(fields)),
            )])
            .unwrap(),
        })
    });
    V::capture(&value).unwrap()
}

#[test]
fn record_membership_visits_captured_dag_pairs_not_logical_expansion() {
    for depth in [20, 1000, 2000, 4000] {
        let a = captured_dag(depth, false, true);
        let b = captured_dag(depth, true, true);
        let different = captured_dag(depth, true, false);
        let context = Context::default();
        assert!(compare(node(&a, &b, &context)));
        assert!(
            context.visits.get() <= 8 * depth + 4,
            "depth={depth}, visits={}",
            context.visits.get()
        );
        assert!(!equal(&a, &different));
    }
}
