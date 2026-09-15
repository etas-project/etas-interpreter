use super::*;
use crate::testing::project::checked_project;

#[test]
fn callable_layouts_exclude_lambda_locals_and_unrelated_scopes() {
    let checked = checked_project(
        r#"
module app.main;
flow sibling(other: i32) -> i32 { return other; }
flow main(seed: i32) -> () -> i32 {
    var total = seed;
    if seed > 0 { let branch_local = seed; total = branch_local; }
    return () => { let lambda_local = total; return lambda_local; };
}
"#,
    );
    let slots = SlotLayoutTable::for_project(&checked);
    let layouts = FrameLayoutTable::build(&checked, &slots).unwrap();
    let HirItem::Flow(flow) = &checked.hir.items[checked.entry.unwrap()] else {
        panic!("flow")
    };
    let layout = layouts.get(flow.scope).unwrap();
    let mut names: Vec<_> = layout
        .symbols()
        .iter()
        .map(|symbol| checked.symbols.get(*symbol).unwrap().name.as_str())
        .collect();
    names.sort_unstable();
    assert_eq!(names, ["branch_local", "seed", "total"]);
    assert!(Arc::ptr_eq(layout, layouts.get(flow.scope).unwrap()));
}

#[test]
fn frame_planning_rejects_missing_parameters_and_cyclic_scopes() {
    let mut checked =
        checked_project("module app.main; flow main(seed: i32) -> i32 { return seed; }");
    let slots = SlotLayoutTable::for_project(&checked);
    let HirItem::Flow(flow) = &checked.hir.items[checked.entry.unwrap()] else {
        panic!("flow")
    };
    let scope = flow.scope;
    let parameter = flow.params[0];
    let original = checked.hir.scopes.clone();
    let mut missing = etas_hir::ScopeTree::default();
    for old in original.iter() {
        let id = missing.alloc(old.parent, old.owner, old.span);
        for symbol in old
            .symbols
            .iter()
            .copied()
            .filter(|symbol| *symbol != parameter)
        {
            missing.insert(id, format!("s{}", symbol.0), symbol);
        }
    }
    checked.hir.scopes = missing;
    assert!(
        FrameLayoutTable::build(&checked, &slots)
            .unwrap_err()
            .contains("outside frame scope")
    );
    let mut cyclic = etas_hir::ScopeTree::default();
    for old in original.iter() {
        let parent = if old.id == scope {
            Some(scope)
        } else {
            old.parent
        };
        let id = cyclic.alloc(parent, old.owner, old.span);
        for symbol in &old.symbols {
            cyclic.insert(id, format!("s{}", symbol.0), *symbol);
        }
    }
    checked.hir.scopes = cyclic;
    assert!(
        FrameLayoutTable::build(&checked, &slots)
            .unwrap_err()
            .contains("cyclic frame scope")
    );
}
