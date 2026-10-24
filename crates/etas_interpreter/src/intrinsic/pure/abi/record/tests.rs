use super::*;
use crate::testing::allocation::measure;

thread_local! {
    static LOOKUPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

pub(super) fn record_lookup() {
    LOOKUPS.set(LOOKUPS.get() + 1);
}

#[test]
fn record_abi_permutations_use_bounded_lookups_and_no_temporary_storage() {
    for count in [1000, 2000, 4000] {
        let fields: Vec<_> = (0..count)
            .map(|n| FieldType {
                name: format!("field_{n}"),
                ty: TypeId(0),
            })
            .collect();
        let (layout, cold) = measure(|| RecordAbiLayout::build(&fields).unwrap());
        for rotation in [0, 1, count / 2, count - 1] {
            for reverse in [false, true] {
                let mut values: Vec<_> = fields
                    .iter()
                    .enumerate()
                    .map(|(n, field)| (field.name.clone(), Box::new(n)))
                    .collect();
                let buffer = values.as_ptr();
                let pointers: Vec<_> = values
                    .iter()
                    .map(|(_, value)| &**value as *const usize)
                    .collect();
                values.rotate_left(rotation);
                if reverse {
                    values.reverse();
                }
                LOOKUPS.set(0);
                let (result, cost) = measure(|| layout.reorder(values, TypeId(1)).unwrap());
                let lookups = LOOKUPS.get();
                assert_eq!(cost.count, 0, "{cost:?}");
                assert_eq!(result.as_ptr(), buffer);
                assert!(lookups <= 2 * count, "{lookups}");
                for (n, (name, value)) in result.iter().enumerate() {
                    assert_eq!(name, &fields[n].name);
                    assert_eq!(**value, n);
                    assert_eq!(&**value as *const usize, pointers[n]);
                }
            }
        }
        eprintln!(
            "record ABI layout n={count}: cold={cold:?}; warm permutations zero allocation, at most 2n lookups"
        );
    }
}

#[test]
fn record_abi_rejects_duplicates_unknown_fields_and_wrong_arity() {
    let fields: Vec<_> = ["b", "a", "c"]
        .into_iter()
        .map(|name| FieldType {
            name: name.into(),
            ty: TypeId(0),
        })
        .collect();
    let layout = RecordAbiLayout::build(&fields).unwrap();
    for names in [
        vec!["a", "b"],
        vec!["a", "a", "b"],
        vec!["a", "b", "b"],
        vec!["c", "c", "c"],
        vec!["a", "b", "unknown"],
        vec!["a", "b", "c", "c"],
    ] {
        LOOKUPS.set(0);
        let result = layout.reorder(
            names.into_iter().map(|s| (s.into(), ())).collect(),
            TypeId(1),
        );
        assert!(matches!(
            result,
            Err(AdapterError::TypeMismatch {
                expected: TypeId(1),
                ..
            })
        ));
        assert!(LOOKUPS.get() <= 2 * fields.len());
    }
    let duplicate = [fields[0].clone(), fields[0].clone()];
    assert!(
        RecordAbiLayout::build(&duplicate)
            .unwrap_err()
            .contains("duplicate")
    );

    for mut encoded in 0..64 {
        let names = std::array::from_fn::<_, 3, _>(|_| {
            let name = ["a", "b", "c", "unknown"][encoded % 4];
            encoded /= 4;
            name
        });
        let valid = fields
            .iter()
            .all(|field| names.contains(&field.name.as_str()));
        LOOKUPS.set(0);
        let result = layout.reorder(
            names
                .iter()
                .enumerate()
                .map(|(n, name)| (name.to_string(), n))
                .collect(),
            TypeId(1),
        );
        assert_eq!(result.is_ok(), valid, "{names:?}");
        assert!(LOOKUPS.get() <= 2 * fields.len());
        if let Ok(result) = result {
            for ((name, position), expected) in result.iter().zip(&fields) {
                assert_eq!(name, &expected.name);
                assert_eq!(name, names[*position]);
            }
        }
    }
}
