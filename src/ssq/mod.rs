//! DDR SSQ stepfile parser.
//!
//! Accepts SSQs with either tick rate observed in DDR World (TPS=1000
//! or TPS=150, see `docs/ssq_format.md` §1.1) and either chunk shape
//! (modern chunks 1/2/3, or with auxiliary chunks 4/5/9/17). The TPS
//! distinction is independent of whether the file is from DDR World or
//! an earlier DDR release — that distinction is a CLI/job-level concern
//! captured by the `--from-format` flag, not by the parser.
//!
//! `parse` returns an [`SsqParseResult`] bundling the format-independent
//! [`Song`] with SSQ-specific sidecar data: raw events (preserved for
//! DDR→DDR round-trips) and metadata describing any auxiliary chunks
//! dropped during parse.

pub mod aux;
pub mod chunk;
pub mod events;
pub mod steps;
pub mod tempo;
pub mod writer;

use thiserror::Error;

use crate::model::{AudioBuffer, PreviewSlice, Rational, Song, Stop, TempoSegment};
use crate::util::io::{IoError, LeReader};

use aux::AuxMeta;
use chunk::{read_header, ChunkHeader};
use events::SsqEvent;

#[derive(Debug, Error)]
pub enum SsqError {
    #[error("I/O error while parsing SSQ: {0}")]
    Io(#[from] IoError),

    #[error("malformed chunk at byte {offset}: {reason}")]
    MalformedChunk { offset: usize, reason: String },

    #[error("unexpected chunk type {ty} at byte {offset} (not yet supported by this parser)")]
    UnexpectedChunkType { ty: u16, offset: usize },

    #[error("SSQ file is missing the required tempo chunk (type 1)")]
    MissingTempoChunk,

    #[error("SSQ file contains multiple tempo chunks; exactly one is required")]
    DuplicateTempoChunk,

    #[error("SSQ file is missing the required events chunk (type 2)")]
    MissingEventsChunk,

    #[error("SSQ file contains multiple events chunks; exactly one is required")]
    DuplicateEventsChunk,

    #[error("tempo chunk at byte {offset} has invalid TPS value 0")]
    InvalidTps { offset: usize },

    #[error("step chunk at byte {offset} has invalid difficulty code 0x{code:04X}")]
    InvalidDifficultyCode { code: u16, offset: usize },

    #[error("step chunk at byte {freeze_offset}: freeze-end at tick {tick} has no matching head for panel {panel}")]
    FreezeWithoutHead {
        freeze_offset: usize,
        tick: i32,
        panel: u8,
    },

    #[error("SSQ writer refuses to emit TPS={tps}; only modern TPS=1000 is supported")]
    CannotWriteTps { tps: u32 },

    #[error("SSQ write failed: {0}")]
    Write(String),

    #[error("cannot serialize beat {beat}/{beat_den} — not an integer measure-tick at TPS=1000")]
    NonIntegerBeat { beat: i64, beat_den: u64 },
}

/// Output of parsing an SSQ file: the format-independent `Song` plus
/// SSQ-specific sidecar data needed to round-trip back to SSQ.
#[derive(Debug, Clone)]
pub struct SsqParseResult {
    pub song: Song,
    pub events: Vec<SsqEvent>,
    /// Raw `(time_offset, tempo_data)` pairs from the source tempo chunk.
    /// Threaded through unchanged so DDR→DDR writes can reproduce the
    /// tempo chunk byte-for-byte. SM5→DDR writes ignore this and
    /// synthesize pairs from the semantic `tempo_segments` + `stops`.
    pub raw_tempo_pairs: Vec<(i32, i32)>,
    /// Metadata for each auxiliary chunk (types 4/5/9/17) that was
    /// encountered and dropped. Callers typically log these at `warn`
    /// level with the source filename attached.
    pub aux_chunks_dropped: Vec<AuxMeta>,
}

/// Parse an SSQ file.
///
/// The returned `Song`'s audio is empty; the caller is expected to
/// populate it from a sibling XWB/WAVM file. Charts are also empty
/// until step-chunk parsing lands in a later task.
pub fn parse(bytes: &[u8]) -> Result<SsqParseResult, SsqError> {
    let mut reader = LeReader::new(bytes);
    let mut partial = PartialSong::new();

    while let Some((offset, header)) = read_header(&mut reader)? {
        let body_size = header.body_size() as usize;
        let body = reader.read_bytes(body_size).map_err(SsqError::Io)?;
        dispatch_chunk(&header, body, offset, &mut partial)?;
    }

    partial.finalize()
}

fn dispatch_chunk(
    header: &ChunkHeader,
    body: &[u8],
    offset: usize,
    partial: &mut PartialSong,
) -> Result<(), SsqError> {
    match header.ty {
        1 => {
            if partial.tempo_seen {
                return Err(SsqError::DuplicateTempoChunk);
            }
            let result = tempo::parse_tempo_chunk(header, body, offset)?;
            partial.apply_tempo(result);
            Ok(())
        }
        2 => {
            if partial.events_seen {
                return Err(SsqError::DuplicateEventsChunk);
            }
            partial.events = events::parse_events_chunk(header, body, offset)?;
            partial.events_seen = true;
            Ok(())
        }
        4 | 5 | 9 | 17 => {
            log::warn!(
                "dropping auxiliary chunk type {} at byte {offset} (size {})",
                header.ty,
                header.length
            );
            partial.aux_chunks_dropped.push(AuxMeta {
                ty: header.ty,
                offset,
                size: header.length,
            });
            Ok(())
        }
        3 => {
            let chart = steps::parse_steps_chunk(header, body, offset)?;
            partial.charts.push(chart);
            Ok(())
        }
        other => Err(SsqError::UnexpectedChunkType { ty: other, offset }),
    }
}

/// Fields accumulated as chunks are parsed. Finalization checks that
/// required chunks were seen and substitutes empty placeholders for
/// pieces that come from other file types (audio, charts).
#[derive(Debug)]
pub(crate) struct PartialSong {
    pub tempo_seen: bool,
    pub events_seen: bool,
    pub tps: u32,
    pub tempo_segments: Vec<TempoSegment>,
    pub stops: Vec<Stop>,
    pub audio_sync_offset_seconds: Rational,
    pub raw_tempo_pairs: Vec<(i32, i32)>,
    pub events: Vec<SsqEvent>,
    pub aux_chunks_dropped: Vec<AuxMeta>,
    pub charts: Vec<crate::model::Chart>,
}

impl PartialSong {
    fn new() -> Self {
        Self {
            tempo_seen: false,
            events_seen: false,
            tps: 0,
            tempo_segments: Vec::new(),
            stops: Vec::new(),
            audio_sync_offset_seconds: Rational::zero(),
            raw_tempo_pairs: Vec::new(),
            events: Vec::new(),
            aux_chunks_dropped: Vec::new(),
            charts: Vec::new(),
        }
    }

    fn apply_tempo(&mut self, result: tempo::TempoParseResult) {
        self.tempo_seen = true;
        self.tps = result.tps;
        self.tempo_segments = result.tempo_segments;
        self.stops = result.stops;
        self.audio_sync_offset_seconds = result.audio_sync_offset_seconds;
        self.raw_tempo_pairs = result.raw_pairs;
    }

    fn finalize(self) -> Result<SsqParseResult, SsqError> {
        if !self.tempo_seen {
            return Err(SsqError::MissingTempoChunk);
        }
        if !self.events_seen {
            return Err(SsqError::MissingEventsChunk);
        }
        let song = Song {
            title: None,
            artist: None,
            tps: self.tps,
            tempo_segments: self.tempo_segments,
            stops: self.stops,
            charts: self.charts,
            audio: AudioBuffer {
                samples: Vec::new(),
                sample_rate: 0,
                channels: 0,
            },
            audio_sync_offset_seconds: self.audio_sync_offset_seconds,
            preview: PreviewSlice::default_window(),
        };
        Ok(SsqParseResult {
            song,
            events: self.events,
            raw_tempo_pairs: self.raw_tempo_pairs,
            aux_chunks_dropped: self.aux_chunks_dropped,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build an SSQ fixture from a list of (type, param2, param3, body) chunks.
    /// Automatically pads each chunk body with zero bytes so the total chunk
    /// length is dword-aligned (as real SSQ chunks are).
    fn build_ssq(chunks: &[(u16, u16, u16, Vec<u8>)]) -> Vec<u8> {
        let mut out = Vec::new();
        for (ty, param2, param3, body) in chunks {
            let body_len = body.len();
            let pad = (4 - ((12 + body_len) % 4)) % 4;
            let length = (12 + body_len + pad) as u32;
            out.extend_from_slice(&length.to_le_bytes());
            out.extend_from_slice(&ty.to_le_bytes());
            out.extend_from_slice(&param2.to_le_bytes());
            out.extend_from_slice(&param3.to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes()); // param4
            out.extend_from_slice(body);
            out.extend_from_slice(&vec![0u8; pad]);
        }
        out.extend_from_slice(&0u32.to_le_bytes()); // terminator
        out
    }

    fn tempo_body(time_offsets: &[i32], tempo_data: &[i32]) -> Vec<u8> {
        let mut v = Vec::new();
        for t in time_offsets {
            v.extend_from_slice(&(*t as u32).to_le_bytes());
        }
        for t in tempo_data {
            v.extend_from_slice(&(*t as u32).to_le_bytes());
        }
        v
    }

    fn events_body(ticks: &[i32], codes: &[(u8, u8)]) -> Vec<u8> {
        assert_eq!(ticks.len(), codes.len());
        let mut v = Vec::new();
        for t in ticks {
            v.extend_from_slice(&(*t as u32).to_le_bytes());
        }
        for (c, a) in codes {
            v.push(*c);
            v.push(*a);
        }
        v
    }

    fn minimal_tempo_chunk() -> (u16, u16, u16, Vec<u8>) {
        (1, 1000, 2, tempo_body(&[0, 4096], &[0, 2000]))
    }

    fn minimal_events_chunk() -> (u16, u16, u16, Vec<u8>) {
        (2, 1, 1, events_body(&[0], &[(1, 4)]))
    }

    #[test]
    fn parses_tps_1000_ssq_with_tempo_and_events() {
        let bytes = build_ssq(&[minimal_tempo_chunk(), minimal_events_chunk()]);
        let result = parse(&bytes).unwrap();
        assert_eq!(result.song.tps, 1000);
        assert_eq!(result.song.tempo_segments.len(), 1);
        assert_eq!(result.song.stops.len(), 0);
        assert_eq!(result.song.charts.len(), 0);
        assert_eq!(result.events.len(), 1);
        assert!(result.aux_chunks_dropped.is_empty());
    }

    #[test]
    fn parses_tps_150_ssq() {
        let tempo = (1, 150, 2, tempo_body(&[0, 4096], &[0, 300]));
        let bytes = build_ssq(&[tempo, minimal_events_chunk()]);
        let result = parse(&bytes).unwrap();
        assert_eq!(result.song.tps, 150);
    }

    #[test]
    fn missing_tempo_chunk_is_rejected() {
        let bytes = build_ssq(&[minimal_events_chunk()]);
        let err = parse(&bytes).unwrap_err();
        assert!(matches!(err, SsqError::MissingTempoChunk));
    }

    #[test]
    fn missing_events_chunk_is_rejected() {
        let bytes = build_ssq(&[minimal_tempo_chunk()]);
        let err = parse(&bytes).unwrap_err();
        assert!(matches!(err, SsqError::MissingEventsChunk));
    }

    #[test]
    fn duplicate_tempo_chunk_is_rejected() {
        let bytes = build_ssq(&[minimal_tempo_chunk(), minimal_tempo_chunk()]);
        let err = parse(&bytes).unwrap_err();
        assert!(matches!(err, SsqError::DuplicateTempoChunk));
    }

    #[test]
    fn duplicate_events_chunk_is_rejected() {
        let bytes = build_ssq(&[
            minimal_tempo_chunk(),
            minimal_events_chunk(),
            minimal_events_chunk(),
        ]);
        let err = parse(&bytes).unwrap_err();
        assert!(matches!(err, SsqError::DuplicateEventsChunk));
    }

    #[test]
    fn empty_steps_chunk_produces_empty_chart() {
        // A Single Basic step chunk with zero entries.
        let bytes = build_ssq(&[
            minimal_tempo_chunk(),
            minimal_events_chunk(),
            (3, 0x0114, 0, vec![]),
        ]);
        let result = parse(&bytes).unwrap();
        assert_eq!(result.song.charts.len(), 1);
        assert_eq!(result.song.charts[0].notes.len(), 0);
    }

    #[test]
    fn aux_chunks_are_dropped_with_metadata() {
        // A TPS=150 file with a type-4 and type-5 aux chunk after events.
        let tempo = (1, 150, 2, tempo_body(&[0, 4096], &[0, 300]));
        let aux4 = (4, 1, 0, vec![]);
        let aux5 = (5, 0, 0, vec![]);
        let bytes = build_ssq(&[tempo, minimal_events_chunk(), aux4, aux5]);
        let result = parse(&bytes).unwrap();
        assert_eq!(result.aux_chunks_dropped.len(), 2);
        assert_eq!(result.aux_chunks_dropped[0].ty, 4);
        assert_eq!(result.aux_chunks_dropped[1].ty, 5);
    }

    #[test]
    fn ffff_param2_chunk_is_dispatched_not_skipped() {
        // Tempo + events, then a type-3 chunk with param2=0xFFFF.
        // The parser reads it normally and tries to decode the difficulty
        // code. 0xFFFF is not valid, so parse returns an error.
        let mut bytes = build_ssq(&[minimal_tempo_chunk(), minimal_events_chunk()]);
        bytes.truncate(bytes.len() - 4); // remove terminator
        let mut bad_chunk = Vec::new();
        bad_chunk.extend_from_slice(&12u32.to_le_bytes());
        bad_chunk.extend_from_slice(&3u16.to_le_bytes());
        bad_chunk.extend_from_slice(&0xFFFFu16.to_le_bytes());
        bad_chunk.extend_from_slice(&0u16.to_le_bytes());
        bad_chunk.extend_from_slice(&0u16.to_le_bytes());
        bytes.extend_from_slice(&bad_chunk);
        bytes.extend_from_slice(&0u32.to_le_bytes()); // terminator
        assert!(parse(&bytes).is_err());
    }
}
