//! Transform an [`SsqParseResult`] from a pre-DDR-World source into the
//! shape the modern SSQ writer accepts.
//!
//! Invoked by the job layer when `--from-format DDR_LEGACY` is set,
//! regardless of the source file's internal TPS. For a DDR World source
//! (`--from-format DDR`), modernization is NOT run even if the tempo
//! chunk used TPS=150 (spec §1.1 notes this coexistence).
//!
//! Transform steps:
//! 1. Shift the chart timeline so the first tempo segment starts at
//!    beat 0. Legacy charts (notably Ultramix) often encode a non-zero
//!    `time_offset[0]` representing an origin-shift between the chart's
//!    measure timeline and the audio-sync timeline. Modern SSQ/SSC
//!    writers expect the first tempo segment at beat 0. The audio
//!    sync offset is adjusted to preserve audio alignment.
//! 2. Set `song.tps = 1000` so the writer emits the modern tick rate.
//! 3. Clear `raw_tempo_pairs` so the writer synthesizes a fresh tempo
//!    chunk sized for TPS=1000 rather than passing through legacy pairs.
//!
//! Auxiliary chunks are NOT cleared here because they were already
//! dropped at parse time; `aux_chunks_dropped` remains as a diagnostic
//! record for the caller to log.

use crate::model::{Beat, Rational};
use crate::ssq::SsqParseResult;

const MODERN_TPS: u32 = 1000;

/// Modernize a parsed legacy SSQ in place.
pub fn modernize(result: &mut SsqParseResult) {
    shift_to_zero_origin(result);
    result.song.tps = MODERN_TPS;
    result.raw_tempo_pairs.clear();
}

/// If the first tempo segment's `start_beat` is non-zero, shift every
/// beat-valued field so the tempo timeline begins at beat 0. Adjusts
/// `audio_sync_offset_seconds` to compensate so audio alignment is
/// preserved.
fn shift_to_zero_origin(result: &mut SsqParseResult) {
    let Some(first) = result.song.tempo_segments.first() else {
        return;
    };
    let first_beat = first.start_beat.as_rational();
    if first_beat == Rational::zero() {
        return;
    }

    // shift = -first_beat (add this to every beat to move the timeline to 0).
    let shift = match Rational::zero().sub(&first_beat) {
        Ok(r) => r,
        Err(e) => {
            log::warn!("modernize: cannot compute shift: {e}");
            return;
        }
    };

    // Compute the audio-time equivalent of the shift, using the first
    // tempo segment's BPM: seconds = shift_beats × 60 / bpm.
    // Adding this to audio_sync_offset_seconds keeps the audio aligned.
    let bpm = first.bpm.as_rational();
    let shift_seconds = shift
        .mul(&Rational::from_integer(60))
        .and_then(|v| v.div(&bpm));
    match shift_seconds {
        Ok(delta) => match result.song.audio_sync_offset_seconds.add(&delta) {
            Ok(new) => result.song.audio_sync_offset_seconds = new,
            Err(e) => log::warn!("modernize: audio offset add failed: {e}"),
        },
        Err(e) => log::warn!("modernize: shift_seconds failed: {e}"),
    }

    // Shift tempo segment start beats.
    for seg in &mut result.song.tempo_segments {
        seg.start_beat = shift_beat(seg.start_beat, &shift);
    }
    // Shift stop positions.
    for stop in &mut result.song.stops {
        stop.at_beat = shift_beat(stop.at_beat, &shift);
    }
    // Shift event ticks (these are in SSQ measure-ticks, not beats).
    let shift_ticks = {
        // shift beats × 1024 ticks/beat; convert to i32.
        let t = shift.mul(&Rational::from_integer(Beat::TICKS_PER_BEAT));
        match t {
            Ok(r) if r.den() == 1 => i32::try_from(r.num()).ok(),
            _ => None,
        }
    };
    if let Some(delta_ticks) = shift_ticks {
        for ev in &mut result.events {
            ev.tick = ev.tick.saturating_add(delta_ticks);
        }
    }
    // Chart note beats are already ≥ 0 (verified empirically across the
    // Ultramix corpus), so they only need shifting if the shift is
    // non-zero — which means shifting them by a positive amount is fine.
    for chart in &mut result.song.charts {
        for note in &mut chart.notes {
            note.beat = shift_beat(note.beat, &shift);
        }
    }
}

fn shift_beat(beat: Beat, shift: &Rational) -> Beat {
    match beat.as_rational().add(shift) {
        Ok(r) => Beat::from_rational(r),
        Err(_) => beat,
    }
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
    fn clears_raw_tempo_pairs() {
        let mut r = legacy_result();
        modernize(&mut r);
        assert!(r.raw_tempo_pairs.is_empty());
    }

    #[test]
    fn leaves_semantic_view_unchanged() {
        let mut r = legacy_result();
        let before = r.song.tempo_segments.clone();
        modernize(&mut r);
        assert_eq!(r.song.tempo_segments, before);
    }
}
