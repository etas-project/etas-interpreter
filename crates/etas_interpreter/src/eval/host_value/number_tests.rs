use super::*;
use crate::value::NumericValue;
use etas_types::{FieldType, NominalTypeRef, RecordType, TypeInterner};

fn json_number(value: f64) -> HostValue {
    HostValue::Json(etas_host::HostJsonValue::Number(value))
}

#[test]
fn json_numbers_decode_using_the_checked_float_type_without_integer_classification() {
    let mut types = TypeInterner::new();
    for primitive in [PrimitiveType::F32, PrimitiveType::F64] {
        let ty = types.primitive(primitive);
        let mut values = vec![90.0, 90.5, -90.0, 0.0, -0.0];
        if primitive == PrimitiveType::F64 {
            values.extend([1e300, -1e300, f64::MAX, f64::MIN_POSITIVE]);
        } else {
            values.extend([f64::from(f32::MAX), f64::from(f32::from_bits(1))]);
        }
        for value in values {
            assert_eq!(
                host_to_typed_interp_value(json_number(value), ty, types.store()),
                Ok(InterpValue::Number(
                    NumericValue::from_float(value, primitive).unwrap()
                )),
                "{primitive:?}: {value}"
            );
        }
        // A typed Host integer is not a JSON number or an implicit language cast.
        assert!(host_to_typed_interp_value(HostValue::Int(90), ty, types.store()).is_err());
    }
}

#[test]
fn json_number_decode_rejects_nonfinite_narrowing_and_fractional_or_overflowing_integers() {
    let mut types = TypeInterner::new();
    for primitive in [PrimitiveType::F32, PrimitiveType::F64] {
        let ty = types.primitive(primitive);
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(host_to_typed_interp_value(json_number(value), ty, types.store()).is_err());
        }
    }
    let f32_ty = types.primitive(PrimitiveType::F32);
    for value in [1e300, -1e300, f64::MIN_POSITIVE, -f64::MIN_POSITIVE] {
        assert!(host_to_typed_interp_value(json_number(value), f32_ty, types.store()).is_err());
    }
    for (primitive, accepted, rejected) in [
        (PrimitiveType::I8, -128.0, 128.0),
        (PrimitiveType::U8, 255.0, 256.0),
        (PrimitiveType::I128, -2.0_f64.powi(127), 2.0_f64.powi(127)),
        (PrimitiveType::U128, 2.0_f64.powi(127), 2.0_f64.powi(128)),
    ] {
        let ty = types.primitive(primitive);
        assert!(host_to_typed_interp_value(json_number(accepted), ty, types.store()).is_ok());
        for value in [90.5, rejected, f64::NAN, f64::INFINITY] {
            assert!(host_to_typed_interp_value(json_number(value), ty, types.store()).is_err());
        }
    }
    let unsigned = types.primitive(PrimitiveType::U128);
    assert!(host_to_typed_interp_value(json_number(-1.0), unsigned, types.store()).is_err());
}

#[test]
fn nested_json_numeric_decode_preserves_nominal_identity_and_target_types() {
    let mut types = TypeInterner::new();
    let float = types.primitive(PrimitiveType::F64);
    let int = types.primitive(PrimitiveType::I32);
    let array = types.intern(Type::Array(float));
    let tuple = types.intern(Type::Tuple(vec![int, float]));
    let record = types.intern(Type::Record(RecordType {
        fields: vec![
            FieldType {
                name: "values".into(),
                ty: array,
            },
            FieldType {
                name: "pair".into(),
                ty: tuple,
            },
        ],
    }));
    let nominal = types.intern(Type::Nominal(NominalTypeRef {
        read_only_erased_view: false,
        name: "app.Measurements".into(),
        params: vec![],
        representation: Some(record),
    }));
    use etas_host::HostJsonValue as J;
    let value = HostValue::Json(J::Object(vec![
        (
            "values".into(),
            J::Array(vec![J::Number(-0.0), J::Number(1e300)]),
        ),
        (
            "pair".into(),
            J::Array(vec![J::Number(90.0), J::Number(90.0)]),
        ),
    ]));
    let decoded = host_to_typed_interp_value(value, nominal, types.store()).unwrap();
    let InterpValue::Nominal { ty, value } = decoded else {
        panic!("nominal")
    };
    assert_eq!(ty, nominal);
    let InterpValue::Record(record) = value.as_ref() else {
        panic!("record")
    };
    assert_eq!(
        record.get("values"),
        Some(InterpValue::Array(
            vec![
                InterpValue::Number(NumericValue::F64((-0.0_f64).to_bits())),
                InterpValue::Number(NumericValue::F64(1e300_f64.to_bits())),
            ]
            .into()
        ))
    );
    assert_eq!(
        record.get("pair"),
        Some(InterpValue::Tuple(
            vec![
                InterpValue::i32(90),
                InterpValue::Number(NumericValue::F64(90.0_f64.to_bits())),
            ]
            .into()
        ))
    );
}
