//! Plain value representation operations. No standard-library algorithms.
use super::{InterpValue as V, NumericValue as N};
use etas_types::value_representation::{ValueLength as L, ValueProjection as P};

pub(crate) fn project(projection: P, value: V) -> Result<V, String> {
    let length = match (projection, value) {
        (P::NumericFamilyBits, V::Number(value)) => {
            return Ok(V::Number(N::U128(value.raw_bits())));
        }
        (P::NumericBits(kind), V::Number(value)) if value.primitive() == kind => {
            return Ok(V::Number(N::U128(value.raw_bits())));
        }
        (P::CharBits, V::String(value)) => {
            let mut chars = value.chars();
            let scalar = chars.next().ok_or("char representation is empty")?;
            if chars.next().is_some() {
                return Err("char representation contains multiple scalars".into());
            }
            return Ok(V::Number(N::U128(scalar as u128)));
        }
        (P::Utf8, V::String(value)) => {
            return Ok(V::Bytes(value.into_string().into_bytes().into()));
        }
        (P::RangeStart, V::Range(value)) => return Ok(*value.start),
        (P::RangeEnd, V::Range(value)) => return Ok(*value.end),
        (P::RangeIncludeStart, V::Range(value)) => {
            return Ok(V::Bool(matches!(
                value.bounds,
                super::RangeBounds::ClosedClosed | super::RangeBounds::ClosedOpen
            )));
        }
        (P::RangeIncludeEnd, V::Range(value)) => {
            return Ok(V::Bool(matches!(
                value.bounds,
                super::RangeBounds::ClosedClosed | super::RangeBounds::OpenClosed
            )));
        }
        (P::Length(L::String), V::String(value)) => value.len(),
        (P::Length(L::Bytes), V::Bytes(value)) => value.len(),
        (P::Length(L::Array), V::Array(value)) => value.borrow().len(),
        (P::Length(L::List), V::List(value)) => value.len(),
        (P::Length(L::Slice), V::Slice(value)) => value.borrow().len(),
        (P::Length(L::Map), V::Map(value)) => value.borrow().len(),
        (P::Length(L::Set), V::Set(value)) => value.borrow().len(),
        _ => return Err("value does not match its checked representation projection".into()),
    };
    Ok(V::usize(length))
}

pub(crate) fn construct(kind: etas_types::PrimitiveType, value: V) -> Result<V, String> {
    use etas_types::PrimitiveType as K;
    match (kind, value) {
        (K::String, V::Array(values)) => {
            let mut text = String::new();
            for value in values.into_values() {
                let V::String(value) = value else {
                    return Err("string scalars require char values".into());
                };
                let mut chars = value.chars();
                let scalar = chars.next().ok_or("string scalar is empty")?;
                if chars.next().is_some() {
                    return Err("string scalar contains multiple characters".into());
                }
                text.push(scalar);
            }
            Ok(V::String(text.into()))
        }
        (K::Bytes, V::Array(values)) => values
            .try_map_values(|value| match value {
                V::Number(N::U8(byte)) => Ok(byte),
                _ => Err("byte data requires u8 values".into()),
            })
            .map(|data| V::Bytes(data.into())),
        (kind, V::Number(N::U128(bits))) => {
            if kind == K::Char {
                let scalar = u32::try_from(bits)
                    .ok()
                    .and_then(char::from_u32)
                    .ok_or("char bits are not a Unicode scalar")?;
                return Ok(V::String(scalar.to_string().into()));
            }
            let value = match kind {
                K::I8 => N::I8(bits as i8),
                K::I16 => N::I16(bits as i16),
                K::I32 => N::I32(bits as i32),
                K::I64 => N::I64(bits as i64),
                K::I128 => N::I128(bits as i128),
                K::ISize => N::ISize(bits as i64),
                K::U8 => N::U8(bits as u8),
                K::U16 => N::U16(bits as u16),
                K::U32 => N::U32(bits as u32),
                K::U64 => N::U64(bits as u64),
                K::U128 => N::U128(bits),
                K::USize => N::USize(bits as u64),
                K::F32 => N::F32(bits as u32),
                K::F64 => N::F64(bits as u64),
                _ => return Err("unsupported scalar representation constructor".into()),
            };
            Ok(V::Number(value))
        }
        _ => Err("primitive constructor input differs from its checked representation".into()),
    }
}
