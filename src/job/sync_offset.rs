//! Moving a chart relative to its audio.
//!
//! Owns [`shift_timeline`], the one operation that moves the whole
//! chart earlier or later against unchanged audio, and the auto-sync
//! orchestration around it: [`auto_sync_delta`] runs the `sync`
//! estimator for a job, logs the outcome, and says how far to move the
//! chart. `--sync-offset-ms` and `--auto-sync` both move charts through
//! `shift_timeline`. The measurement itself lives in `crate::sync`.

use std::fmt::Write as _;

use crate::cli::job::{AutoSync, AutoSyncMode, Job};
use crate::model::{AudioBuffer, Chart, Rational, Song};
use crate::sync::{self, Outcome, Params, Refusal, SyncEstimate, TimeMap};

use super::JobError;

/// Measure how far `charts`, placed on `map`, sit from `audio`, log the
/// outcome for `job`, and return how many ms later to move the chart:
/// the correction in `Apply` mode, and 0 in `Report` mode, when already
/// in sync, or when the estimate is refused. A missing `map` (unusable
/// timing) is refused as having too few events. Never fails the job.
pub(super) fn auto_sync_delta(
    job: &Job,
    cfg: AutoSync,
    audio: &AudioBuffer,
    charts: &[Chart],
    map: Option<&TimeMap>,
) -> i32 {
    let events = map
        .map(|m| sync::chart_events(charts, m))
        .unwrap_or_default();
    let params = Params {
        max_correction_ms: cfg.max_correction_ms,
    };
    let estimate = sync::estimate(audio, &events, &params);
    let decision = decide(&estimate, cfg.mode, cfg.max_correction_ms);
    log::log!(
        decision.level,
        "{}: {}",
        job.chart_in.display(),
        decision.message
    );
    decision.delta_ms
}

/// What auto-sync does for one song, and the log line describing it.
#[derive(Debug, Clone, PartialEq)]
struct Decision {
    /// How many ms later to move the chart (0 = leave it).
    delta_ms: i32,
    level: log::Level,
    /// Human-readable text, then ` — ` and a stable `key=value` tail.
    message: String,
}

/// Turn an estimate into the applied delta and its log line.
///
/// The tail always starts with `auto_sync=` (`apply`, `unchanged`,
/// `report`, or `refused`), followed by `delta_ms=` (the correction
/// applied, or in report mode the one that would be) or `reason=` for a
/// refusal, then the measured values when there are any, then `events=`.
fn decide(est: &SyncEstimate, mode: AutoSyncMode, cap_ms: u32) -> Decision {
    let correction = est.correction_ms.unwrap_or(0.0);
    let direction = |d: i32| if d > 0 { "later" } else { "earlier" };
    let (kind, delta_ms, level, text) = match (est.outcome, mode) {
        (Outcome::Refused(reason), _) => (
            "refused",
            None,
            log::Level::Warn,
            format!(
                "auto-sync left the sync unchanged: {}",
                refusal_advice(reason, est, cap_ms)
            ),
        ),
        (Outcome::Apply { delta_ms }, AutoSyncMode::Apply) => (
            "apply",
            Some(delta_ms),
            log::Level::Info,
            format!(
                "auto-sync moved the chart {} ms {} (measured correction {correction:+.2} ms)",
                delta_ms.unsigned_abs(),
                direction(delta_ms)
            ),
        ),
        (Outcome::Apply { delta_ms }, AutoSyncMode::Report) => (
            "report",
            Some(delta_ms),
            log::Level::Info,
            format!(
                "auto-sync (report) would move the chart {} ms {} (measured correction \
                 {correction:+.2} ms)",
                delta_ms.unsigned_abs(),
                direction(delta_ms)
            ),
        ),
        (Outcome::Unchanged, mode) => (
            if mode == AutoSyncMode::Report {
                "report"
            } else {
                "unchanged"
            },
            Some(0),
            log::Level::Info,
            format!("auto-sync: already in sync (correction {correction:+.2} ms rounds to 0)"),
        ),
    };

    let mut message = format!("{text} — auto_sync={kind}");
    match (delta_ms, est.outcome) {
        (Some(d), _) => {
            let _ = write!(message, " delta_ms={d}");
        }
        (None, Outcome::Refused(reason)) => {
            let _ = write!(message, " reason={}", reason.key());
        }
        (None, _) => {}
    }
    for (key, value) in [
        ("correction_ms", est.correction_ms),
        ("measured_ms", est.measured_ms),
        ("rival", est.rival),
        ("split_ms", est.split_ms),
    ] {
        if let Some(v) = value {
            let _ = write!(message, " {key}={v:.2}");
        }
    }
    let _ = write!(message, " events={}", est.events_used);

    let applied = match (mode, delta_ms) {
        (AutoSyncMode::Apply, Some(d)) => d,
        _ => 0,
    };
    Decision {
        delta_ms: applied,
        level,
        message,
    }
}

/// Why a refused estimate was not trusted, and what the user can do.
fn refusal_advice(reason: Refusal, est: &SyncEstimate, cap_ms: u32) -> String {
    match reason {
        Refusal::NoAudio => "no usable audio to analyze".to_string(),
        Refusal::TooFewEvents => format!(
            "too few notes to measure against ({} usable, need {})",
            est.events_used,
            sync::MIN_EVENTS
        ),
        Refusal::AtSearchEdge => format!(
            "the best alignment is at the edge of the ±{} ms search, so the sync may be off by \
             more than {cap_ms} ms; raise --auto-sync-max-ms or fix it by hand",
            cap_ms + sync::SEARCH_MARGIN_MS
        ),
        Refusal::AmbiguousPeak => format!(
            "two alignments fit about equally well (rival {:.2}); check the sync by hand",
            est.rival.unwrap_or(1.0)
        ),
        Refusal::HalvesDisagree => format!(
            "the song's two halves disagree by {:.1} ms; the tempo may drift or the audio may be \
             cut",
            est.split_ms.unwrap_or(0.0)
        ),
        Refusal::BeyondCap => format!(
            "the sync looks off by about {:+.0} ms, beyond the ±{cap_ms} ms cap; raise \
             --auto-sync-max-ms or fix it by hand",
            est.correction_ms.unwrap_or(0.0)
        ),
    }
}

/// Move the whole chart `delta_ms` later relative to the audio
/// (negative moves it earlier).
///
/// Adds `delta_ms` to every `tempo_data` value in `tempo_pairs` and
/// `delta_ms / 1000` seconds to `song.audio_sync_offset_seconds`.
/// Both carry the DDR sign convention (positive = beat 0 later in the
/// audio), so SSQ output moves every anchor and SSC output moves
/// `#OFFSET` by `−delta_ms / 1000`. BPMs and stops are unchanged
/// because every anchor moves together.
///
/// `tempo_pairs` are `(measure_tick, tempo_data)` at TPS 1000, where a
/// seconds-tick is a millisecond. Paths that write SSC pass an empty
/// slice. Nothing is modified unless every new value is representable.
pub(super) fn shift_timeline(
    song: &mut Song,
    tempo_pairs: &mut [(i32, i32)],
    delta_ms: i32,
) -> Result<(), JobError> {
    if delta_ms == 0 {
        return Ok(());
    }
    let overflow = || JobError::SyncShiftOverflow { delta_ms };

    let shifted_ticks = tempo_pairs
        .iter()
        .map(|&(_, td)| td.checked_add(delta_ms).ok_or_else(overflow))
        .collect::<Result<Vec<i32>, JobError>>()?;
    let shifted_offset = Rational::new(i64::from(delta_ms), 1000)
        .and_then(|delta| song.audio_sync_offset_seconds.add(&delta))
        .map_err(|_| overflow())?;

    for (pair, td) in tempo_pairs.iter_mut().zip(shifted_ticks) {
        pair.1 = td;
    }
    song.audio_sync_offset_seconds = shifted_offset;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AudioBuffer, PreviewSlice};

    fn song_with_offset_ms(ms: i64) -> Result<Song, Box<dyn std::error::Error>> {
        Ok(Song {
            title: None,
            artist: None,
            tps: 1000,
            tempo_segments: Vec::new(),
            stops: Vec::new(),
            charts: Vec::new(),
            audio: AudioBuffer {
                samples: Vec::new(),
                sample_rate: 0,
                channels: 0,
            },
            audio_sync_offset_seconds: Rational::new(ms, 1000)?,
            preview: PreviewSlice::default_window(),
        })
    }

    /// Two segments with a 500 ms stop between them.
    fn multi_segment_pairs() -> Vec<(i32, i32)> {
        vec![
            (0, 20),
            (4096, 2020),
            (8192, 4020),
            (8192, 4520),
            (16384, 8520),
        ]
    }

    fn diffs(pairs: &[(i32, i32)]) -> Vec<(i32, i32)> {
        pairs
            .windows(2)
            .map(|w| (w[1].0 - w[0].0, w[1].1 - w[0].1))
            .collect()
    }

    #[test]
    fn shifts_every_pair_and_preserves_slopes_and_stops() -> Result<(), Box<dyn std::error::Error>>
    {
        let mut song = song_with_offset_ms(20)?;
        let mut pairs = multi_segment_pairs();
        shift_timeline(&mut song, &mut pairs, 53)?;

        assert_eq!(
            pairs,
            vec![
                (0, 73),
                (4096, 2073),
                (8192, 4073),
                (8192, 4573),
                (16384, 8573)
            ]
        );
        assert_eq!(diffs(&pairs), diffs(&multi_segment_pairs()));
        assert_eq!(song.audio_sync_offset_seconds, Rational::new(73, 1000)?);
        Ok(())
    }

    #[test]
    fn single_bpm_song_moves_its_final_pair() -> Result<(), Box<dyn std::error::Error>> {
        // Regression: the old bias only touched the first pair, so the
        // correction faded to nothing by the end of a single-BPM song.
        let mut song = song_with_offset_ms(0)?;
        let mut pairs = vec![(0, 0), (118_784, 58_000)];
        shift_timeline(&mut song, &mut pairs, 53)?;
        assert_eq!(pairs, vec![(0, 53), (118_784, 58_053)]);
        Ok(())
    }

    #[test]
    fn negative_shift_moves_everything_earlier() -> Result<(), Box<dyn std::error::Error>> {
        let mut song = song_with_offset_ms(20)?;
        let mut pairs = multi_segment_pairs();
        shift_timeline(&mut song, &mut pairs, -30)?;

        let expected: Vec<(i32, i32)> = multi_segment_pairs()
            .into_iter()
            .map(|(t, td)| (t, td - 30))
            .collect();
        assert_eq!(pairs, expected);
        assert_eq!(song.audio_sync_offset_seconds, Rational::new(-10, 1000)?);
        Ok(())
    }

    #[test]
    fn zero_shift_is_a_no_op() -> Result<(), Box<dyn std::error::Error>> {
        let mut song = song_with_offset_ms(20)?;
        let mut pairs = multi_segment_pairs();
        shift_timeline(&mut song, &mut pairs, 0)?;
        assert_eq!(pairs, multi_segment_pairs());
        assert_eq!(song.audio_sync_offset_seconds, Rational::new(20, 1000)?);
        Ok(())
    }

    #[test]
    fn empty_pair_list_shifts_only_the_model_offset() -> Result<(), Box<dyn std::error::Error>> {
        let mut song = song_with_offset_ms(0)?;
        shift_timeline(&mut song, &mut [], 7)?;
        assert_eq!(song.audio_sync_offset_seconds, Rational::new(7, 1000)?);
        Ok(())
    }

    // ---------- auto-sync decision and logging ----------

    use crate::cli::job::AutoSyncMode;
    use crate::sync::{Outcome, Refusal, SyncEstimate};

    fn measured(outcome: Outcome, correction_ms: f64) -> SyncEstimate {
        SyncEstimate {
            outcome,
            measured_ms: Some(correction_ms - 2.11),
            correction_ms: Some(correction_ms),
            rival: Some(0.21),
            split_ms: Some(0.9),
            events_used: 412,
        }
    }

    #[test]
    fn apply_moves_and_logs_tail() {
        let d = decide(
            &measured(Outcome::Apply { delta_ms: 7 }, 6.81),
            AutoSyncMode::Apply,
            60,
        );
        assert_eq!(d.delta_ms, 7);
        assert_eq!(d.level, log::Level::Info);
        assert!(
            d.message.contains("moved the chart 7 ms later"),
            "{}",
            d.message
        );
        assert!(
            d.message.ends_with(
                "auto_sync=apply delta_ms=7 correction_ms=6.81 measured_ms=4.70 \
                 rival=0.21 split_ms=0.90 events=412"
            ),
            "{}",
            d.message
        );
    }

    #[test]
    fn negative_delta_says_earlier() {
        let d = decide(
            &measured(Outcome::Apply { delta_ms: -12 }, -12.3),
            AutoSyncMode::Apply,
            60,
        );
        assert_eq!(d.delta_ms, -12);
        assert!(
            d.message.contains("moved the chart 12 ms earlier"),
            "{}",
            d.message
        );
        assert!(d.message.contains("delta_ms=-12 "), "{}", d.message);
    }

    #[test]
    fn unchanged_logs_info() {
        let d = decide(&measured(Outcome::Unchanged, 0.3), AutoSyncMode::Apply, 60);
        assert_eq!(d.delta_ms, 0);
        assert_eq!(d.level, log::Level::Info);
        assert!(d.message.contains("already in sync"), "{}", d.message);
        assert!(
            d.message.contains("auto_sync=unchanged delta_ms=0 "),
            "{}",
            d.message
        );
    }

    #[test]
    fn report_mode_applies_nothing() {
        let d = decide(
            &measured(Outcome::Apply { delta_ms: -12 }, -12.3),
            AutoSyncMode::Report,
            60,
        );
        assert_eq!(d.delta_ms, 0, "report mode never moves the chart");
        assert_eq!(d.level, log::Level::Info);
        assert!(
            d.message.contains("would move the chart 12 ms earlier"),
            "{}",
            d.message
        );
        assert!(
            d.message.contains("auto_sync=report delta_ms=-12 "),
            "{}",
            d.message
        );
    }

    #[test]
    fn report_mode_refusal_is_refused() {
        let d = decide(
            &measured(Outcome::Refused(Refusal::HalvesDisagree), 3.0),
            AutoSyncMode::Report,
            60,
        );
        assert_eq!(d.delta_ms, 0);
        assert_eq!(d.level, log::Level::Warn);
        assert!(
            d.message
                .contains("auto_sync=refused reason=halves_disagree"),
            "{}",
            d.message
        );
    }

    #[test]
    fn each_refusal_has_actionable_message_and_key() {
        let cases = [
            (Refusal::AtSearchEdge, "--auto-sync-max-ms"),
            (Refusal::AmbiguousPeak, "equally well"),
            (Refusal::HalvesDisagree, "drift"),
            (Refusal::BeyondCap, "--auto-sync-max-ms"),
        ];
        for (reason, hint) in cases {
            let d = decide(
                &measured(Outcome::Refused(reason), 64.0),
                AutoSyncMode::Apply,
                60,
            );
            assert_eq!(d.delta_ms, 0, "{reason:?}");
            assert_eq!(d.level, log::Level::Warn, "{reason:?}");
            assert!(
                d.message.contains("left the sync unchanged"),
                "{}",
                d.message
            );
            assert!(d.message.contains(hint), "{reason:?}: {}", d.message);
            assert!(
                d.message.contains(&format!("reason={}", reason.key())),
                "{}",
                d.message
            );
        }
    }

    #[test]
    fn unmeasured_refusal_omits_measured_keys() {
        for reason in [Refusal::NoAudio, Refusal::TooFewEvents] {
            let est = SyncEstimate {
                outcome: Outcome::Refused(reason),
                measured_ms: None,
                correction_ms: None,
                rival: None,
                split_ms: None,
                events_used: 10,
            };
            let d = decide(&est, AutoSyncMode::Apply, 60);
            assert_eq!(d.level, log::Level::Warn);
            assert!(!d.message.contains("measured_ms="), "{}", d.message);
            assert!(
                d.message.ends_with(&format!(
                    "auto_sync=refused reason={} events=10",
                    reason.key()
                )),
                "{}",
                d.message
            );
        }
    }

    #[test]
    fn overflow_is_reported_without_mutation() -> Result<(), Box<dyn std::error::Error>> {
        let mut song = song_with_offset_ms(20)?;
        let original = vec![(0, 0), (4096, i32::MAX - 5)];
        let mut pairs = original.clone();
        let err = shift_timeline(&mut song, &mut pairs, 10);
        assert!(
            matches!(err, Err(JobError::SyncShiftOverflow { delta_ms: 10 })),
            "got {err:?}"
        );
        assert_eq!(pairs, original, "no pair may change on overflow");
        assert_eq!(song.audio_sync_offset_seconds, Rational::new(20, 1000)?);

        let mut pairs = vec![(0, i32::MIN + 3)];
        let err = shift_timeline(&mut song, &mut pairs, -10);
        assert!(
            matches!(err, Err(JobError::SyncShiftOverflow { delta_ms: -10 })),
            "got {err:?}"
        );
        assert_eq!(pairs, vec![(0, i32::MIN + 3)]);
        Ok(())
    }
}
