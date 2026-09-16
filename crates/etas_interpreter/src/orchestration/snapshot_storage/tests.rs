use super::*;
use crate::testing::allocation::measure;

fn chain(depth: usize) -> ValueSnapshot {
    (0..depth).fold(ValueSnapshot::Bool(true), |value, i| {
        if i % 2 == 0 {
            ValueSnapshot::OptionSome(SnapshotBox::new(value))
        } else {
            ValueSnapshot::Array(vec![value].into())
        }
    })
}

#[test]
fn snapshot_aliases_share_aggregate_structure_without_allocating() {
    for count in [1000, 2000, 4000] {
        let value = ValueSnapshot::Array((0..count).map(|_| chain(4)).collect::<Vec<_>>().into());
        let (alias, cost) = measure(|| value.clone());
        eprintln!("snapshot alias width={count}: {cost:?}");
        assert_eq!(cost.count, 0, "snapshot descendants were copied: {cost:?}");
        assert!(value == alias);
    }
}

#[test]
fn dropping_wide_snapshot_does_not_allocate_a_second_child_buffer() {
    for count in [1000, 2000, 4000] {
        let cases = [
            ValueSnapshot::Array(vec![ValueSnapshot::Bool(true); count].into()),
            ValueSnapshot::Map(
                vec![(ValueSnapshot::Bool(true), ValueSnapshot::Bool(false)); count].into(),
            ),
            ValueSnapshot::Record(vec![(String::new(), ValueSnapshot::Bool(true)); count].into()),
        ];
        for (kind, value) in cases.into_iter().enumerate() {
            let value = ValueSnapshot::OptionSome(SnapshotBox::new(value));
            let (_, cost) = measure(|| drop(value));
            eprintln!("snapshot release kind={kind} width={count}: {cost:?}");
            assert_eq!(cost.count, 0, "release copied the child table: {cost:?}");
        }
    }
}

#[test]
fn snapshot_mutation_detaches_only_the_changed_path() {
    let nominal = ValueSnapshot::Nominal {
        ty: etas_types::TypeId(41),
        value: SnapshotBox::new(ValueSnapshot::Tuple(vec![ValueSnapshot::Bool(true)].into())),
    };
    let original = ValueSnapshot::Array(vec![nominal, chain(32)].into());
    let mut changed = original.clone();
    let ValueSnapshot::Array(ref original_fields) = original else {
        panic!("array")
    };
    let ValueSnapshot::Array(ref mut changed_fields) = changed else {
        panic!("array")
    };
    assert_eq!(original_fields.as_ptr(), changed_fields.as_ptr());
    let ValueSnapshot::Nominal { ty, value } = &mut changed_fields[0] else {
        panic!("nominal")
    };
    assert_eq!(*ty, etas_types::TypeId(41));
    let ValueSnapshot::Tuple(fields) = value.as_mut() else {
        panic!("tuple")
    };
    fields[0] = ValueSnapshot::Bool(false);
    assert_ne!(original_fields.as_ptr(), changed_fields.as_ptr());
    let ValueSnapshot::Array(original_tail) = &original_fields[1] else {
        panic!("array")
    };
    let ValueSnapshot::Array(changed_tail) = &changed_fields[1] else {
        panic!("array")
    };
    assert_eq!(original_tail.as_ptr(), changed_tail.as_ptr());
    let ValueSnapshot::Nominal { value, .. } = &original_fields[0] else {
        panic!("nominal")
    };
    let ValueSnapshot::Tuple(fields) = value.as_ref() else {
        panic!("tuple")
    };
    assert!(matches!(fields[0], ValueSnapshot::Bool(true)));
    assert!(original != changed);
}

#[test]
fn snapshot_extraction_moves_unique_slots_and_copies_only_shared_slots() {
    for count in [1000, 2000, 4000] {
        let children: SnapshotChildren<ValueSnapshot> = (0..count).map(|_| chain(4)).collect();
        let pointer = children.as_ptr();
        let (copy, cost) = measure(|| children.clone().into_values());
        assert_eq!(cost.count, 1);
        assert_eq!(cost.bytes, count * std::mem::size_of::<ValueSnapshot>());
        assert_ne!(pointer, copy.as_ptr());
        let (moved, cost) = measure(|| children.into_values());
        assert_eq!(cost.count, 0);
        assert_eq!(moved.as_ptr(), pointer);
        assert!(moved == copy);
    }
}

#[test]
fn malformed_snapshot_alias_does_not_corrupt_the_retained_checkpoint() {
    let original =
        ValueSnapshot::Set(vec![ValueSnapshot::Bool(true), ValueSnapshot::Bool(false)].into());
    let mut corrupted = original.clone();
    let ValueSnapshot::Set(fields) = &mut corrupted else {
        panic!("set")
    };
    fields.push(ValueSnapshot::Bool(true));
    assert!(corrupted.restore().unwrap_err().contains("duplicate"));
    let restored = original.restore().unwrap();
    let crate::value::InterpValue::Set(values) = restored else {
        panic!("set")
    };
    assert_eq!(values.borrow().len(), 2);
}

#[test]
fn deep_shared_snapshot_lifecycle_is_stack_safe_and_reclaims_all_nodes() {
    const WORKER: &str = "ETAS_SHARED_SNAPSHOT_LIFECYCLE_WORKER";
    if std::env::var_os(WORKER).is_none() {
        let thread = std::thread::current();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", thread.name().unwrap(), "--nocapture"])
            .env(WORKER, "1")
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
    let (_, total) = measure(|| {
        let root = chain(30_000);
        let alias = root.clone();
        let branch = SnapshotBox::new(ValueSnapshot::Array(vec![root, alias].into()));
        let retained = branch.clone();
        let weak = Rc::downgrade(&branch.0);
        drop(branch);
        assert!(weak.upgrade().is_some());
        drop(retained);
        assert!(weak.upgrade().is_none());
    });
    assert_eq!(
        total.bytes, total.released_bytes,
        "snapshot graph leaked: {total:?}"
    );
}
