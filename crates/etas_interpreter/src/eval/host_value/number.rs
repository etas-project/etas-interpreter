use crate::value::NumericValue;
use etas_types::PrimitiveType;

pub(super) fn decode_json(value: f64, primitive: PrimitiveType) -> Option<NumericValue> {
    if !value.is_finite() {
        return None;
    }
    match primitive {
        PrimitiveType::F32 | PrimitiveType::F64 => decode_float(value, primitive),
        PrimitiveType::I8
        | PrimitiveType::I16
        | PrimitiveType::I32
        | PrimitiveType::I64
        | PrimitiveType::I128
        | PrimitiveType::ISize
            if value.fract() == 0.0 && value >= i128::MIN as f64 && value < -(i128::MIN as f64) =>
        {
            NumericValue::from_signed(value as i128, primitive)
        }
        PrimitiveType::U8
        | PrimitiveType::U16
        | PrimitiveType::U32
        | PrimitiveType::U64
        | PrimitiveType::U128
        | PrimitiveType::USize
            if value.fract() == 0.0 && value >= 0.0 && value < 2.0_f64.powi(128) =>
        {
            NumericValue::from_unsigned(value as u128, primitive)
        }
        _ => None,
    }
}

pub(in crate::eval) fn decode_float(value: f64, primitive: PrimitiveType) -> Option<NumericValue> {
    if !value.is_finite() {
        return None;
    }
    if primitive == PrimitiveType::F32 {
        let narrowed = value as f32;
        if !narrowed.is_finite() || (narrowed == 0.0 && value != 0.0) {
            return None;
        }
    }
    NumericValue::from_float(value, primitive)
}
