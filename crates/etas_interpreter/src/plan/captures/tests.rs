use super::*;
use crate::testing::{allocation::measure, project::checked_project};

#[test]
fn capture_planning_rejects_a_resolved_local_from_a_sibling_flow() {
    let mut checked = checked_project(
        r#"
module app.main;
flow sibling(other: i32) -> i32 { return other; }
flow main(seed: i32) -> () -> i32 { return () => seed; }
"#,
    );
    let seed = checked
        .hir
        .symbols
        .iter()
        .find(|symbol| symbol.name == "seed")
        .unwrap()
        .id;
    let other = checked
        .hir
        .symbols
        .iter()
        .find(|symbol| symbol.name == "other")
        .unwrap()
        .id;
    let expr = checked
        .hir
        .exprs
        .iter()
        .find_map(|(id, data)| match data {
            HirExpr::Path(path) if path.resolution == ResolveResult::Resolved(seed) => Some(id),
            _ => None,
        })
        .unwrap();
    crate::testing::edit_checked_hir(&mut checked, |hir| {
        let HirExpr::Path(path) = hir.exprs.get_mut(expr).unwrap() else {
            panic!("path");
        };
        path.resolution = ResolveResult::Resolved(other);
    })
    .unwrap();
    let error = ClosureLayoutTable::build(&checked).unwrap_err();
    assert!(error.contains("outside the lexical scope"), "{error}");
}

#[test]
fn closure_planning_scales_linearly_with_local_uses() {
    let mut previous = None;
    for count in [1000, 2000, 4000] {
        let mut source =
            "module app.main; flow main(seed: i32) -> () -> i32 { return () => {".to_owned();
        for n in 0..count {
            source.push_str(&format!("let local_{n} = seed;\n"));
        }
        source.push_str("return seed; }; }");
        let checked = checked_project(&source);
        let (layouts, allocations) = measure(|| ClosureLayoutTable::build(&checked).unwrap());
        let scopes = checked.hir.scopes.iter().count();
        assert_eq!(layouts.layouts.len(), 1);
        let layout = layouts.layouts.values().next().unwrap();
        assert_eq!(layout.captures.len(), 1);
        assert_eq!(
            checked.hir.symbols.get(layout.captures[0]).unwrap().name,
            "seed"
        );
        assert_eq!(layout.slots.slot_count(), count + 1);
        eprintln!(
            "closure plan n={count}: scopes={scopes}, total plan allocations={allocations:?}"
        );
        if let Some(previous) = previous {
            assert!(
                allocations.bytes <= previous * 3,
                "capture allocation growth exceeds linear scale"
            );
        }
        previous = Some(allocations.bytes);
    }
}
