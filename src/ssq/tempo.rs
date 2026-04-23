//! Type-1 tempo chunk parser (spec §3).
//!
//! Decodes TPS, BPM segments, stops, and the audio-sync offset from the
//! tempo chunk's body. The chunk body is `N × i32` measure-tick offsets
//! followed by `N × i32` seconds-tick tempo values.

use crate::model::{Beat, Bpm, Rational, Stop, TempoSegment};
use crate::util::io::LeReader;

use super::chunk::ChunkHeader;
use super::SsqError;

/// Output of parsing one tempo chunk. These fields are folded into the
/// partial `Song` under construction.
#[derive(Debug)]
pub struct TempoParseResult {
    pub tps: u32,
    pub tempo_segments: Vec<TempoSegment>,
    pub stops: Vec<Stop>,
    pub audio_sync_offset_seconds: Rational,
    /// Raw `(time_offset, tempo_data)` pairs in file order. Preserved
    /// so a DDR→DDR writer can round-trip the tempo chunk byte-exactly.
    pub raw_pairs: Vec<(i32, i32)>,
}

/// Parse a type-1 tempo chunk.
pub fn parse_tempo_chunk(
    header: &ChunkHeader,
    body: &[u8],
    chunk_offset: usize,
) -> Result<TempoParseResult, SsqError> {
    let tps = u32::from(header.param2);
    if tps == 0 {
        return Err(SsqError::InvalidTps {
            offset: chunk_offset,
        });
    }
    if tps != 1000 && tps != 150 {
        log::warn!(
            "unexpected TPS {tps} in tempo chunk at byte {chunk_offset} (expected 150 or 1000)"
        );
    }

    let entry_count = usize::from(header.param3);
    let expected_body = entry_count.checked_mul(8).ok_or(SsqError::MalformedChunk {
        offset: chunk_offset,
        reason: format!("tempo entry count {entry_count} overflows body size"),
    })?;
    if body.len() != expected_body {
        return Err(SsqError::MalformedChunk {
            offset: chunk_offset,
            reason: format!(
                "tempo body size {} does not match {entry_count} entries × 8 bytes ({expected_body})",
                body.len()
            ),
        });
    }

    if entry_count == 0 {
        return Err(SsqError::MalformedChunk {
            offset: chunk_offset,
            reason: "tempo chunk has zero entries".to_string(),
        });
    }

    let mut reader = LeReader::new(body);
    let mut time_offsets = Vec::with_capacity(entry_count);
    for _ in 0..entry_count {
        time_offsets.push(reader.read_u32().map_err(SsqError::Io)? as i32);
    }
    let mut tempo_data = Vec::with_capacity(entry_count);
    for _ in 0..entry_count {
        tempo_data.push(reader.read_u32().map_err(SsqError::Io)? as i32);
    }

    // Spec §3.1: time_offset[0] is always 0.
    if time_offsets[0] != 0 {
        return Err(SsqError::MalformedChunk {
            offset: chunk_offset,
            reason: format!("time_offset[0] must be 0, got {}", time_offsets[0]),
        });
    }

    // Spec §3.1: tempo_data[0] is the audio-sync offset in seconds-ticks.
    let audio_sync_offset_seconds = Rational::new(i64::from(tempo_data[0]), i64::from(tps))
        .map_err(|e| SsqError::MalformedChunk {
            offset: chunk_offset,
            reason: format!("invalid audio sync offset: {e}"),
        })?;

    let mut tempo_segments = Vec::new();
    let mut stops = Vec::new();

    for i in 1..entry_count {
        let delta_measure = i64::from(time_offsets[i]) - i64::from(time_offsets[i - 1]);
        let delta_seconds_ticks = i64::from(tempo_data[i]) - i64::from(tempo_data[i - 1]);

        if delta_measure == 0 {
            // Spec §3.3: stop encoded as equal consecutive time_offsets.
            let at_beat = Beat::from_measure_ticks(i64::from(time_offsets[i]))
                .map_err(|e| chunk_math_err(chunk_offset, e))?;
            let duration_seconds = Rational::new(delta_seconds_ticks, i64::from(tps))
                .map_err(|e| chunk_math_err(chunk_offset, e))?;
            stops.push(Stop {
                at_beat,
                duration_seconds,
            });
        } else {
            // Spec §3.2: BPM = 240 * TPS * delta_measure / (4096 * delta_seconds_ticks)
            if delta_seconds_ticks == 0 {
                return Err(SsqError::MalformedChunk {
                    offset: chunk_offset,
                    reason: format!(
                        "tempo entry {i} advances measure-ticks without advancing seconds-ticks"
                    ),
                });
            }
            let num = 240i64
                .checked_mul(i64::from(tps))
                .and_then(|v| v.checked_mul(delta_measure))
                .ok_or_else(|| SsqError::MalformedChunk {
                    offset: chunk_offset,
                    reason: format!("BPM numerator overflow at entry {i}"),
                })?;
            let den = 4096i64.checked_mul(delta_seconds_ticks).ok_or_else(|| {
                SsqError::MalformedChunk {
                    offset: chunk_offset,
                    reason: format!("BPM denominator overflow at entry {i}"),
                }
            })?;
            let bpm_rational =
                Rational::new(num, den).map_err(|e| chunk_math_err(chunk_offset, e))?;
            let start_beat = Beat::from_measure_ticks(i64::from(time_offsets[i - 1]))
                .map_err(|e| chunk_math_err(chunk_offset, e))?;
            tempo_segments.push(TempoSegment {
                start_beat,
                bpm: Bpm::from_rational(bpm_rational),
            });
        }
    }

    let raw_pairs: Vec<(i32, i32)> = time_offsets
        .iter()
        .zip(tempo_data.iter())
        .map(|(t, d)| (*t, *d))
        .collect();

    Ok(TempoParseResult {
        tps,
        tempo_segments,
        stops,
        audio_sync_offset_seconds,
        raw_pairs,
    })
}

fn chunk_math_err(offset: usize, err: crate::model::RationalError) -> SsqError {
    SsqError::MalformedChunk {
        offset,
        reason: format!("tempo math: {err}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_tempo_chunk(
        tps: u16,
        time_offsets: &[i32],
        tempo_data: &[i32],
    ) -> (ChunkHeader, Vec<u8>) {
        assert_eq!(time_offsets.len(), tempo_data.len());
        let n = time_offsets.len() as u16;
        let header = ChunkHeader {
            length: 12 + 8 * u32::from(n),
            ty: 1,
            param2: tps,
            param3: n,
            param4: 0,
        };
        let mut body = Vec::new();
        for t in time_offsets {
            body.extend_from_slice(&(*t as u32).to_le_bytes());
        }
        for t in tempo_data {
            body.extend_from_slice(&(*t as u32).to_le_bytes());
        }
        (header, body)
    }

    #[test]
    fn parses_modern_tempo_chunk_single_segment() {
        // TPS=1000, 2 entries, no stops: 1 segment of 120 BPM.
        // delta_measure = 4096 (one whole note)
        // delta_seconds = 2 * 1000 = 2000 seconds-ticks
        // BPM = 240 * 1000 * 4096 / (4096 * 2000) = 240000/2000 = 120
        let (h, body) = build_tempo_chunk(1000, &[0, 4096], &[0, 2000]);
        let result = parse_tempo_chunk(&h, &body, 0).unwrap();
        assert_eq!(result.tps, 1000);
        assert_eq!(result.stops, vec![]);
        assert_eq!(result.tempo_segments.len(), 1);
        assert_eq!(
            result.tempo_segments[0].bpm,
            Bpm::from_rational(Rational::from_integer(120))
        );
        assert_eq!(result.audio_sync_offset_seconds, Rational::zero());
    }

    #[test]
    fn parses_tps_150_tempo_chunk_with_stop() {
        // A TPS=150 tempo chunk (still a DDR World asset — see docs §1.1;
        // both TPS values coexist inside DDR World). Mimics the structure
        // of the example in docs §3.5 (aeth.ssq first three entries):
        //   time_offset: [0, 4096, 73728, 73728]  (last two equal = stop)
        //   tempo_data:  [0, 94, 1689, 1829]
        // Stop duration = (1829 - 1689) / 150 seconds.
        let (h, body) = build_tempo_chunk(150, &[0, 4096, 73728, 73728], &[0, 94, 1689, 1829]);
        let result = parse_tempo_chunk(&h, &body, 0).unwrap();
        assert_eq!(result.tps, 150);
        assert_eq!(result.stops.len(), 1);
        assert_eq!(
            result.stops[0].duration_seconds,
            Rational::new(140, 150).unwrap()
        );
        assert_eq!(result.tempo_segments.len(), 2);
    }

    #[test]
    fn audio_sync_offset_scales_by_tps() {
        // TPS=1000, tempo_data[0] = 22 → offset = 22/1000 s
        let (h, body) = build_tempo_chunk(1000, &[0, 4096], &[22, 2022]);
        let result = parse_tempo_chunk(&h, &body, 0).unwrap();
        assert_eq!(
            result.audio_sync_offset_seconds,
            Rational::new(22, 1000).unwrap()
        );
    }

    #[test]
    fn zero_tps_is_rejected() {
        let (h, body) = build_tempo_chunk(0, &[0, 4096], &[0, 2000]);
        let err = parse_tempo_chunk(&h, &body, 42).unwrap_err();
        assert!(matches!(err, SsqError::InvalidTps { offset: 42 }));
    }

    #[test]
    fn unusual_tps_warns_but_parses() {
        // 500 is not a known TPS — emits a warn but still parses.
        let (h, body) = build_tempo_chunk(500, &[0, 4096], &[0, 1000]);
        let result = parse_tempo_chunk(&h, &body, 0).unwrap();
        assert_eq!(result.tps, 500);
    }

    #[test]
    fn non_zero_first_time_offset_is_rejected() {
        let (h, body) = build_tempo_chunk(1000, &[1, 4096], &[0, 2000]);
        let err = parse_tempo_chunk(&h, &body, 0).unwrap_err();
        assert!(matches!(err, SsqError::MalformedChunk { .. }));
    }

    #[test]
    fn zero_entry_count_is_rejected() {
        let h = ChunkHeader {
            length: 12,
            ty: 1,
            param2: 1000,
            param3: 0,
            param4: 0,
        };
        let err = parse_tempo_chunk(&h, &[], 0).unwrap_err();
        assert!(matches!(err, SsqError::MalformedChunk { .. }));
    }

    #[test]
    fn body_size_mismatch_is_rejected() {
        let h = ChunkHeader {
            length: 12 + 8 * 2,
            ty: 1,
            param2: 1000,
            param3: 2,
            param4: 0,
        };
        let short_body = vec![0u8; 8]; // should be 16
        let err = parse_tempo_chunk(&h, &short_body, 0).unwrap_err();
        assert!(matches!(err, SsqError::MalformedChunk { .. }));
    }
}
