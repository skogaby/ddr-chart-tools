//! Transform an [`SsqParseResult`] from a pre-DDR-World source into the
//! shape the modern SSQ writer accepts.
//!
//! Invoked by the job layer when `--from-format DDR_LEGACY` is set,
//! regardless of the source file's internal TPS. For a DDR World source
//! (`--from-format DDR`), modernization is NOT run even if the tempo
//! chunk used TPS=150 (spec §1.1 notes this coexistence).
//!
//! Transform steps (applied to `raw_tempo_pairs` in place so the SSQ
//! writer emits them verbatim — no semantic-view synthesis):
//!
//! 1. Shift every tempo/event/step measure-tick by `-time_offset[0]` so
//!    the first tempo entry sits at tick 0. Legacy Ultramix charts
//!    commonly encode a non-zero `time_offset[0]` representing an
//!    origin-shift between the chart's measure timeline and the
//!    audio-sync timeline. The audio file itself doesn't move; the
//!    shift is a pure relabeling of the chart-measure axis.
//! 2. Rescale every `tempo_data` seconds-tick value by `1000 / source_tps`
//!    so the writer can emit them at TPS=1000.
//! 3. Set `song.tps = 1000`.
//!
//! `audio_sync_offset_seconds` on the Song is recomputed from the
//! shifted+rescaled `tempo_data[0]` so SSC output has the correct
//! `#OFFSET` without double-counting.
//!
//! Auxiliary chunks are NOT cleared here because they were already
//! dropped at parse time; `aux_chunks_dropped` remains as a diagnostic
//! record for the caller to log.

use crate::model::{Beat, Rational, TickScale};
use crate::ssq::SsqParseResult;

const MODERN_TPS: u32 = 1000;

/// Modernize a parsed legacy SSQ in place.
pub fn modernize(result: &mut SsqParseResult) {
    let src_tps = result.song.tps;
    if src_tps == 0 || result.raw_tempo_pairs.is_empty() {
        result.song.tps = MODERN_TPS;
        return;
    }

    let scale = match TickScale::new(src_tps, MODERN_TPS) {
        Ok(s) => s,
        Err(e) => {
            log::warn!("modernize: cannot build TPS scaler: {e}");
            return;
        }
    };

    // Compute measure-tick shift from the first raw pair.
    let shift_ticks = -(result.raw_tempo_pairs[0].0 as i64);

    // Apply shift + rescale to raw_tempo_pairs.
    for pair in &mut result.raw_tempo_pairs {
        pair.0 = match i32::try_from(pair.0 as i64 + shift_ticks) {
            Ok(v) => v,
            Err(_) => {
                log::warn!("modernize: tempo tick overflow during shift");
                return;
            }
        };
        pair.1 = match scale.scale_rounded(pair.1 as i64).and_then(|v| i32::try_from(v).ok()) {
            Some(v) => v,
            None => {
                log::warn!("modernize: tempo seconds-tick rescale overflow");
                return;
            }
        };
    }

    // Apply same shift (in measure-ticks, not rescaled) to event ticks
    // and step note beats — these are in measure-ticks which are
    // TPS-independent.
    let shift_ticks_i32 = match i32::try_from(shift_ticks) {
        Ok(v) => v,
        Err(_) => 0,
    };
    for ev in &mut result.events {
        ev.tick = ev.tick.saturating_add(shift_ticks_i32);
    }
    let shift_beats = match Rational::new(shift_ticks, Beat::TICKS_PER_BEAT) {
        Ok(r) => r,
        Err(_) => Rational::zero(),
    };
    for seg in &mut result.song.tempo_segments {
        seg.start_beat = Beat::from_rational(
            seg.start_beat
                .as_rational()
                .add(&shift_beats)
                .unwrap_or(seg.start_beat.as_rational()),
        );
    }
    for stop in &mut result.song.stops {
        stop.at_beat = Beat::from_rational(
            stop.at_beat
                .as_rational()
                .add(&shift_beats)
                .unwrap_or(stop.at_beat.as_rational()),
        );
    }
    for chart in &mut result.song.charts {
        for note in &mut chart.notes {
            note.beat = Beat::from_rational(
                note.beat
                    .as_rational()
                    .add(&shift_beats)
                    .unwrap_or(note.beat.as_rational()),
            );
        }
    }

    // Recompute audio_sync_offset_seconds from the new raw_tempo_pairs[0].1
    // (which is now in seconds-ticks at MODERN_TPS).
    result.song.audio_sync_offset_seconds =
        Rational::new(result.raw_tempo_pairs[0].1 as i64, MODERN_TPS as i64)
            .unwrap_or(Rational::zero());

    result.song.tps = MODERN_TPS;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AudioBuffer, Beat, Bpm, PreviewSlice, Rational, Song, TempoSegment};
    use crate::ssq::SsqParseResult;

    fn legacy_result() -> SsqParseResult {
        SsqParseResult {
            song: Song {
                title: None,
                artist: None,
                tps: 150,
                tempo_segments: vec![TempoSegment {
                    start_beat: Beat::zero(),
                    bpm: Bpm::from_rational(Rational::from_integer(120)),
                }],
                stops: Vec::new(),
                charts: Vec::new(),
                audio: AudioBuffer {
                    samples: Vec::new(),
                    sample_rate: 0,
                    channels: 0,
                },
                audio_sync_offset_seconds: Rational::zero(),
                preview: PreviewSlice::default_window(),
            },
            events: Vec::new(),
            raw_tempo_pairs: vec![(0, 0), (4096, 300)],
            aux_chunks_dropped: Vec::new(),
        }
    }

    #[test]
    fn sets_tps_to_1000() {
        let mut r = legacy_result();
        assert_eq!(r.song.tps, 150);
        modernize(&mut r);
        assert_eq!(r.song.tps, 1000);
    }

    #[test]
    fn rescales_raw_tempo_pairs_to_modern_tps() {
        // Source TPS=150, pairs (0,0), (4096, 300).
        // 1000/150 = 20/3. 0*20/3=0. 300*20/3=2000 exactly.
        let mut r = legacy_result();
        modernize(&mut r);
        assert_eq!(r.raw_tempo_pairs, vec![(0, 0), (4096, 2000)]);
    }

    #[test]
    fn shifts_negative_origin_to_zero() {
        let mut r = legacy_result();
        // Override with a negative-origin tempo chunk.
        r.raw_tempo_pairs = vec![(-4096, -3), (4096, 251)];
        r.song.tps = 75;
        r.song.tempo_segments = vec![TempoSegment {
            start_beat: Beat::from_measure_ticks(-4096).unwrap(),
            bpm: Bpm::from_rational(Rational::from_integer(120)),
        }];
        modernize(&mut r);
        // First pair shifted to (0, ?). -3 seconds-ticks at TPS=75
        // rescales to -3 * 40/3 = -40 seconds-ticks at TPS=1000.
        assert_eq!(r.raw_tempo_pairs[0], (0, -40));
        // Second pair: tick 4096-(-4096) = 8192. tds 251 * 40/3 = 3346.67 → 3347 rounded.
        assert_eq!(r.raw_tempo_pairs[1].0, 8192);
        // Semantic tempo segment start_beat shifted from -4 to 0.
        assert_eq!(r.song.tempo_segments[0].start_beat, Beat::zero());
        // audio_sync_offset_seconds = -40/1000 = -0.040.
        assert_eq!(r.song.audio_sync_offset_seconds, Rational::new(-40, 1000).unwrap());
    }
}
