use std::cmp::Ordering;

use etas_types::PrimitiveType;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NumericValue {
    I8(i8),
    I16(i16),
    I32(i32),
    I64(i64),
    I128(i128),
    ISize(i64),
    U8(u8),
    U16(u16),
    U32(u32),
    U64(u64),
    U128(u128),
    USize(u64),
    F32(u32),
    F64(u64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NumericError {
    TypeMismatch,
    Overflow,
    DivisionByZero,
    InvalidLiteral,
}

impl NumericValue {
    pub fn i32(value: i32) -> Self {
        Self::I32(value)
    }

    pub fn usize(value: usize) -> Self {
        Self::USize(value as u64)
    }

    pub fn primitive(self) -> PrimitiveType {
        match self {
            Self::I8(_) => PrimitiveType::I8,
            Self::I16(_) => PrimitiveType::I16,
            Self::I32(_) => PrimitiveType::I32,
            Self::I64(_) => PrimitiveType::I64,
            Self::I128(_) => PrimitiveType::I128,
            Self::ISize(_) => PrimitiveType::ISize,
            Self::U8(_) => PrimitiveType::U8,
            Self::U16(_) => PrimitiveType::U16,
            Self::U32(_) => PrimitiveType::U32,
            Self::U64(_) => PrimitiveType::U64,
            Self::U128(_) => PrimitiveType::U128,
            Self::USize(_) => PrimitiveType::USize,
            Self::F32(_) => PrimitiveType::F32,
            Self::F64(_) => PrimitiveType::F64,
        }
    }

    pub fn parse_integer(text: &str, primitive: PrimitiveType) -> Result<Self, NumericError> {
        let (negative, magnitude) = if let Some(magnitude) = text.strip_prefix('-') {
            (true, magnitude)
        } else if let Some(magnitude) = text.strip_prefix('+') {
            (false, magnitude)
        } else {
            (false, text)
        };
        Self::parse_integer_with_sign(magnitude, primitive, negative)
    }

    pub(crate) fn parse_negated_integer(
        text: &str,
        primitive: PrimitiveType,
    ) -> Result<Self, NumericError> {
        Self::parse_integer_with_sign(text, primitive, true)
    }

    pub fn parse_float(text: &str, primitive: PrimitiveType) -> Result<Self, NumericError> {
        let text = text.replace('_', "");
        match primitive {
            PrimitiveType::F32 => text.parse::<f32>().map(|value| Self::F32(value.to_bits())),
            PrimitiveType::F64 => text.parse::<f64>().map(|value| Self::F64(value.to_bits())),
            _ => return Err(NumericError::TypeMismatch),
        }
        .map_err(|_| NumericError::InvalidLiteral)
    }

    pub(crate) fn parse_negated_float(
        text: &str,
        primitive: PrimitiveType,
    ) -> Result<Self, NumericError> {
        Self::parse_float(&format!("-{text}"), primitive)
    }

    fn parse_integer_with_sign(
        text: &str,
        primitive: PrimitiveType,
        negative: bool,
    ) -> Result<Self, NumericError> {
        let text = text.replace('_', "");
        let (radix, digits) = if let Some(digits) =
            text.strip_prefix("0x").or_else(|| text.strip_prefix("0X"))
        {
            (16, digits)
        } else if let Some(digits) = text.strip_prefix("0o").or_else(|| text.strip_prefix("0O")) {
            (8, digits)
        } else if let Some(digits) = text.strip_prefix("0b").or_else(|| text.strip_prefix("0B")) {
            (2, digits)
        } else {
            (10, text.as_str())
        };
        let magnitude =
            u128::from_str_radix(digits, radix).map_err(|_| NumericError::InvalidLiteral)?;
        if negative {
            return match primitive {
                PrimitiveType::I128 if magnitude == 1_u128 << 127 => Ok(Self::I128(i128::MIN)),
                PrimitiveType::I8
                | PrimitiveType::I16
                | PrimitiveType::I32
                | PrimitiveType::I64
                | PrimitiveType::I128
                | PrimitiveType::ISize => {
                    let magnitude =
                        i128::try_from(magnitude).map_err(|_| NumericError::Overflow)?;
                    Self::from_signed(-magnitude, primitive).ok_or(NumericError::Overflow)
                }
                PrimitiveType::U8
                | PrimitiveType::U16
                | PrimitiveType::U32
                | PrimitiveType::U64
                | PrimitiveType::U128
                | PrimitiveType::USize => Err(NumericError::Overflow),
                _ => Err(NumericError::TypeMismatch),
            };
        }
        match primitive {
            PrimitiveType::I8
            | PrimitiveType::I16
            | PrimitiveType::I32
            | PrimitiveType::I64
            | PrimitiveType::I128
            | PrimitiveType::ISize => {
                let value = i128::try_from(magnitude).map_err(|_| NumericError::Overflow)?;
                Self::from_signed(value, primitive).ok_or(NumericError::Overflow)
            }
            PrimitiveType::U8
            | PrimitiveType::U16
            | PrimitiveType::U32
            | PrimitiveType::U64
            | PrimitiveType::U128
            | PrimitiveType::USize => {
                Self::from_unsigned(magnitude, primitive).ok_or(NumericError::Overflow)
            }
            _ => Err(NumericError::TypeMismatch),
        }
    }

    pub fn from_signed(value: i128, primitive: PrimitiveType) -> Option<Self> {
        Some(match primitive {
            PrimitiveType::I8 => Self::I8(i8::try_from(value).ok()?),
            PrimitiveType::I16 => Self::I16(i16::try_from(value).ok()?),
            PrimitiveType::I32 => Self::I32(i32::try_from(value).ok()?),
            PrimitiveType::I64 => Self::I64(i64::try_from(value).ok()?),
            PrimitiveType::I128 => Self::I128(value),
            PrimitiveType::ISize => Self::ISize(i64::try_from(value).ok()?),
            _ => return None,
        })
    }

    pub fn from_unsigned(value: u128, primitive: PrimitiveType) -> Option<Self> {
        Some(match primitive {
            PrimitiveType::U8 => Self::U8(u8::try_from(value).ok()?),
            PrimitiveType::U16 => Self::U16(u16::try_from(value).ok()?),
            PrimitiveType::U32 => Self::U32(u32::try_from(value).ok()?),
            PrimitiveType::U64 => Self::U64(u64::try_from(value).ok()?),
            PrimitiveType::U128 => Self::U128(value),
            PrimitiveType::USize => Self::USize(u64::try_from(value).ok()?),
            _ => return None,
        })
    }

    pub fn from_float(value: f64, primitive: PrimitiveType) -> Option<Self> {
        match primitive {
            PrimitiveType::F32 => Some(Self::F32((value as f32).to_bits())),
            PrimitiveType::F64 => Some(Self::F64(value.to_bits())),
            _ => None,
        }
    }

    pub fn as_i128(self) -> Option<i128> {
        match self {
            Self::I8(value) => Some(i128::from(value)),
            Self::I16(value) => Some(i128::from(value)),
            Self::I32(value) => Some(i128::from(value)),
            Self::I64(value) | Self::ISize(value) => Some(i128::from(value)),
            Self::I128(value) => Some(value),
            _ => None,
        }
    }

    pub fn as_u128(self) -> Option<u128> {
        match self {
            Self::U8(value) => Some(u128::from(value)),
            Self::U16(value) => Some(u128::from(value)),
            Self::U32(value) => Some(u128::from(value)),
            Self::U64(value) | Self::USize(value) => Some(u128::from(value)),
            Self::U128(value) => Some(value),
            _ => None,
        }
    }

    pub fn as_f64(self) -> Option<f64> {
        match self {
            Self::F32(bits) => Some(f64::from(f32::from_bits(bits))),
            Self::F64(bits) => Some(f64::from_bits(bits)),
            _ => None,
        }
    }

    pub fn as_usize(self) -> Option<usize> {
        self.as_u128()
            .and_then(|value| usize::try_from(value).ok())
            .or_else(|| self.as_i128().and_then(|value| usize::try_from(value).ok()))
    }

    pub fn as_u32(self) -> Option<u32> {
        self.as_u128()
            .and_then(|value| u32::try_from(value).ok())
            .or_else(|| self.as_i128().and_then(|value| u32::try_from(value).ok()))
    }

    pub fn as_u64(self) -> Option<u64> {
        self.as_u128()
            .and_then(|value| u64::try_from(value).ok())
            .or_else(|| self.as_i128().and_then(|value| u64::try_from(value).ok()))
    }

    pub fn as_i64(self) -> Option<i64> {
        self.as_i128()
            .and_then(|value| i64::try_from(value).ok())
            .or_else(|| self.as_u128().and_then(|value| i64::try_from(value).ok()))
    }

    pub fn checked_neg(self) -> Result<Self, NumericError> {
        macro_rules! signed {
            ($variant:ident, $value:expr) => {
                $value
                    .checked_neg()
                    .map(Self::$variant)
                    .ok_or(NumericError::Overflow)
            };
        }
        match self {
            Self::I8(value) => signed!(I8, value),
            Self::I16(value) => signed!(I16, value),
            Self::I32(value) => signed!(I32, value),
            Self::I64(value) => signed!(I64, value),
            Self::I128(value) => signed!(I128, value),
            Self::ISize(value) => signed!(ISize, value),
            Self::F32(bits) => Ok(Self::F32((-f32::from_bits(bits)).to_bits())),
            Self::F64(bits) => Ok(Self::F64((-f64::from_bits(bits)).to_bits())),
            _ => Err(NumericError::TypeMismatch),
        }
    }

    pub fn one_same(self) -> Result<Self, NumericError> {
        Ok(match self {
            Self::I8(_) => Self::I8(1),
            Self::I16(_) => Self::I16(1),
            Self::I32(_) => Self::I32(1),
            Self::I64(_) => Self::I64(1),
            Self::I128(_) => Self::I128(1),
            Self::ISize(_) => Self::ISize(1),
            Self::U8(_) => Self::U8(1),
            Self::U16(_) => Self::U16(1),
            Self::U32(_) => Self::U32(1),
            Self::U64(_) => Self::U64(1),
            Self::U128(_) => Self::U128(1),
            Self::USize(_) => Self::USize(1),
            Self::F32(_) | Self::F64(_) => return Err(NumericError::TypeMismatch),
        })
    }

    pub fn checked_add(self, rhs: Self) -> Result<Self, NumericError> {
        self.checked_binary(rhs, IntegerOp::Add)
    }

    pub fn checked_sub(self, rhs: Self) -> Result<Self, NumericError> {
        self.checked_binary(rhs, IntegerOp::Sub)
    }

    pub fn checked_mul(self, rhs: Self) -> Result<Self, NumericError> {
        self.checked_binary(rhs, IntegerOp::Mul)
    }

    pub fn checked_div(self, rhs: Self) -> Result<Self, NumericError> {
        self.checked_binary(rhs, IntegerOp::Div)
    }

    pub fn checked_rem(self, rhs: Self) -> Result<Self, NumericError> {
        self.checked_binary(rhs, IntegerOp::Rem)
    }

    pub fn partial_cmp_same(self, rhs: Self) -> Result<Option<Ordering>, NumericError> {
        macro_rules! cmp {
            ($lhs:expr, $rhs:expr) => {
                Ok($lhs.partial_cmp(&$rhs))
            };
        }
        match (self, rhs) {
            (Self::I8(lhs), Self::I8(rhs)) => cmp!(lhs, rhs),
            (Self::I16(lhs), Self::I16(rhs)) => cmp!(lhs, rhs),
            (Self::I32(lhs), Self::I32(rhs)) => cmp!(lhs, rhs),
            (Self::I64(lhs), Self::I64(rhs)) => cmp!(lhs, rhs),
            (Self::I128(lhs), Self::I128(rhs)) => cmp!(lhs, rhs),
            (Self::ISize(lhs), Self::ISize(rhs)) => cmp!(lhs, rhs),
            (Self::U8(lhs), Self::U8(rhs)) => cmp!(lhs, rhs),
            (Self::U16(lhs), Self::U16(rhs)) => cmp!(lhs, rhs),
            (Self::U32(lhs), Self::U32(rhs)) => cmp!(lhs, rhs),
            (Self::U64(lhs), Self::U64(rhs)) => cmp!(lhs, rhs),
            (Self::U128(lhs), Self::U128(rhs)) => cmp!(lhs, rhs),
            (Self::USize(lhs), Self::USize(rhs)) => cmp!(lhs, rhs),
            (Self::F32(lhs), Self::F32(rhs)) => cmp!(f32::from_bits(lhs), f32::from_bits(rhs)),
            (Self::F64(lhs), Self::F64(rhs)) => cmp!(f64::from_bits(lhs), f64::from_bits(rhs)),
            _ => Err(NumericError::TypeMismatch),
        }
    }

    pub fn display_value(self) -> String {
        match self {
            Self::I8(value) => value.to_string(),
            Self::I16(value) => value.to_string(),
            Self::I32(value) => value.to_string(),
            Self::I64(value) => value.to_string(),
            Self::I128(value) => value.to_string(),
            Self::ISize(value) => value.to_string(),
            Self::U8(value) => value.to_string(),
            Self::U16(value) => value.to_string(),
            Self::U32(value) => value.to_string(),
            Self::U64(value) => value.to_string(),
            Self::U128(value) => value.to_string(),
            Self::USize(value) => value.to_string(),
            Self::F32(bits) => f32::from_bits(bits).to_string(),
            Self::F64(bits) => f64::from_bits(bits).to_string(),
        }
    }

    fn checked_binary(self, rhs: Self, op: IntegerOp) -> Result<Self, NumericError> {
        macro_rules! integer {
            ($variant:ident, $lhs:expr, $rhs:expr) => {{
                let value = match op {
                    IntegerOp::Add => $lhs.checked_add($rhs),
                    IntegerOp::Sub => $lhs.checked_sub($rhs),
                    IntegerOp::Mul => $lhs.checked_mul($rhs),
                    IntegerOp::Div if $rhs == 0 => return Err(NumericError::DivisionByZero),
                    IntegerOp::Div => $lhs.checked_div($rhs),
                    IntegerOp::Rem if $rhs == 0 => return Err(NumericError::DivisionByZero),
                    IntegerOp::Rem => $lhs.checked_rem($rhs),
                };
                value.map(Self::$variant).ok_or(NumericError::Overflow)
            }};
        }
        match (self, rhs) {
            (Self::I8(lhs), Self::I8(rhs)) => integer!(I8, lhs, rhs),
            (Self::I16(lhs), Self::I16(rhs)) => integer!(I16, lhs, rhs),
            (Self::I32(lhs), Self::I32(rhs)) => integer!(I32, lhs, rhs),
            (Self::I64(lhs), Self::I64(rhs)) => integer!(I64, lhs, rhs),
            (Self::I128(lhs), Self::I128(rhs)) => integer!(I128, lhs, rhs),
            (Self::ISize(lhs), Self::ISize(rhs)) => integer!(ISize, lhs, rhs),
            (Self::U8(lhs), Self::U8(rhs)) => integer!(U8, lhs, rhs),
            (Self::U16(lhs), Self::U16(rhs)) => integer!(U16, lhs, rhs),
            (Self::U32(lhs), Self::U32(rhs)) => integer!(U32, lhs, rhs),
            (Self::U64(lhs), Self::U64(rhs)) => integer!(U64, lhs, rhs),
            (Self::U128(lhs), Self::U128(rhs)) => integer!(U128, lhs, rhs),
            (Self::USize(lhs), Self::USize(rhs)) => integer!(USize, lhs, rhs),
            (Self::F32(lhs), Self::F32(rhs)) => {
                let (lhs, rhs) = (f32::from_bits(lhs), f32::from_bits(rhs));
                Ok(Self::F32(
                    match op {
                        IntegerOp::Add => lhs + rhs,
                        IntegerOp::Sub => lhs - rhs,
                        IntegerOp::Mul => lhs * rhs,
                        IntegerOp::Div => lhs / rhs,
                        IntegerOp::Rem => lhs % rhs,
                    }
                    .to_bits(),
                ))
            }
            (Self::F64(lhs), Self::F64(rhs)) => {
                let (lhs, rhs) = (f64::from_bits(lhs), f64::from_bits(rhs));
                Ok(Self::F64(
                    match op {
                        IntegerOp::Add => lhs + rhs,
                        IntegerOp::Sub => lhs - rhs,
                        IntegerOp::Mul => lhs * rhs,
                        IntegerOp::Div => lhs / rhs,
                        IntegerOp::Rem => lhs % rhs,
                    }
                    .to_bits(),
                ))
            }
            _ => Err(NumericError::TypeMismatch),
        }
    }
}

#[derive(Clone, Copy)]
enum IntegerOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_numeric_primitive_preserves_its_runtime_width() {
        let values = [
            NumericValue::I8(i8::MIN),
            NumericValue::I16(i16::MIN),
            NumericValue::I32(i32::MIN),
            NumericValue::I64(i64::MIN),
            NumericValue::I128(i128::MIN),
            NumericValue::ISize(i64::MIN),
            NumericValue::U8(u8::MAX),
            NumericValue::U16(u16::MAX),
            NumericValue::U32(u32::MAX),
            NumericValue::U64(u64::MAX),
            NumericValue::U128(u128::MAX),
            NumericValue::USize(u64::MAX),
            NumericValue::F32((-0.0_f32).to_bits()),
            NumericValue::F64((-0.0_f64).to_bits()),
        ];

        let primitives = values.map(NumericValue::primitive);
        assert_eq!(
            primitives,
            [
                PrimitiveType::I8,
                PrimitiveType::I16,
                PrimitiveType::I32,
                PrimitiveType::I64,
                PrimitiveType::I128,
                PrimitiveType::ISize,
                PrimitiveType::U8,
                PrimitiveType::U16,
                PrimitiveType::U32,
                PrimitiveType::U64,
                PrimitiveType::U128,
                PrimitiveType::USize,
                PrimitiveType::F32,
                PrimitiveType::F64,
            ]
        );
    }

    #[test]
    fn checked_integer_arithmetic_reports_overflow_and_zero_divisor() {
        assert_eq!(
            NumericValue::I8(i8::MAX).checked_add(NumericValue::I8(1)),
            Err(NumericError::Overflow)
        );
        assert_eq!(
            NumericValue::U128(u128::MAX).checked_mul(NumericValue::U128(2)),
            Err(NumericError::Overflow)
        );
        assert_eq!(
            NumericValue::I32(1).checked_div(NumericValue::I32(0)),
            Err(NumericError::DivisionByZero)
        );
        assert_eq!(
            NumericValue::U64(1).checked_rem(NumericValue::U64(0)),
            Err(NumericError::DivisionByZero)
        );
        assert_eq!(
            NumericValue::I32(1).checked_add(NumericValue::I64(1)),
            Err(NumericError::TypeMismatch)
        );
    }

    #[test]
    fn negated_literal_parser_accepts_signed_minimum_without_widening() {
        assert_eq!(
            NumericValue::parse_negated_integer("128", PrimitiveType::I8),
            Ok(NumericValue::I8(i8::MIN))
        );
        assert_eq!(
            NumericValue::parse_negated_integer("0x80", PrimitiveType::I8),
            Ok(NumericValue::I8(i8::MIN))
        );
        assert_eq!(
            NumericValue::parse_negated_integer("129", PrimitiveType::I8),
            Err(NumericError::Overflow)
        );
    }

    #[test]
    fn floating_arithmetic_comparison_and_negation_preserve_width() {
        let f32_sum = NumericValue::F32(1.5_f32.to_bits())
            .checked_add(NumericValue::F32(2.25_f32.to_bits()))
            .expect("matching f32 operands");
        assert_eq!(f32_sum, NumericValue::F32(3.75_f32.to_bits()));

        let f64_neg = NumericValue::F64(1.25_f64.to_bits())
            .checked_neg()
            .expect("f64 negation");
        assert_eq!(f64_neg, NumericValue::F64((-1.25_f64).to_bits()));
        assert_eq!(
            NumericValue::F64(f64::NAN.to_bits())
                .partial_cmp_same(NumericValue::F64(0.0_f64.to_bits()))
                .expect("matching f64 operands"),
            None
        );
    }
}
