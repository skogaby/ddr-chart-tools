//! Reduced rational numbers with exact arithmetic.
//!
//! Stepfile positions, tempo values, and audio sync offsets must
//! round-trip losslessly across formats. Floats drift; this type does not.
//! The denominator is always positive; the numerator carries the sign.

use std::cmp::Ordering;
use std::num::NonZeroU64;

use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum RationalError {
    #[error("denominator cannot be zero")]
    ZeroDenominator,
    #[error("arithmetic overflow")]
    Overflow,
}

/// A rational number `num / den` in lowest terms.
#[derive(Debug, Clone, Copy)]
pub struct Rational {
    num: i64,
    den: NonZeroU64,
}

impl Rational {
    /// Construct from integer numerator and denominator, reducing to
    /// lowest terms. Rejects zero denominators.
    pub fn new(num: i64, den: i64) -> Result<Self, RationalError> {
        if den == 0 {
            return Err(RationalError::ZeroDenominator);
        }
        // Normalize sign onto the numerator.
        let (num, den_u) = if den < 0 {
            (
                num.checked_neg().ok_or(RationalError::Overflow)?,
                den.unsigned_abs(),
            )
        } else {
            (num, den as u64)
        };
        let g = gcd(num.unsigned_abs(), den_u);
        let num = num / (g as i64);
        let den = NonZeroU64::new(den_u / g).expect("den/g > 0 because den != 0");
        Ok(Self { num, den })
    }

    #[must_use]
    pub fn from_integer(n: i64) -> Self {
        Self {
            num: n,
            den: NonZeroU64::new(1).expect("1 != 0"),
        }
    }

    #[must_use]
    pub const fn zero() -> Self {
        Self {
            num: 0,
            // SAFETY: 1 is non-zero.
            den: unsafe { NonZeroU64::new_unchecked(1) },
        }
    }

    #[must_use]
    pub const fn num(&self) -> i64 {
        self.num
    }

    #[must_use]
    pub const fn den(&self) -> u64 {
        self.den.get()
    }

    #[must_use]
    pub fn is_zero(&self) -> bool {
        self.num == 0
    }

    /// Convert to `f64`. Lossy for numerators/denominators near `i64::MAX`.
    #[must_use]
    pub fn as_f64(&self) -> f64 {
        (self.num as f64) / (self.den.get() as f64)
    }

    pub fn add(&self, other: &Self) -> Result<Self, RationalError> {
        // a/b + c/d = (a*d + c*b) / (b*d)
        let bd = (self.den.get() as i128)
            .checked_mul(other.den.get() as i128)
            .ok_or(RationalError::Overflow)?;
        let ad = (self.num as i128)
            .checked_mul(other.den.get() as i128)
            .ok_or(RationalError::Overflow)?;
        let cb = (other.num as i128)
            .checked_mul(self.den.get() as i128)
            .ok_or(RationalError::Overflow)?;
        let num = ad.checked_add(cb).ok_or(RationalError::Overflow)?;
        Self::from_i128_ratio(num, bd)
    }

    pub fn sub(&self, other: &Self) -> Result<Self, RationalError> {
        let neg = Self::new(
            other.num.checked_neg().ok_or(RationalError::Overflow)?,
            other.den.get() as i64,
        )?;
        self.add(&neg)
    }

    pub fn mul(&self, other: &Self) -> Result<Self, RationalError> {
        let num = (self.num as i128)
            .checked_mul(other.num as i128)
            .ok_or(RationalError::Overflow)?;
        let den = (self.den.get() as i128)
            .checked_mul(other.den.get() as i128)
            .ok_or(RationalError::Overflow)?;
        Self::from_i128_ratio(num, den)
    }

    pub fn div(&self, other: &Self) -> Result<Self, RationalError> {
        if other.num == 0 {
            return Err(RationalError::ZeroDenominator);
        }
        let num = (self.num as i128)
            .checked_mul(other.den.get() as i128)
            .ok_or(RationalError::Overflow)?;
        let den = (self.den.get() as i128)
            .checked_mul(other.num as i128)
            .ok_or(RationalError::Overflow)?;
        Self::from_i128_ratio(num, den)
    }

    /// Reduce an i128 numerator / i128 denominator into a `Rational`,
    /// normalizing sign and checking that the result fits in i64/u64.
    fn from_i128_ratio(num: i128, den: i128) -> Result<Self, RationalError> {
        if den == 0 {
            return Err(RationalError::ZeroDenominator);
        }
        let (num, den) = if den < 0 { (-num, -den) } else { (num, den) };
        let g = gcd_i128(num.unsigned_abs(), den as u128);
        let num = num / (g as i128);
        let den = (den as u128) / g;
        let num_i64 = i64::try_from(num).map_err(|_| RationalError::Overflow)?;
        let den_u64 = u64::try_from(den).map_err(|_| RationalError::Overflow)?;
        let den_nz = NonZeroU64::new(den_u64).ok_or(RationalError::ZeroDenominator)?;
        Ok(Self {
            num: num_i64,
            den: den_nz,
        })
    }
}

impl PartialEq for Rational {
    fn eq(&self, other: &Self) -> bool {
        // Stored in lowest terms, so direct comparison is exact.
        self.num == other.num && self.den == other.den
    }
}

impl Eq for Rational {}

impl std::hash::Hash for Rational {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.num.hash(state);
        self.den.hash(state);
    }
}

impl PartialOrd for Rational {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Rational {
    fn cmp(&self, other: &Self) -> Ordering {
        // a/b vs c/d => a*d vs c*b (with b, d > 0), compared in i128
        // to avoid overflow when multiplying two i64s.
        let lhs = (self.num as i128) * (other.den.get() as i128);
        let rhs = (other.num as i128) * (self.den.get() as i128);
        lhs.cmp(&rhs)
    }
}

fn gcd(mut a: u64, mut b: u64) -> u64 {
    if a == 0 {
        return b.max(1);
    }
    while b != 0 {
        let t = a % b;
        a = b;
        b = t;
    }
    a.max(1)
}

fn gcd_i128(mut a: u128, mut b: u128) -> u128 {
    if a == 0 {
        return b.max(1);
    }
    while b != 0 {
        let t = a % b;
        a = b;
        b = t;
    }
    a.max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(num: i64, den: i64) -> Rational {
        Rational::new(num, den).unwrap()
    }

    #[test]
    fn reduces_to_lowest_terms() {
        let x = r(4, 8);
        assert_eq!(x.num(), 1);
        assert_eq!(x.den(), 2);
    }

    #[test]
    fn negative_denominator_flips_to_numerator() {
        let x = r(3, -4);
        assert_eq!(x.num(), -3);
        assert_eq!(x.den(), 4);
    }

    #[test]
    fn zero_denominator_is_rejected() {
        assert_eq!(Rational::new(1, 0), Err(RationalError::ZeroDenominator));
    }

    #[test]
    fn equal_after_reduction() {
        assert_eq!(r(2, 4), r(3, 6));
    }

    #[test]
    fn ordering_across_denominators() {
        // 1/3 < 1/2 < 2/3
        assert!(r(1, 3) < r(1, 2));
        assert!(r(1, 2) < r(2, 3));
    }

    #[test]
    fn add_common_denominator() {
        assert_eq!(r(1, 4).add(&r(1, 4)).unwrap(), r(1, 2));
    }

    #[test]
    fn add_different_denominators() {
        assert_eq!(r(1, 2).add(&r(1, 3)).unwrap(), r(5, 6));
    }

    #[test]
    fn sub_produces_negative() {
        assert_eq!(r(1, 3).sub(&r(1, 2)).unwrap(), r(-1, 6));
    }

    #[test]
    fn mul_reduces() {
        assert_eq!(r(2, 3).mul(&r(3, 4)).unwrap(), r(1, 2));
    }

    #[test]
    fn div_by_zero_rational_is_error() {
        assert_eq!(
            r(1, 2).div(&Rational::zero()),
            Err(RationalError::ZeroDenominator)
        );
    }

    #[test]
    fn div_is_inverse_of_mul() {
        let a = r(5, 7);
        let b = r(3, 11);
        assert_eq!(a.mul(&b).unwrap().div(&b).unwrap(), a);
    }

    #[test]
    fn from_integer() {
        let x = Rational::from_integer(42);
        assert_eq!(x.num(), 42);
        assert_eq!(x.den(), 1);
    }

    #[test]
    fn zero_is_zero() {
        assert!(Rational::zero().is_zero());
        assert!(!Rational::from_integer(1).is_zero());
    }

    #[test]
    fn as_f64_is_approximate() {
        assert!((r(1, 3).as_f64() - 1.0 / 3.0).abs() < 1e-12);
    }
}
