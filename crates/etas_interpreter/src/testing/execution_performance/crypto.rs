use super::*;

#[test]
fn checked_source_sha256_returns_a_nominal_digest() {
    let program = PreparedExecution::new(
        "module app.main; import std.crypto; import std.crypto.Digest; flow main(data: bytes) -> Digest { return crypto.sha256(data); }",
    );
    let (value, _, _) = program.run(&[InterpValue::Bytes(b"abc".to_vec().into())]);
    let InterpValue::Nominal { ty, value } = value else {
        panic!("source sha256 returned a non-nominal digest");
    };
    assert!(
        matches!(program.checked.type_store.get(ty), Some(etas_types::Type::Nominal(nominal)) if nominal.name == "std.crypto.Digest")
    );
    let InterpValue::Bytes(bytes) = &*value else {
        panic!("digest representation")
    };
    assert_eq!(
        &bytes[..],
        &[
            186, 120, 22, 191, 143, 1, 207, 234, 65, 65, 64, 222, 93, 174, 34, 35, 176, 3, 97, 163,
            150, 23, 122, 156, 180, 16, 255, 97, 242, 0, 21, 173
        ]
    );
}

#[test]
fn checked_source_crypto_retains_digest_identity_and_borrows_live_inputs() {
    let program = PreparedExecution::new(
        r#"
module app.main;
import std.crypto;
flow main(data: bytes) -> bool {
    let first = crypto.sha256(data);
    let second = crypto.sha256(data);
    return first == second && crypto.constant_time_eq(data, data);
}
"#,
    );
    let mut baseline = None;
    for size in [1000, 2000, 4000] {
        let input = InterpValue::Bytes(vec![7; size * 1024].into());
        let (value, cost, elapsed) = program.run(&[input]);
        assert_eq!(value, InterpValue::Bool(true));
        eprintln!(
            "checked source crypto bytes={}: {cost:?}, {elapsed:?}",
            size * 1024
        );
        if let Some(expected) = baseline {
            assert_eq!((cost.count, cost.bytes), expected);
        }
        baseline = Some((cost.count, cost.bytes));
    }
}
