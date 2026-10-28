use super::*;

#[test]
fn deep_shared_record_dag_releases_normally_without_recursive_drop() {
    if std::env::var_os("ETAS_RECORD_DROP_SUBPROCESS").is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "value::record::tests::deep_shared_record_dag_releases_normally_without_recursive_drop", "--nocapture"])
            .env("ETAS_RECORD_DROP_SUBPROCESS", "1")
            .output().unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    for depth in [1000, 2000, 4000, 30_000] {
        let value = (0..depth).fold(InterpValue::Bool(true), |child, _| {
            InterpValue::Record(RecordValue::new(vec![
                ("a".into(), child.clone()),
                ("b".into(), child),
            ]))
        });
        let alias = value.clone();
        drop(value);
        drop(alias);
    }
}

#[test]
fn consuming_unique_record_moves_its_fields_and_keeps_alias_versions() {
    let record = RecordValue::new(vec![(
        "field".into(),
        InterpValue::String("payload".repeat(1024).into()),
    )]);
    let original = record.clone();
    let copied = record.into_values();
    assert_eq!(copied, original.borrow().as_slice());
    let pointer = original.borrow().as_ptr();
    let (moved, cost) = crate::testing::allocation::measure(|| original.into_values());
    assert_eq!(cost.count, 0);
    assert_eq!(pointer, moved.as_ptr());
}
