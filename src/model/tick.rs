//! Integer rational rescaling of SSQ `tempo_data` values between TPS rates.
//!
//! `tempo_data` values in an SSQ tempo chunk are stored in "seconds-ticks"
//! (`seconds × TPS`). Modernizing a legacy SSQ to TPS=1000 means multiplying
//! these by `1000 / source_tps` (e.g., `20/3` for TPS=150, `40/3` for TPS=75).
//! This module does that multiplication exactly and reports values that would
//! require rounding.
//!
//! Note/tempo/event *measure-tick* positions are TPS-independent
//! (4096 per measure regardless of TPS) and need no rescaling.

use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum TickScaleError {
    #[error("source or destination TPS cannot be zero")]
    ZeroTps,
}

/// Lossless integer-rational scaler between two TPS values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TickScale {
    /// Destination TPS after reduction (numerator of the scale factor).
    num: u32,
    /// Source TPS after reduction (denominator of the scale factor).
    den: u32,
}

impl TickScale {
    /// Create a scaler that converts ticks from `src_tps` to `dst_tps`,
    /// reducing the ratio to lowest terms (e.g. 150→1000 becomes 20/3).
    pub fn new(src_tps: u32, dst_tps: u32) -> Result<Self, TickScaleError> {
        if src_tps == 0 || dst_tps == 0 {
            return Err(TickScaleError::ZeroTps);
        }
        let g = gcd(src_tps, dst_tps);
        Ok(Self {
            num: dst_tps / g,
            den: src_tps / g,
        })
    }

    /// Returns `Some(scaled)` if the scale is exact, `None` if it would
    /// require rounding. Callers choose the fallback (error, round, log).
    #[must_use]
    pub fn scale_exact(&self, ticks: i64) -> Option<i64> {
        let numerator = (ticks as i128).checked_mul(self.num as i128)?;
        if numerator % (self.den as i128) != 0 {
            return None;
        }
        i64::try_from(numerator / (self.den as i128)).ok()
    }

    /// Scale with round-half-away-from-zero fallback for inexact values.
    /// Use only where sub-tick drift is acceptable (e.g. audio-sync
    /// fine-tune, which is already noisy at the ±millisecond level).
    #[must_use]
    pub fn scale_rounded(&self, ticks: i64) -> Option<i64> {
        let numerator = (ticks as i128).checked_mul(self.num as i128)?;
        let den = self.den as i128;
        let half = if numerator >= 0 { den / 2 } else { -(den / 2) };
        i64::try_from((numerator + half) / den).ok()
    }

    /// Returns true if no scaling is needed (source and destination TPS are equal).
    #[must_use]
    pub fn is_identity(&self) -> bool {
        self.num == self.den
    }
}

fn gcd(mut a: u32, mut b: u32) -> u32 {
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

    #[test]
    fn identity_scale() {
        let s = TickScale::new(1000, 1000).unwrap();
        assert!(s.is_identity());
        assert_eq!(s.scale_exact(12345), Some(12345));
    }

    #[test]
    fn zero_tps_rejected() {
        assert_eq!(TickScale::new(0, 1000), Err(TickScaleError::ZeroTps));
        assert_eq!(TickScale::new(150, 0), Err(TickScaleError::ZeroTps));
    }

    #[test]
    fn scale_150_to_1000_exact_on_multiples_of_3() {
        // 1000/150 = 20/3, so exact iff input divisible by 3.
        let s = TickScale::new(150, 1000).unwrap();
        assert_eq!(s.scale_exact(3), Some(20));
        assert_eq!(s.scale_exact(150), Some(1000));
        assert_eq!(s.scale_exact(300), Some(2000));
    }

    #[test]
    fn scale_150_to_1000_inexact_returns_none() {
        let s = TickScale::new(150, 1000).unwrap();
        assert_eq!(s.scale_exact(1), None);
        assert_eq!(s.scale_exact(2), None);
    }

    #[test]
    fn round_trip_150_to_1000_to_150_on_divisible_values() {
        let up = TickScale::new(150, 1000).unwrap();
        let down = TickScale::new(1000, 150).unwrap();
        for t in [0, 3, 150, 1500, 300_000] {
            let at_1000 = up.scale_exact(t).unwrap();
            let back = down.scale_exact(at_1000).unwrap();
            assert_eq!(back, t, "round-trip failed for {t}");
        }
    }

    #[test]
    fn scale_rounded_rounds_half_away_from_zero() {
        let s = TickScale::new(150, 1000).unwrap();
        // 1 * 20 / 3 = 6.67 → 7
        assert_eq!(s.scale_rounded(1), Some(7));
        // 2 * 20 / 3 = 13.33 → 13
        assert_eq!(s.scale_rounded(2), Some(13));
        // negative: -1 * 20 / 3 = -6.67 → -7
        assert_eq!(s.scale_rounded(-1), Some(-7));
    }

    #[test]
    fn ratio_is_reduced_on_construction() {
        let s = TickScale::new(150, 1000).unwrap();
        assert_eq!((s.num, s.den), (20, 3));
    }
}
