use super::*;
use crate::{
    orchestration::ValueSnapshot,
    testing::allocation::measure,
    value::{
        InterpValue, MapValue, SetValue,
        membership::{MembershipIndex, MembershipValue},
    },
};
use etas_builtin::text::input::TextInput;

fn cold(prefix: &str, suffix: &str) -> StringValue {
    let value = StringValue::from(prefix);
    let alias = value.clone();
    let value = value.concat(suffix.into()).unwrap();
    drop(alias);
    value
}

#[test]
fn cold_text_comparisons_do_not_materialize_input_payloads() {
    let mut costs = Vec::new();
    for count in [1000, 2000, 4000] {
        let prefix = format!("start{}", "中😀e\u{301}".repeat(count));
        let expected = format!("{prefix}end");
        for operation in 0..11 {
            let left = cold(&prefix, "end");
            let (a, b) = expected.split_at(8);
            let right = cold(a, b);
            let different = cold(&prefix, "enz");
            let (correct, cost) = measure(|| match operation {
                0 => left == right,
                1 => left == expected.as_str(),
                2 => left == expected,
                3 => expected == left,
                4 => left.cmp(&right).is_eq(),
                5 => left.cmp(&different).is_lt(),
                6 => left != different,
                7 => InterpValue::String(left.clone()) == InterpValue::String(right.clone()),
                8 => ValueSnapshot::String(left.clone())
                    .member_eq(&ValueSnapshot::String(right.clone())),
                9 => crate::value::ordered::default_order(
                    &InterpValue::String(left.clone()),
                    &InterpValue::String(right.clone()),
                )
                .unwrap()
                .is_eq(),
                10 => crate::value::ordered::snapshot_order(
                    &ValueSnapshot::String(left.clone()),
                    &ValueSnapshot::String(right.clone()),
                )
                .unwrap()
                .is_eq(),
                _ => unreachable!(),
            });
            assert!(correct, "operation={operation} n={count}");
            let cached = [left, right, different]
                .iter()
                .any(|value| value.contiguous().is_some());
            costs.push((operation, count, cost, cached));
        }
    }
    eprintln!("cold text comparison costs: {costs:?}");
    for (operation, count, cost, cached) in costs {
        assert!(
            !cached,
            "operation={operation} n={count}: retained flat input"
        );
        assert!(
            cost.bytes < 4096,
            "operation={operation} n={count}: {cost:?}"
        );
    }
}

#[test]
fn cold_text_membership_does_not_materialize_key_payloads() {
    let mut costs = Vec::new();
    for count in [1000, 2000, 4000] {
        let prefix = "中😀e\u{301}".repeat(count);
        let expected = format!("{prefix}tail");
        for operation in 0..6 {
            let text = cold(&prefix, "tail");
            let value = InterpValue::String(text.clone());
            let saved = ValueSnapshot::capture(&value).unwrap();
            let flat = InterpValue::String(expected.clone().into());
            let flat_saved = ValueSnapshot::capture(&flat).unwrap();
            let map = MapValue::new(vec![(flat.clone(), InterpValue::i32(7))]);
            assert!(map.contains_key(&flat));
            let set = SetValue::new(vec![flat.clone()]);
            let index = MembershipIndex::default();
            let runtime_hash = index.fingerprint(&flat);
            let snapshot_hash = index.fingerprint(&flat_saved);
            let (correct, cost) = measure(|| match operation {
                0 => map.contains_key(&value),
                1 => set.contains(&value),
                2 => index.fingerprint(&value) == runtime_hash,
                3 => index.fingerprint(&saved) == snapshot_hash,
                4 => SetValue::from_unique(vec![flat.clone(), value.clone()]).is_err(),
                5 => MembershipIndex::require_unique(&[flat_saved.clone(), saved.clone()]).is_err(),
                _ => unreachable!(),
            });
            assert!(correct, "operation={operation} n={count}");
            costs.push((operation, count, cost, text.contiguous().is_some()));
        }
    }
    eprintln!("cold text membership costs: {costs:?}");
    for (operation, count, cost, cached) in costs {
        assert!(
            !cached,
            "operation={operation} n={count}: retained flat key"
        );
        assert!(
            cost.bytes < 4096,
            "operation={operation} n={count}: {cost:?}"
        );
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum HashCall {
    Bytes(Vec<u8>),
    Usize(usize),
    U8(u8),
}

#[derive(Clone, Default)]
struct BoundaryHasher(Vec<HashCall>);

impl Hasher for BoundaryHasher {
    fn finish(&self) -> u64 {
        self.0.len() as u64
    }
    fn write(&mut self, bytes: &[u8]) {
        self.0.push(HashCall::Bytes(bytes.to_vec()));
    }
    fn write_usize(&mut self, value: usize) {
        self.0.push(HashCall::Usize(value));
    }
    fn write_u8(&mut self, value: u8) {
        self.0.push(HashCall::U8(value));
    }
}

fn boundaries(text: &str) -> impl Iterator<Item = usize> + '_ {
    text.char_indices()
        .map(|(index, _)| index)
        .chain(std::iter::once(text.len()))
}

#[test]
fn membership_hash_calls_are_independent_of_utf8_chunk_boundaries() {
    for source in [
        "".to_owned(),
        "\0".into(),
        "中😀e\u{301}".repeat(20),
        format!("{}end", "a".repeat(193)),
        "quote\"\\\n\r".into(),
    ] {
        let flat = StringValue::from(source.clone());
        let mut expected = BoundaryHasher::default();
        flat.hash_membership(&mut expected);
        assert_eq!(expected.0.first(), Some(&HashCall::Usize(source.len())));
        let joined = expected
            .0
            .iter()
            .skip(1)
            .flat_map(|call| match call {
                HashCall::Bytes(bytes) => bytes.as_slice(),
                _ => panic!("one length prefix followed by logical byte blocks"),
            })
            .copied()
            .collect::<Vec<_>>();
        assert_eq!(joined, source.as_bytes());
        for boundary in boundaries(&source) {
            let value = cold(&source[..boundary], &source[boundary..]);
            let mut actual = BoundaryHasher::default();
            value.hash_membership(&mut actual);
            assert_eq!(actual.0, expected.0, "boundary={boundary}");
        }
        // The public Rust Hash contract remains str-compatible, independently
        // of the runtime's private, process-local partition protocol.
        let mut public = BoundaryHasher::default();
        let mut standard = BoundaryHasher::default();
        flat.hash(&mut public);
        source.hash(&mut standard);
        assert_eq!(public.0, standard.0);
    }
}

#[test]
fn borrowed_text_order_matches_standard_lexical_order_for_all_layouts() {
    let corpus = [
        "", "\0", "a", "aa", "a\0", "e\u{301}", "é", "中", "中😀", "😀", "\"\\\n",
    ];
    for a in corpus {
        for b in corpus {
            for left_boundary in boundaries(a) {
                for right_boundary in boundaries(b) {
                    let left = cold(&a[..left_boundary], &a[left_boundary..]);
                    let right = cold(&b[..right_boundary], &b[right_boundary..]);
                    assert_eq!(left.cmp(&right), a.cmp(b));
                    assert_eq!(left == right, a == b);
                    assert_eq!(left == b, a == b);
                    let owned = b.to_owned();
                    assert_eq!(owned == left, b == a);
                }
            }
        }
    }
}

#[test]
fn colliding_cold_text_keys_still_require_exact_value_equality() {
    #[derive(PartialEq)]
    struct Colliding(StringValue);
    impl MembershipValue for Colliding {
        fn hash_partition(&self, _: &mut (impl Hasher + Clone)) {}
    }
    let prefix = "中😀".repeat(1000);
    let values = [Colliding(cold(&prefix, "a")), Colliding(cold(&prefix, "b"))];
    let index = MembershipIndex::require_unique(&values).unwrap();
    for suffix in ["a", "b", "c"] {
        let key = Colliding(cold(&prefix, suffix));
        assert_eq!(
            index.contains(&values, &key, index.fingerprint(&key)),
            suffix != "c"
        );
        assert!(key.0.contiguous().is_none());
    }
    assert!(values.iter().all(|value| value.0.contiguous().is_none()));
}

#[test]
fn deep_cold_comparison_and_fingerprint_only_allocate_cursor_metadata() {
    for count in [1000, 2000, 4000] {
        let piece = format!("{}😀", "x".repeat(1024));
        let mut left = StringValue::default();
        let mut right = StringValue::default();
        for _ in 0..count {
            left = left.clone().concat(piece.clone().into()).unwrap();
            right = right.clone().concat(piece.clone().into()).unwrap();
        }
        let index = MembershipIndex::default();
        let a = InterpValue::String(left.clone());
        let b = InterpValue::String(right.clone());
        let sa = ValueSnapshot::capture(&a).unwrap();
        let sb = ValueSnapshot::capture(&b).unwrap();
        let (equal, comparison) = measure(|| left == right);
        assert!(equal);
        let ((ha, hb), runtime) = measure(|| (index.fingerprint(&a), index.fingerprint(&b)));
        let ((hsa, hsb), snapshot) = measure(|| (index.fingerprint(&sa), index.fingerprint(&sb)));
        assert_eq!(ha, hb);
        assert_eq!(hsa, hsb);
        assert!(left.contiguous().is_none() && right.contiguous().is_none());
        for cost in [comparison, runtime, snapshot] {
            assert!(
                cost.bytes < count * 128 + 4096,
                "cursor costs n={count}: {cost:?}"
            );
            assert_eq!(
                cost.bytes, cost.released_bytes,
                "no retained cursor/cache: {cost:?}"
            );
        }
        eprintln!(
            "deep text reads n={count}: comparison={comparison:?}, runtime={runtime:?}, snapshot={snapshot:?}"
        );
    }
}
