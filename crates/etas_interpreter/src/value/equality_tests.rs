use super::InterpValue as V;
use crate::{orchestration::ValueSnapshot, value::membership::MembershipValue};

#[test]
fn set_comparison_resumes_after_failed_collision_candidates() {
    use super::comparison::SetSearch;

    let candidates = |_: usize, offset: usize| (offset < 2).then_some(offset);
    for mut search in [SetSearch::new(2), SetSearch::unique(2)] {
        assert_eq!(search.next(None, candidates), Ok(Some((0, 0))));
        assert_eq!(search.next(Some(false), candidates), Ok(Some((0, 1))));
        assert_eq!(search.next(Some(true), candidates), Ok(Some((1, 0))));
        assert_eq!(search.next(Some(true), candidates), Ok(None));
    }
}

#[test]
fn snapshot_membership_cannot_reuse_one_matching_member() {
    let number = |n| ValueSnapshot::Number(super::NumericValue::I32(n));
    let duplicate = ValueSnapshot::Set(vec![number(1), number(1)].into());
    let distinct = ValueSnapshot::Set(vec![number(1), number(2)].into());
    assert!(!duplicate.member_eq(&distinct));
    assert!(!distinct.member_eq(&duplicate));
}

fn subprocess(name: &str, case: &str) {
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", name, "--nocapture"])
        .env("ETAS_EQUALITY_SUBPROCESS", case)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn deep_runtime_aggregate_equality_is_stack_safe() {
    if std::env::var("ETAS_EQUALITY_SUBPROCESS").as_deref() != Ok("runtime") {
        return subprocess(
            "value::equality_tests::deep_runtime_aggregate_equality_is_stack_safe",
            "runtime",
        );
    }
    let chain = |leaf| {
        (0..30_000).fold(V::i32(leaf), |value, n| match n % 3 {
            0 => V::Tuple(vec![value].into()),
            1 => V::Array(vec![value].into()),
            _ => V::Nominal {
                ty: etas_types::TypeId(17),
                value: value.into(),
            },
        })
    };
    let a = chain(1);
    let b = chain(1);
    let different = chain(2);
    assert!(a == b);
    assert!(a != different);
}

#[test]
fn deep_snapshot_set_membership_equality_is_stack_safe() {
    if std::env::var("ETAS_EQUALITY_SUBPROCESS").as_deref() != Ok("snapshot") {
        return subprocess(
            "value::equality_tests::deep_snapshot_set_membership_equality_is_stack_safe",
            "snapshot",
        );
    }
    let chain = |leaf| {
        (0..30_000).fold(
            ValueSnapshot::Number(super::NumericValue::I32(leaf)),
            |value, _| ValueSnapshot::Set(vec![value].into()),
        )
    };
    let a = chain(1);
    let b = chain(1);
    let different = chain(2);
    assert!(a.member_eq(&b));
    assert!(!a.member_eq(&different));
}
