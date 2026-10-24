use super::{HostJsonSupportValue as Json, InterpValue, membership::MembershipIndex};
use crate::{orchestration::ValueSnapshot, testing::allocation::measure};

fn key(n: usize) -> Json {
    Json::Object(
        vec![(
            "key".into(),
            Json::Array(vec![Json::String(n.to_string().into())].into()),
        )]
        .into(),
    )
}

#[test]
fn json_clone_and_snapshot_capture_share_immutable_payloads() {
    for count in [1000, 2000, 4000] {
        let value = InterpValue::Json(Json::Object(
            (0..count)
                .map(|n| (n.to_string(), Json::String("x".repeat(1024).into())))
                .collect(),
        ));
        let (alias, cloned) = measure(|| value.clone());
        let (snapshot, captured) = measure(|| ValueSnapshot::capture(&value).unwrap());
        let (snapshot_alias, snapshot_cloned) = measure(|| snapshot.clone());
        let (restored, restored_allocations) = measure(|| snapshot_alias.restore().unwrap());
        assert!(value == alias && alias == restored);
        eprintln!(
            "JSON clone n={count}: {cloned:?}, capture={captured:?}, snapshot_clone={snapshot_cloned:?}, restore={restored_allocations:?}"
        );
        assert_eq!(cloned.count, 0);
        assert_eq!(captured.count, 0);
        assert_eq!(snapshot_cloned.count, 0);
        assert_eq!(restored_allocations.count, 0);
    }
}

#[test]
fn json_text_clone_is_constant_cost_and_cow_preserves_snapshot() {
    for count in [1000, 2000, 4000] {
        let mut value = InterpValue::Json(Json::String("x".repeat(count).into()));
        let (alias, allocations) = measure(|| value.clone());
        assert_eq!(allocations.count, 0);
        let saved = ValueSnapshot::capture(&value).unwrap();
        let InterpValue::Json(Json::String(text)) = &mut value else {
            panic!("text");
        };
        text.push_str("new");
        assert_eq!(saved.restore().unwrap(), alias);
        assert_ne!(value, alias);
    }
}

#[test]
fn deep_json_clone_and_drop_preserve_shared_and_unique_lifetimes() {
    if std::env::var("ETAS_JSON_SUBPROCESS").as_deref() != Ok("lifetime") {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "value::json_tests::deep_json_clone_and_drop_preserve_shared_and_unique_lifetimes",
                "--nocapture",
            ])
            .env("ETAS_JSON_SUBPROCESS", "lifetime")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    for branching in [false, true] {
        let value = (0..30_000).fold(Json::Bool(true), |child, n| {
            if n % 2 == 0 {
                let mut children = vec![child];
                if branching {
                    children.push(Json::Null);
                }
                Json::Array(children.into())
            } else {
                let mut fields = vec![("child".into(), child)];
                if branching {
                    fields.push(("sibling".into(), Json::Null));
                }
                Json::Object(fields.into())
            }
        });
        let (alias, allocations) = measure(|| value.clone());
        assert_eq!(allocations.count, 0, "{allocations:?}");
        drop(value);
        let (_, released) = measure(|| drop(alias));
        if branching {
            assert!(
                released.count < 32,
                "only pending cursor stack growth: {released:?}"
            );
        } else {
            assert_eq!(
                released.count, 0,
                "unique chain reuses detached buffers: {released:?}"
            );
        }
    }
}

#[test]
fn json_membership_queries_use_structural_partitions() {
    for count in [1000, 2000, 4000] {
        let runtime: Vec<_> = (0..count).map(|n| InterpValue::Json(key(n))).collect();
        let snapshots: Vec<_> = (0..count).map(|n| ValueSnapshot::Json(key(n))).collect();
        let index = MembershipIndex::require_unique(&runtime).unwrap();
        let snapshot_index = MembershipIndex::require_unique(&snapshots).unwrap();
        for n in 0..count {
            let value = InterpValue::Json(key(n));
            assert!(index.contains(&runtime, &value, index.fingerprint(&value)));
            let missing = InterpValue::Json(key(n + count));
            assert!(!index.contains(&runtime, &missing, index.fingerprint(&missing)));
            let value = ValueSnapshot::Json(key(n));
            assert!(snapshot_index.contains(
                &snapshots,
                &value,
                snapshot_index.fingerprint(&value)
            ));
            let missing = ValueSnapshot::Json(key(n + count));
            assert!(!snapshot_index.contains(
                &snapshots,
                &missing,
                snapshot_index.fingerprint(&missing)
            ));
        }
        eprintln!(
            "JSON n={count}: runtime={}, snapshot={} candidates",
            index.comparison_count(),
            snapshot_index.comparison_count()
        );
        assert!(index.comparison_count() <= count * 2);
        assert!(snapshot_index.comparison_count() <= count * 2);
    }
}

#[test]
fn deep_json_comparison_is_stack_safe() {
    if std::env::var("ETAS_JSON_SUBPROCESS").as_deref() != Ok("equality") {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "value::json_tests::deep_json_comparison_is_stack_safe",
                "--nocapture",
            ])
            .env("ETAS_JSON_SUBPROCESS", "equality")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let chain = |leaf| {
        (0..30_000).fold(Json::Bool(leaf), |child, n| {
            if n % 2 == 0 {
                Json::Array(vec![child].into())
            } else {
                Json::Object(vec![("child".into(), child)].into())
            }
        })
    };
    let a = chain(true);
    let b = chain(true);
    let c = chain(false);
    let (equal, allocations) = measure(|| a == b);
    let unequal = a != c;
    let fingerprint = |v: &Json| {
        use std::hash::Hasher;
        let mut hash = std::hash::DefaultHasher::new();
        super::json::hash(v, &mut hash);
        hash.finish()
    };
    let (ha, hash_allocations) = measure(|| fingerprint(&a));
    assert_eq!(ha, fingerprint(&b));
    assert_ne!(ha, fingerprint(&c));
    let early_a = Json::Array(vec![Json::Bool(false), a].into());
    let early_b = Json::Array(vec![Json::Bool(true), b].into());
    let (short_circuit, short_allocations) = measure(|| early_a != early_b);
    for value in [early_a, early_b, c] {
        drop(value);
    }
    assert!(equal && unequal);
    assert!(
        allocations.count < 32,
        "only traversal stack growth: {allocations:?}"
    );
    assert!(hash_allocations.count < 32, "{hash_allocations:?}");
    assert!(short_circuit);
    assert_eq!(short_allocations.count, 0, "must not visit the deep tail");
    eprintln!("JSON depth=30000: equality={allocations:?}, hash={hash_allocations:?}");
}

#[test]
fn flat_json_comparison_and_hash_borrow_large_payloads() {
    for count in [1000, 2000, 4000] {
        let fields = || {
            (0..count)
                .map(|n| (n.to_string(), Json::String("x".repeat(1024).into())))
                .collect()
        };
        let a = Json::Object(fields());
        let b = Json::Object(fields());
        let (equal, comparison) = measure(|| a == b);
        assert!(equal);
        assert_eq!(comparison.count, 0, "{comparison:?}");
        let mut hasher = std::hash::DefaultHasher::new();
        let (_, hashing) = measure(|| super::json::hash(&a, &mut hasher));
        assert_eq!(hashing.count, 0, "{hashing:?}");
    }
}

#[test]
fn json_equality_retains_tags_order_labels_and_numeric_bits() {
    fn reference(a: &Json, b: &Json) -> bool {
        match (a, b) {
            (Json::Null, Json::Null) => true,
            (Json::Bool(a), Json::Bool(b)) => a == b,
            (Json::NumberBits(a), Json::NumberBits(b)) => a == b,
            (Json::String(a), Json::String(b)) => a == b,
            (Json::Array(a), Json::Array(b)) => {
                a.len() == b.len() && a.iter().zip(b).all(|(a, b)| reference(a, b))
            }
            (Json::Object(a), Json::Object(b)) => {
                a.len() == b.len()
                    && a.iter()
                        .zip(b)
                        .all(|((ak, av), (bk, bv))| ak == bk && reference(av, bv))
            }
            _ => false,
        }
    }
    let mut values = vec![
        Json::Null,
        Json::Bool(false),
        Json::Bool(true),
        Json::String("".into()),
        Json::String("value".into()),
        Json::NumberBits(0.0f64.to_bits()),
        Json::NumberBits((-0.0f64).to_bits()),
        Json::NumberBits(0x7ff8_0000_0000_0001),
        Json::NumberBits(0x7ff8_0000_0000_0002),
        Json::Array(vec![].into()),
        Json::Object(vec![].into()),
    ];
    for _ in 0..2 {
        let previous = values.clone();
        for value in previous {
            values.push(Json::Array(vec![value.clone()].into()));
            values.push(Json::Object(
                vec![("a".into(), value.clone()), ("b".into(), Json::Null)].into(),
            ));
            values.push(Json::Object(
                vec![("b".into(), Json::Null), ("a".into(), value)].into(),
            ));
        }
    }
    let index = MembershipIndex::default();
    for a in &values {
        for b in &values {
            let expected = reference(a, b);
            assert_eq!(a == b, expected);
            if expected {
                assert_eq!(
                    index.fingerprint(&InterpValue::Json(a.clone())),
                    index.fingerprint(&InterpValue::Json(b.clone()))
                );
            }
        }
    }
}
