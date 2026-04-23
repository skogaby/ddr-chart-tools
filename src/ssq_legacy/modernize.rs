//! Transform an [`SsqParseResult`] from a pre-DDR-World source into the
//! shape the modern SSQ writer accepts.
//!
//! Invoked by the job layer when `--from-format DDR_LEGACY` is set,
//! regardless of the source file's internal TPS. For a DDR World source
//! (`--from-format DDR`), modernization is NOT run even if the tempo
//! chunk used TPS=150 (spec §1.1 notes this coexistence).
//!
//! Transform steps:
//! 1. Set `song.tps = 1000` so the writer emits the modern tick rate.
//! 2. Clear `raw_tempo_pairs` so the writer synthesizes a fresh tempo
//!    chunk sized for TPS=1000 rather than passing through legacy pairs.
//! 3. Leave everything else unchanged — all positional data (measure
//!    ticks, rational seconds, beat positions, BPM values) is already
//!    TPS-independent and survives the TPS change untouched.
//!
//! Auxiliary chunks are NOT cleared here because they were already
//! dropped at parse time; `aux_chunks_dropped` remains as a diagnostic
//! record for the caller to log.

use crate::ssq::SsqParseResult;

const MODERN_TPS: u32 = 1000;

/// Modernize a parsed legacy SSQ in place.
pub fn modernize(result: &mut SsqParseResult) {
    result.song.tps = MODERN_TPS;
    result.raw_tempo_pairs.clear();
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
