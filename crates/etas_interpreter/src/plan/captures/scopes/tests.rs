use super::*;
use etas_core::{SourceId, Span, TextRange, TextSize};
use etas_hir::HirBlockId;

fn span() -> Span {
    Span::new(SourceId(17), TextRange::new(TextSize(0), TextSize(1)))
}
fn block() -> ScopeOwner {
    ScopeOwner::Block(HirBlockId(0))
}

#[test]
fn missing_parents_and_parent_cycles_are_rejected() {
    let mut missing = ScopeTree::default();
    missing.alloc(Some(ScopeId(7)), block(), span());
    assert!(
        ClosureScopes::build(&missing)
            .err()
            .unwrap()
            .contains("missing closure scope parent")
    );
    let mut cyclic = ScopeTree::default();
    cyclic.alloc(None, block(), span());
    cyclic.alloc(Some(ScopeId(2)), block(), span());
    cyclic.alloc(Some(ScopeId(1)), block(), span());
    assert!(
        ClosureScopes::build(&cyclic)
            .err()
            .unwrap()
            .contains("cyclic")
    );
    let mut self_cycle = ScopeTree::default();
    self_cycle.alloc(Some(ScopeId(0)), block(), span());
    assert!(
        ClosureScopes::build(&self_cycle)
            .err()
            .unwrap()
            .contains("cyclic")
    );
}

#[test]
fn forward_parent_references_do_not_require_allocation_order() {
    let mut scopes = ScopeTree::default();
    let child = scopes.alloc(Some(ScopeId(1)), block(), span());
    let root = scopes.alloc(None, ScopeOwner::Lambda(HirExprId(5)), span());
    let index = ClosureScopes::build(&scopes).unwrap();
    assert!(index.contains(root, child).unwrap());
    assert!(!index.contains(child, root).unwrap());
    assert_eq!(
        index.nearest(child).unwrap(),
        Some(ClosureScope {
            scope: root,
            expr: HirExprId(5)
        })
    );
    assert!(index.nearest(ScopeId(99)).is_err());
}

#[test]
fn intervals_and_nearest_closures_agree_with_lexical_parent_walks() {
    let mut scopes = ScopeTree::default();
    for n in 0..96 {
        let parent = if n % 13 == 0 {
            None
        } else {
            Some(ScopeId((n * 17 + 3) % n))
        };
        let owner = if n % 5 == 0 {
            ScopeOwner::Lambda(HirExprId(n))
        } else {
            block()
        };
        scopes.alloc(parent, owner, span());
    }
    let index = ClosureScopes::build(&scopes).unwrap();
    for scope in scopes.iter() {
        let mut ancestors = Vec::new();
        let mut current = Some(scope.id);
        while let Some(id) = current {
            let ancestor = scopes.get(id).unwrap();
            ancestors.push(id);
            current = ancestor.parent;
        }
        for possible in scopes.iter() {
            assert_eq!(
                index.contains(possible.id, scope.id).unwrap(),
                ancestors.contains(&possible.id)
            );
        }
        let closures: Vec<_> = ancestors
            .iter()
            .filter_map(|id| match scopes.get(*id).unwrap().owner {
                ScopeOwner::Lambda(expr) => Some(ClosureScope { scope: *id, expr }),
                _ => None,
            })
            .collect();
        assert_eq!(index.nearest(scope.id).unwrap(), closures.first().copied());
        for (position, closure) in closures.iter().enumerate() {
            assert_eq!(
                index.outer(*closure).unwrap(),
                closures.get(position + 1).copied()
            );
        }
    }
}

#[test]
fn deep_scope_queries_do_not_allocate_or_rewalk_parent_chains() {
    let mut scopes = ScopeTree::default();
    let root = scopes.alloc(None, ScopeOwner::Lambda(HirExprId(1)), span());
    let mut last = root;
    for _ in 0..30_000 {
        last = scopes.alloc(Some(last), block(), span());
    }
    let index = ClosureScopes::build(&scopes).unwrap();
    let (_, allocations) = crate::testing::allocation::measure(|| {
        for scope in scopes.iter() {
            assert!(index.contains(root, scope.id).unwrap());
            assert_eq!(index.nearest(scope.id).unwrap().unwrap().scope, root);
        }
    });
    assert_eq!(allocations.count, 0);
    assert!(index.contains(root, last).unwrap());
}
