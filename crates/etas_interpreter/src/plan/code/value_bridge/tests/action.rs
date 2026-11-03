use super::*;
use crate::{control::CheckedActionInvocation, value::CheckedExecutionProgram};

fn source_invocation(prepared: &Arc<PreparedCodeRevision>) -> CheckedActionInvocation {
    prepared.type_bridge().bind_runtime_owner(prepared);
    let checked = prepared.type_bridge().original_program().clone();
    let expr = checked
        .hir
        .exprs
        .iter()
        .find_map(|(id, expr)| matches!(expr, etas_hir::HirExpr::Perform { .. }).then_some(id))
        .unwrap();
    let item = checked
        .hir
        .items
        .iter()
        .find_map(|(id, item)| {
            matches!(item, etas_hir::HirItem::Flow(flow) if checked.symbols.get(flow.symbol).is_some_and(|symbol| symbol.name == "invoke")).then_some(id)
        })
        .unwrap();
    let plan = Arc::new(
        prepared
            .source_program()
            .select_callable(etas_frontend::CheckedCallableRoot::Item(item))
            .unwrap(),
    );
    CheckedActionInvocation::source(
        CheckedExecutionProgram {
            checked,
            plan,
            definitions: Some(prepared.source_definitions.clone()),
            revision: None,
        },
        expr,
        &Default::default(),
    )
    .unwrap()
}

fn revision() -> Arc<PreparedCodeRevision> {
    prepared_body(
        "type UserId = string; effect Query { action choose(value: UserId) -> UserId; } flow invoke(value: UserId) -> UserId ![Query.choose] { return perform Query.choose(value); }",
        "UserId",
        "UserId",
        "return value;",
    )
    .0
}

#[test]
fn action_origin_replays_only_contiguous_checked_nominal_revision_projections() {
    let prepared = revision();
    let original = source_invocation(&prepared);
    let candidate = original
        .project(&RevisionProjection::Candidate(
            prepared.type_bridge().clone(),
        ))
        .unwrap();
    assert!(Arc::ptr_eq(&candidate.program, prepared.program()));
    candidate.validate_origin().unwrap();
    let restored = candidate
        .project(&RevisionProjection::Original(
            prepared.type_bridge().clone(),
        ))
        .unwrap();
    assert_eq!(restored.effect, original.effect);
    assert_eq!(restored.parameters, original.parameters);
    assert_eq!(restored.output, original.output);
    restored.validate_origin().unwrap();
    let mut invalid = candidate;
    invalid.output = TypeId(u32::MAX);
    assert!(invalid.validate_origin().is_err());
    let foreign = revision();
    foreign.type_bridge().bind_runtime_owner(&foreign);
    assert!(
        original
            .project(&RevisionProjection::Candidate(
                foreign.type_bridge().clone()
            ))
            .unwrap_err()
            .contains("not contiguous")
    );
}

#[test]
fn deep_action_origin_chains_share_edges_and_release_pinned_revisions_iteratively() {
    let prepared = revision();
    let weak = Arc::downgrade(&prepared);
    let mut proof = source_invocation(&prepared);
    for _ in 0..4096 {
        proof = proof
            .project(&RevisionProjection::Candidate(
                prepared.type_bridge().clone(),
            ))
            .unwrap();
        proof = proof
            .project(&RevisionProjection::Original(
                prepared.type_bridge().clone(),
            ))
            .unwrap();
    }
    let (aliases, cost) =
        crate::testing::allocation::measure(|| (0..100).map(|_| proof.clone()).collect::<Vec<_>>());
    assert!(
        cost.bytes < 32_000,
        "action alias copied its projection chain: {cost:?}"
    );
    proof.validate_origin().unwrap();
    drop(prepared);
    assert!(
        weak.upgrade().is_some(),
        "live invocation lost its native revision"
    );
    drop(proof);
    assert!(
        weak.upgrade().is_some(),
        "aliased invocation lost its native revision"
    );
    drop(aliases);
    assert!(
        weak.upgrade().is_none(),
        "action evidence leaked a revision ownership cycle"
    );
}
