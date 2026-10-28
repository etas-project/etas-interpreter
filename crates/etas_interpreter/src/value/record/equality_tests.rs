use super::*;
use crate::{
    orchestration::ValueSnapshot,
    value::{
        MapValue, SetValue,
        membership::{MembershipIndex, MembershipValue},
    },
};

fn row(ty: u32, names: &[&str]) -> InterpValue {
    InterpValue::Nominal {
        ty: etas_types::TypeId(ty),
        value: InterpValue::Record(RecordValue::new(
            names
                .iter()
                .map(|name| {
                    (
                        (*name).into(),
                        InterpValue::String(format!("value:{name}").into()),
                    )
                })
                .collect(),
        ))
        .into(),
    }
}

#[test]
fn record_equality_by_field_name_preserves_nominal_identity_and_values() {
    let expected = row(1, &["a", "b", "c"]);
    for names in [
        ["a", "b", "c"],
        ["a", "c", "b"],
        ["b", "a", "c"],
        ["b", "c", "a"],
        ["c", "a", "b"],
        ["c", "b", "a"],
    ] {
        let actual = row(1, &names);
        assert_eq!(expected, actual, "{names:?}");
        assert_eq!(actual, expected, "{names:?}");
        assert_ne!(actual, row(2, &names));
    }
    assert_ne!(expected, row(1, &["a", "b", "d"]));
    assert_ne!(expected, row(1, &["a", "b"]));
    let InterpValue::Nominal { value, .. } = &expected else {
        unreachable!()
    };
    assert_ne!(expected, **value);
    let mut changed = row(1, &["c", "b", "a"]);
    let InterpValue::Nominal { value, .. } = &mut changed else {
        unreachable!()
    };
    let InterpValue::Record(fields) = value.make_mut() else {
        unreachable!()
    };
    *fields.field_mut("a").unwrap() = InterpValue::String("different".into());
    assert_ne!(expected, changed);
}

#[test]
fn record_equality_by_field_name_agrees_with_runtime_and_snapshot_membership() {
    let a = row(1, &["status", "state"]);
    let b = row(1, &["state", "status"]);
    let index = MembershipIndex::default();
    assert_eq!(index.fingerprint(&a), index.fingerprint(&b));
    let mut set = SetValue::new(vec![a.clone()]);
    assert!(set.contains(&b));
    assert!(!set.insert(b.clone()));
    assert_eq!(set.borrow().len(), 1);
    assert!(SetValue::from_unique(vec![a.clone(), b.clone()]).is_err());
    let mut map = MapValue::new(vec![(a.clone(), InterpValue::i32(1))]);
    assert_eq!(map.get(&b), Some(InterpValue::i32(1)));
    map.insert(b.clone(), InterpValue::i32(2));
    assert_eq!(map.borrow().len(), 1);
    assert_eq!(map.get(&a), Some(InterpValue::i32(2)));

    let sa = ValueSnapshot::capture(&a).unwrap();
    let sb = ValueSnapshot::capture(&b).unwrap();
    // Artifact definition equality must still detect reordered storage.
    assert_ne!(sa, sb);
    assert!(sa.member_eq(&sb));
    assert_eq!(index.fingerprint(&sa), index.fingerprint(&sb));
    assert!(MembershipIndex::require_unique(&[sa, sb]).is_err());
}

#[test]
fn record_equality_by_field_name_tracks_cow_layout_changes_without_reordering_storage() {
    let mut a = RecordValue::new(vec![
        ("b".into(), InterpValue::i32(2)),
        ("a".into(), InterpValue::i32(1)),
    ]);
    let b = RecordValue::new(vec![
        ("a".into(), InterpValue::i32(1)),
        ("b".into(), InterpValue::i32(2)),
    ]);
    assert_eq!(a, b);
    let alias = a.clone();
    a.borrow_mut().reverse();
    assert_eq!(a, b);
    assert_eq!(alias.borrow()[0].0, "b");
    assert_eq!(a.borrow()[0].0, "a");
    a.borrow_mut()[0].0 = "renamed".into();
    assert_ne!(a, b);
    assert_eq!(alias, b);
}

#[test]
fn record_equality_by_field_name_is_stack_safe_for_deep_nested_records() {
    let build = |reverse: bool, leaf: bool| {
        (0..30_000).fold(InterpValue::Bool(leaf), |child, _| {
            let mut fields = vec![("next".into(), child), ("tag".into(), InterpValue::i32(7))];
            if reverse {
                fields.reverse();
            }
            InterpValue::Record(RecordValue::new(fields))
        })
    };
    let a = build(false, true);
    let b = build(true, true);
    let c = build(true, false);
    assert!(a == b);
    assert!(a != c);
    let index = MembershipIndex::default();
    assert_eq!(index.fingerprint(&a), index.fingerprint(&b));
}
