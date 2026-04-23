//! Preview-clip metadata for a song.
//!
//! DDR's XWB holds a short `<name>_s` preview entry alongside the main
//! song. SM5's SSC uses `#SAMPLESTART` and `#SAMPLELENGTH` tags to the
//! same effect. This type is the format-independent representation and
//! provides the defaults used when a source format lacks preview info.

use super::rational::Rational;

/// A slice of the main audio reserved for previewing.
#[derive(Debug, Clone)]
pub struct PreviewSlice {
    pub start_seconds: Rational,
    pub length_seconds: Rational,
}

impl PreviewSlice {
    /// Default preview window used when no source-format metadata is
    /// available: 10-second clip starting 30 seconds into the song.
    #[must_use]
    pub fn default_window() -> Self {
        Self {
            start_seconds: Rational::from_integer(30),
            length_seconds: Rational::from_integer(10),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_window_is_30s_10s() {
        let p = PreviewSlice::default_window();
        assert_eq!(p.start_seconds, Rational::from_integer(30));
        assert_eq!(p.length_seconds, Rational::from_integer(10));
    }
}
