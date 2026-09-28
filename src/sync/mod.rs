//! Auto-sync analysis: measuring how far a chart sits from its audio.
//!
//! Owns onset analysis of an [`AudioBuffer`](crate::model::AudioBuffer),
//! the beat → audio-time mapping used for analysis ([`TimeMap`]), the
//! weighted chart events the audio is correlated against
//! ([`chart_events`]), and the offset estimate itself.
//!
//! Pure computation: no I/O, no format parsing, and no mutation of songs.
//! Applying a correction is the job layer's concern.
//!
//! Sign convention used throughout: a *measured offset* is audio onset
//! time minus chart event time, in milliseconds; a *correction* is how
//! far later the chart should move (negative = earlier), the same sense
//! as `--sync-offset-ms`.

mod estimate;
mod events;
mod onset;
mod timemap;

pub use estimate::estimate;
pub use events::{chart_events, SyncEvent};
pub use timemap::TimeMap;

/// Caller-controlled estimation parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Params {
    /// Largest correction that may be applied, in whole ms. Values above
    /// [`MAX_CORRECTION_LIMIT_MS`] are treated as that limit.
    pub max_correction_ms: u32,
}

impl Default for Params {
    fn default() -> Self {
        Self {
            max_correction_ms: DEFAULT_MAX_CORRECTION_MS,
        }
    }
}

/// Result of one estimate.
///
/// `measured_ms` is audio onset time minus chart event time; the chart
/// is in sync when it equals [`TARGET_OFFSET_MS`]. `correction_ms` is
/// `measured_ms − TARGET_OFFSET_MS`: how far *later* the chart should
/// move (negative = earlier). The measured fields are `None` only when
/// the estimator refused before measuring (`NoAudio`, `TooFewEvents`).
#[derive(Debug, Clone, PartialEq)]
pub struct SyncEstimate {
    /// What to do with the chart.
    pub outcome: Outcome,
    /// Refined score-peak position, in ms.
    pub measured_ms: Option<f64>,
    /// `measured_ms − TARGET_OFFSET_MS`, in ms.
    pub correction_ms: Option<f64>,
    /// Strongest rival peak relative to the main peak, above the median.
    pub rival: Option<f64>,
    /// Disagreement between the song's two halves, in ms.
    pub split_ms: Option<f64>,
    /// Chart events far enough inside the audio to be scored.
    pub events_used: usize,
}

/// Decision of an estimate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Move the chart `delta_ms` later (negative = earlier).
    Apply {
        /// Whole milliseconds, rounded half away from zero.
        delta_ms: i32,
    },
    /// The correction rounds to 0 ms: the chart is already in sync.
    Unchanged,
    /// The estimate is not trustworthy; leave the chart's sync alone.
    Refused(Refusal),
}

/// Why an estimate was not trusted. Reported in the order listed: the
/// first that applies wins.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// No decodable audio (empty, zero rate or channels, too short).
    NoAudio,
    /// Fewer than [`MIN_EVENTS`] chart events inside the audio.
    TooFewEvents,
    /// The score peak is at the edge of the search: the true offset may
    /// lie outside it.
    AtSearchEdge,
    /// Another alignment scores nearly as well as the best one.
    AmbiguousPeak,
    /// The two halves of the song want different corrections (tempo
    /// drift, a cut, or a wrong BPM).
    HalvesDisagree,
    /// The correction exceeds the cap.
    BeyondCap,
}

impl Refusal {
    /// Stable machine-readable label, used as `reason=` in log lines so
    /// batch runs can be grepped.
    #[must_use]
    pub fn key(self) -> &'static str {
        match self {
            Self::NoAudio => "no_audio",
            Self::TooFewEvents => "too_few_events",
            Self::AtSearchEdge => "at_search_edge",
            Self::AmbiguousPeak => "ambiguous_peak",
            Self::HalvesDisagree => "halves_disagree",
            Self::BeyondCap => "beyond_cap",
        }
    }
}

/// Measured offset (ms) of a chart that is in sync with its audio.
///
/// The onset detector responds a little after a perceived attack, so an
/// in-sync chart does not measure 0. The target is calibrated as
/// `median(measured + community offset)` over stock DDR World songs at
/// TPS 1000, where the community offset database gives each song's
/// play-validated correction.
///
/// Calibrated 2026-09-28 by `tests/auto_sync_calibration.rs` over 711
/// TPS 1000 stock songs: `T = −2.107 ms`; 94.1% of songs within 1 ms and
/// 97.6% within 2 ms of the community value; 99.3% accepted. Any change
/// to the onset parameters below invalidates it until recalibrated.
pub const TARGET_OFFSET_MS: f64 = -2.11;

/// Default cap on an applied correction, in whole ms. Covers the ~53 ms
/// offset seen when legacy console charts play on DDR World, while
/// keeping every half-beat alias of DDR's common 150–200 BPM music out of
/// reach.
pub const DEFAULT_MAX_CORRECTION_MS: u32 = 60;

/// Largest cap `--auto-sync-max-ms` accepts. Above roughly 100 ms,
/// half-beat aliases re-enter the search and only the refusal gates
/// guard against them.
pub const MAX_CORRECTION_LIMIT_MS: u32 = 200;

/// How far past the cap the search runs, in ms, so a correction close
/// to the cap is not truncated by the search edge. Without it about a
/// quarter of 55 ms corrections were refused at the edge.
pub const SEARCH_MARGIN_MS: u32 = 10;

/// A score peak this close (in 1 ms samples) to either end of the
/// search is refused: the true peak may lie outside the search.
pub const EDGE_MARGIN_SAMPLES: usize = 3;

/// Local maxima within this many ms of the main peak are part of it,
/// not rivals.
pub const RIVAL_EXCLUSION_MS: usize = 15;

/// Refuse when the strongest rival peak scores at least this fraction of
/// the main peak (both measured above the curve's median). Stock songs
/// that trip it are genuinely bistable.
pub const RIVAL_REFUSE_RATIO: f64 = 0.9;

/// Refuse when the first and second halves of the song disagree by more
/// than this many ms — a sign of tempo drift, a cut, or a wrong BPM. The
/// threshold simulated against the stock corpus.
pub const SPLIT_REFUSE_MS: f64 = 5.0;

/// Fewest usable chart events worth estimating from.
pub const MIN_EVENTS: usize = 32;

// ---------------------------------------------------------------------
// Onset envelope parameters (`onset`). These define the detector the
// target offset was calibrated for; see `TARGET_OFFSET_MS`.
// ---------------------------------------------------------------------

/// Analysis window length: 10 ms, the short window of the "rising edge"
/// feature (after +9ms or Null?, <https://github.com/telperion/nine-or-null>).
/// Short windows localize attacks sharply.
pub(crate) const ONSET_WINDOW_S: f64 = 0.010;

/// Hop between frames, in samples at 44.1 kHz (≈ 0.73 ms); scaled to the
/// buffer's rate so frame timing does not depend on it.
pub(crate) const ONSET_HOP_SAMPLES_AT_44K: f64 = 32.0;

/// Lower edge of the analysed band, in Hz: below it is rumble.
pub(crate) const ONSET_BAND_LO_HZ: f64 = 30.0;

/// Upper edge of the analysed band, in Hz.
pub(crate) const ONSET_BAND_HI_HZ: f64 = 16_000.0;

/// Spectral emphasis `f·e^(−f/ONSET_EMPHASIS_HZ)`, peaking at this
/// frequency: favors the attack band of drums and plucked sounds over
/// sub-bass and hiss. 3 kHz scored best against the community offsets.
pub(crate) const ONSET_EMPHASIS_HZ: f64 = 3_000.0;

/// Normalizes the emphasis curve to a peak of about 1
/// (`3000·e^(−1) ≈ 1103.6`).
pub(crate) const ONSET_EMPHASIS_NORM: f64 = 1_103.6;

/// Rising edge: each frame's energy minus the energy this long before.
pub(crate) const ONSET_LAG_S: f64 = 0.002;

/// Floor inside `log2(power + floor)`, keeping silence finite.
pub(crate) const ONSET_LOG_FLOOR: f32 = 1e-9;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refusal_keys_are_stable() {
        let keys = [
            (Refusal::NoAudio, "no_audio"),
            (Refusal::TooFewEvents, "too_few_events"),
            (Refusal::AtSearchEdge, "at_search_edge"),
            (Refusal::AmbiguousPeak, "ambiguous_peak"),
            (Refusal::HalvesDisagree, "halves_disagree"),
            (Refusal::BeyondCap, "beyond_cap"),
        ];
        for (reason, key) in keys {
            assert_eq!(reason.key(), key);
        }
    }
}
