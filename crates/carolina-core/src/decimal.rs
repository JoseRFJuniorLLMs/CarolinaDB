//! Checked fixed-scale decimals (SPEC-003 §4).
//!
//! `Decimal(precision, scale)` with `1 <= precision <= 38`, `0 <= scale <= precision`,
//! a signed 128-bit coefficient and fixed scale. A value is valid only if
//! `|coefficient| < 10^precision`. Addition/subtraction are exact and checked;
//! multiplication is by integer constants; rescaling must be exact. Wrapping,
//! saturation and rounding are forbidden.

use std::cmp::Ordering;
use std::fmt;

use crate::canon::{CanonValue, Canonical};
use crate::error::{CoreError, CoreResult, ErrorCode};

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Decimal {
    coefficient: i128,
    precision: u8,
    scale: u8,
}

const POW10: [i128; 39] = {
    let mut t = [1i128; 39];
    let mut i = 1;
    while i < 39 {
        t[i] = t[i - 1] * 10;
        i += 1;
    }
    t
};

impl Decimal {
    pub fn new(coefficient: i128, precision: u8, scale: u8) -> CoreResult<Decimal> {
        if precision == 0 || precision > 38 {
            return Err(CoreError::new(
                ErrorCode::InvalidDecimal,
                format!("precision {precision} out of 1..=38"),
            ));
        }
        if scale > precision {
            return Err(CoreError::new(
                ErrorCode::InvalidDecimal,
                format!("scale {scale} > precision {precision}"),
            ));
        }
        let d = Decimal {
            coefficient,
            precision,
            scale,
        };
        d.check_range()?;
        Ok(d)
    }

    fn check_range(&self) -> CoreResult<()> {
        let bound = POW10[self.precision as usize];
        if self.coefficient <= -bound || self.coefficient >= bound {
            return Err(CoreError::new(
                ErrorCode::NumericOverflow,
                format!(
                    "decimal coefficient {} does not fit precision {}",
                    self.coefficient, self.precision
                ),
            ));
        }
        Ok(())
    }

    pub fn coefficient(&self) -> i128 {
        self.coefficient
    }
    pub fn precision(&self) -> u8 {
        self.precision
    }
    pub fn scale(&self) -> u8 {
        self.scale
    }
    pub fn zero(precision: u8, scale: u8) -> CoreResult<Decimal> {
        Decimal::new(0, precision, scale)
    }
    pub fn is_negative(&self) -> bool {
        self.coefficient < 0
    }

    fn same_type(&self, other: &Decimal) -> CoreResult<()> {
        if self.scale != other.scale || self.precision != other.precision {
            return Err(CoreError::new(
                ErrorCode::TypeMismatch,
                format!(
                    "decimal type mismatch: ({},{}) vs ({},{})",
                    self.precision, self.scale, other.precision, other.scale
                ),
            ));
        }
        Ok(())
    }

    pub fn checked_add(&self, other: &Decimal) -> CoreResult<Decimal> {
        self.same_type(other)?;
        let c = self
            .coefficient
            .checked_add(other.coefficient)
            .ok_or_else(|| CoreError::new(ErrorCode::NumericOverflow, "decimal add overflow"))?;
        Decimal::new(c, self.precision, self.scale)
    }

    pub fn checked_sub(&self, other: &Decimal) -> CoreResult<Decimal> {
        self.same_type(other)?;
        let c = self
            .coefficient
            .checked_sub(other.coefficient)
            .ok_or_else(|| CoreError::new(ErrorCode::NumericOverflow, "decimal sub overflow"))?;
        Decimal::new(c, self.precision, self.scale)
    }

    /// Multiply by an integer constant (affine analysis fragment).
    pub fn checked_mul_int(&self, k: i128) -> CoreResult<Decimal> {
        let c = self
            .coefficient
            .checked_mul(k)
            .ok_or_else(|| CoreError::new(ErrorCode::NumericOverflow, "decimal mul overflow"))?;
        Decimal::new(c, self.precision, self.scale)
    }

    pub fn checked_neg(&self) -> CoreResult<Decimal> {
        let c = self
            .coefficient
            .checked_neg()
            .ok_or_else(|| CoreError::new(ErrorCode::NumericOverflow, "neg overflow"))?;
        Decimal::new(c, self.precision, self.scale)
    }

    /// Exact rescale to a new (precision, scale); fails on inexact conversion or overflow.
    pub fn rescale(&self, precision: u8, scale: u8) -> CoreResult<Decimal> {
        if scale >= self.scale {
            let f = POW10[(scale - self.scale) as usize];
            let c = self
                .coefficient
                .checked_mul(f)
                .ok_or_else(|| CoreError::new(ErrorCode::NumericOverflow, "rescale overflow"))?;
            Decimal::new(c, precision, scale)
        } else {
            let f = POW10[(self.scale - scale) as usize];
            if self.coefficient % f != 0 {
                return Err(CoreError::new(
                    ErrorCode::InvalidDecimal,
                    "inexact decimal rescale",
                ));
            }
            Decimal::new(self.coefficient / f, precision, scale)
        }
    }

    /// Exact comparison across scales (used only where the frontend inserted an explicit conversion).
    pub fn cmp_exact(&self, other: &Decimal) -> CoreResult<Ordering> {
        let s = self.scale.max(other.scale);
        let p = 38;
        let a = self.rescale(p, s)?;
        let b = other.rescale(p, s)?;
        Ok(a.coefficient.cmp(&b.coefficient))
    }

    /// Parse `[-]digits[.digits]` into a decimal of the given type; fractional digits must
    /// not exceed `scale` (no rounding); missing fractional digits are zero-padded exactly.
    pub fn parse(text: &str, precision: u8, scale: u8) -> CoreResult<Decimal> {
        let (neg, body) = match text.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, text),
        };
        if body.is_empty() || body.starts_with('+') {
            return Err(CoreError::new(
                ErrorCode::InvalidDecimal,
                format!("invalid decimal literal `{text}`"),
            ));
        }
        let (int_part, frac_part) = match body.split_once('.') {
            Some((i, f)) => (i, f),
            None => (body, ""),
        };
        if int_part.is_empty()
            || !int_part.bytes().all(|b| b.is_ascii_digit())
            || !frac_part.bytes().all(|b| b.is_ascii_digit())
        {
            return Err(CoreError::new(
                ErrorCode::InvalidDecimal,
                format!("invalid decimal literal `{text}`"),
            ));
        }
        if int_part.len() > 1 && int_part.starts_with('0') {
            return Err(CoreError::new(
                ErrorCode::InvalidDecimal,
                "redundant leading zero in decimal literal",
            ));
        }
        if frac_part.len() > scale as usize {
            return Err(CoreError::new(
                ErrorCode::InvalidDecimal,
                format!("literal `{text}` has more than {scale} fractional digits"),
            ));
        }
        let mut digits = String::from(int_part);
        digits.push_str(frac_part);
        for _ in frac_part.len()..scale as usize {
            digits.push('0');
        }
        if digits.len() > 39 {
            return Err(CoreError::new(
                ErrorCode::NumericOverflow,
                "decimal literal too long",
            ));
        }
        let mut c: i128 = 0;
        for b in digits.bytes() {
            c = c
                .checked_mul(10)
                .and_then(|x| x.checked_add((b - b'0') as i128))
                .ok_or_else(|| {
                    CoreError::new(ErrorCode::NumericOverflow, "decimal literal overflow")
                })?;
        }
        if neg {
            c = -c;
        }
        if neg && c == 0 {
            return Err(CoreError::new(
                ErrorCode::InvalidDecimal,
                "signed zero is forbidden",
            ));
        }
        Decimal::new(c, precision, scale)
    }

    /// Human text with exactly `scale` fractional digits (e.g. `-12.30`).
    pub fn to_text(&self) -> String {
        let neg = self.coefficient < 0;
        let mag = self.coefficient.unsigned_abs();
        let s = mag.to_string();
        let scale = self.scale as usize;
        let mut out = String::new();
        if neg {
            out.push('-');
        }
        if scale == 0 {
            out.push_str(&s);
        } else if s.len() <= scale {
            out.push('0');
            out.push('.');
            for _ in s.len()..scale {
                out.push('0');
            }
            out.push_str(&s);
        } else {
            let (i, f) = s.split_at(s.len() - scale);
            out.push_str(i);
            out.push('.');
            out.push_str(f);
        }
        out
    }
}

impl fmt::Debug for Decimal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Decimal({}, p{}, s{})",
            self.to_text(),
            self.precision,
            self.scale
        )
    }
}

impl fmt::Display for Decimal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_text())
    }
}

impl PartialOrd for Decimal {
    /// Ordering is defined only within one (precision, scale) type; cross-type comparison
    /// must use [`Decimal::cmp_exact`] after explicit conversion.
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        if self.scale == other.scale && self.precision == other.precision {
            Some(self.coefficient.cmp(&other.coefficient))
        } else {
            None
        }
    }
}

impl Canonical for Decimal {
    fn to_canon(&self) -> CanonValue {
        CanonValue::obj()
            .fstr("kind", "decimal.v1")
            .f("coefficient", CanonValue::int(self.coefficient))
            .f("precision", CanonValue::u32(self.precision as u32))
            .f("scale", CanonValue::u32(self.scale as u32))
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        v.expect_fields(&["coefficient", "kind", "precision", "scale"])?;
        if v.field("kind")?.as_str()? != "decimal.v1" {
            return Err(CoreError::new(
                ErrorCode::NonCanonicalEncoding,
                "expected decimal.v1",
            ));
        }
        let c = v.field("coefficient")?.as_i128()?;
        let p = v.field("precision")?.as_u32()?;
        let s = v.field("scale")?.as_u32()?;
        if p > 38 || s > 38 {
            return Err(CoreError::new(
                ErrorCode::InvalidDecimal,
                "precision/scale out of range",
            ));
        }
        Decimal::new(c, p as u8, s as u8)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_arithmetic() {
        let a = Decimal::parse("10.50", 10, 2).unwrap();
        let b = Decimal::parse("0.05", 10, 2).unwrap();
        assert_eq!(a.checked_add(&b).unwrap().to_text(), "10.55");
        assert_eq!(a.checked_sub(&b).unwrap().to_text(), "10.45");
        assert_eq!(a.checked_mul_int(3).unwrap().to_text(), "31.50");
        assert_eq!(Decimal::parse("-0.5", 5, 2).unwrap().to_text(), "-0.50");
    }

    #[test]
    fn overflow_and_inexact_rejected() {
        let max = Decimal::parse("99.99", 4, 2).unwrap();
        let one = Decimal::parse("0.01", 4, 2).unwrap();
        assert_eq!(
            max.checked_add(&one).unwrap_err().code,
            ErrorCode::NumericOverflow
        );
        assert!(Decimal::parse("1.234", 10, 2).is_err());
        assert!(Decimal::parse("+1", 10, 2).is_err());
        assert!(Decimal::parse("-0", 10, 2).is_err());
        assert!(Decimal::parse("01", 10, 2).is_err());
        let x = Decimal::parse("1.25", 10, 2).unwrap();
        assert!(x.rescale(10, 1).is_err());
        assert_eq!(x.rescale(12, 4).unwrap().to_text(), "1.2500");
        assert!(Decimal::new(1, 39, 0).is_err());
        assert!(Decimal::new(1, 5, 6).is_err());
    }

    #[test]
    fn scale_mismatch_is_type_error() {
        let a = Decimal::parse("1.0", 10, 1).unwrap();
        let b = Decimal::parse("1.00", 10, 2).unwrap();
        assert_eq!(a.checked_add(&b).unwrap_err().code, ErrorCode::TypeMismatch);
        assert!(a.partial_cmp(&b).is_none());
        assert_eq!(a.cmp_exact(&b).unwrap(), Ordering::Equal);
    }

    #[test]
    fn canonical_roundtrip() {
        let a = Decimal::parse("-12.30", 10, 2).unwrap();
        let bytes = a.encode();
        assert_eq!(
            String::from_utf8(bytes.clone()).unwrap(),
            r#"{"coefficient":"-1230","kind":"decimal.v1","precision":"10","scale":"2"}"#
        );
        assert_eq!(
            Decimal::decode(&bytes, &crate::limits::Limits::v1()).unwrap(),
            a
        );
    }
}
