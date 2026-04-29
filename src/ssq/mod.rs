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

pub mod auxiliary;
pub mod chunk;
pub mod events;
pub mod mines;
pub mod steps;
pub mod tempo;
pub mod writer;

use thiserror::Error;

use crate::model::{AudioBuffer, PreviewSlice, Rational, Song, Stop, TempoSegment};
use crate::util::io::{IoError, LeReader};

use auxiliary::AuxMeta;
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

    #[error("mine chunk at byte {offset}: declared length {declared} does not match entry count {param3} (expected {expected})")]
    MineChunkLengthMismatch {
        offset: usize,
        declared: u32,
        param3: u16,
        expected: u32,
    },

    #[error("mines::write_chunk refuses to emit shock-mask panels 0x{panels:02X} (reserved for step-chunk shocks per spec §3.2)")]
    InvalidMinePanels { panels: u8 },
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
        20 => {
            // MINE_DATA chunk (`docs/ssq_mine_chunk_format.md`).
            // `parse_chunk` returns `None` on header-level failures
            // (length mismatch, param2 invalid) after logging a
            // `warn!`; in that case we skip this chunk and continue
            // parsing the rest of the file. On success, collect the
            // chunk into `pending_mine_chunks` — final attachment to
            // charts happens at `finalize` time so that charts with
            // higher byte offsets than their paired MINE_DATA chunk
            // still get their mines attached correctly.
            if let Some((param2, notes)) = mines::parse_chunk(header, body, offset) {
                partial.pending_mine_chunks.push((param2, notes));
            }
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
    /// Parsed kind-20 mine chunks in insertion order, keyed by their
    /// `param2` difficulty code (see `docs/ssq_mine_chunk_format.md`
    /// §2.1). Drained by `finalize` into the matching chart's
    /// `notes` list, with duplicate `(type=20, param2=X)` chunks
    /// warned-and-dropped per spec §2.2.
    pub pending_mine_chunks: Vec<(u16, Vec<crate::model::Note>)>,
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
            pending_mine_chunks: Vec::new(),
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

    fn finalize(mut self) -> Result<SsqParseResult, SsqError> {
        if !self.tempo_seen {
            return Err(SsqError::MissingTempoChunk);
        }
        if !self.events_seen {
            return Err(SsqError::MissingEventsChunk);
        }

        // Attach parsed MINE_DATA chunks to their matching charts by
        // difficulty code (`docs/ssq_mine_chunk_format.md §2.1`).
        // Walk in insertion order so duplicate-detection is
        // deterministic (first chunk wins, subsequent duplicates
        // warn+drop). A chunk whose `param2` matches no chart is an
        // orphan: warn+drop per spec §2.2.
        attach_mine_chunks(&mut self.charts, &self.pending_mine_chunks);

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

/// Attach parsed MINE_DATA chunks to their matching charts by
/// difficulty code, following the rules in
/// `docs/ssq_mine_chunk_format.md §2`:
///
/// - Walks `pending` in insertion order.
/// - **Match found, first chunk for this chart** → clone each note
///   into the chart's `notes` list, re-mask `panels` via
///   [`PanelSet::from_bits`] (idempotent on valid inputs — defensive
///   against hand-built `Note`s that bypass [`mines::parse_chunk`]'s
///   validation), then stable-sort the chart's notes by `beat`.
/// - **Match found, subsequent chunk for same chart** → duplicate
///   `(type=20, param2=X)` pair; spec §2.2 says the DLL stops at
///   the first match, so the tool mirrors that: warn and discard.
/// - **No matching chart** → orphan chunk; warn and discard.
///
/// This function only emits `warn!` on the discard paths; successful
/// attachments are silent because the chart's note count is already
/// visible at `debug!` level elsewhere in the pipeline.
fn attach_mine_chunks(
    charts: &mut [crate::model::Chart],
    pending: &[(u16, Vec<crate::model::Note>)],
) {
    use std::collections::HashSet;

    let mut charts_with_mines: HashSet<usize> = HashSet::new();

    for (param2, notes) in pending {
        let match_idx = charts
            .iter()
            .position(|c| writer::difficulty_code(c.style, c.difficulty) == *param2);
        match match_idx {
            None => log::warn!(
                "orphan mine chunk param2=0x{param2:04X}: no step chunk with this difficulty code; discarding {} entries",
                notes.len()
            ),
            Some(idx) if charts_with_mines.contains(&idx) => log::warn!(
                "duplicate mine chunk param2=0x{param2:04X}: already attached a chunk to this chart; discarding {} entries from the subsequent chunk (spec §2.2)",
                notes.len()
            ),
            Some(idx) => {
                let chart = &mut charts[idx];
                for note in notes {
                    let mut cloned = note.clone();
                    cloned.panels = crate::model::PanelSet::from_bits(chart.style, note.panels.bits());
                    chart.notes.push(cloned);
                }
                chart.notes.sort_by_key(|n| n.beat);
                charts_with_mines.insert(idx);
            }
        }
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

    // ---------- Mine wiring tests (Task 3) ----------
    // These cover US-6 (no-mine baseline preserved) and the parser's
    // orphan/duplicate-detection paths in `attach_mine_chunks`.

    /// Build a complete minimal Song with the given charts, ready to
    /// be fed to `writer::write`. One tempo segment at 120 BPM,
    /// TPS=1000, no stops, empty audio.
    fn song_with_charts(charts: Vec<crate::model::Chart>) -> crate::model::Song {
        use crate::model::{AudioBuffer, Bpm, PreviewSlice, Song, TempoSegment};
        Song {
            title: None,
            artist: None,
            tps: 1000,
            tempo_segments: vec![TempoSegment {
                start_beat: crate::model::Beat::zero(),
                bpm: Bpm::from_rational(Rational::from_integer(120)),
            }],
            stops: Vec::new(),
            charts,
            audio: AudioBuffer {
                samples: Vec::new(),
                sample_rate: 0,
                channels: 0,
            },
            audio_sync_offset_seconds: Rational::zero(),
            preview: PreviewSlice::default_window(),
        }
    }

    /// Build an 8-byte mine entry (for synthetic orphan/duplicate bodies).
    fn mine_entry_bytes(beat_count: i32, panels: u8) -> Vec<u8> {
        let mut v = Vec::with_capacity(8);
        v.extend_from_slice(&(beat_count as u32).to_le_bytes());
        v.push(panels);
        v.push(0); // flags
        v.extend_from_slice(&0u16.to_le_bytes()); // reserved
        v
    }

    /// Build a MINE_DATA chunk body from `(beat_count, panels)` tuples.
    fn mine_body(entries: &[(i32, u8)]) -> Vec<u8> {
        let mut body = Vec::with_capacity(entries.len() * 8);
        for (beat, panels) in entries {
            body.extend_from_slice(&mine_entry_bytes(*beat, *panels));
        }
        body
    }

    /// Construct a chart with just `NoteKind::Mine` notes at the
    /// given `(beat_tick, panels)` positions.
    fn mine_only_chart(
        style: crate::model::Style,
        difficulty: crate::model::Difficulty,
        mines: &[(i32, u8)],
    ) -> crate::model::Chart {
        use crate::model::{Beat, Chart, Note, NoteKind, PanelSet};
        let notes = mines
            .iter()
            .map(|(tick, panels)| Note {
                beat: Beat::from_measure_ticks(i64::from(*tick)).unwrap(),
                kind: NoteKind::Mine,
                panels: PanelSet::from_bits(style, *panels),
            })
            .collect();
        Chart {
            style,
            difficulty,
            notes,
        }
    }

    #[test]
    fn ddr_to_ddr_no_mine_baseline_is_byte_identical() {
        // A chart with one Tap and zero mines must produce
        // byte-identical output under write → parse → write.
        // Guards US-6: the writer's new per-chart mines loop
        // emits nothing for mine-free charts, so the output
        // byte sequence is unchanged from pre-feature behavior.
        use crate::model::{Beat, Chart, Difficulty, Note, NoteKind, PanelSet, Style};

        let chart = Chart {
            style: Style::Single,
            difficulty: Difficulty::Basic,
            notes: vec![Note {
                beat: Beat::from_measure_ticks(1024).unwrap(),
                kind: NoteKind::Tap,
                panels: PanelSet::from_bits(Style::Single, 0x01),
            }],
        };
        let song = song_with_charts(vec![chart]);
        let events = vec![events::SsqEvent {
            tick: 0,
            code: 1,
            arg: 4,
        }];
        let raw_pairs = vec![(0, 0), (4096, 2000)];

        let mut bytes1 = Vec::new();
        writer::write(&song, &events, &raw_pairs, &mut bytes1).unwrap();

        let parsed = parse(&bytes1).unwrap();

        let mut bytes2 = Vec::new();
        writer::write(
            &parsed.song,
            &parsed.events,
            &parsed.raw_tempo_pairs,
            &mut bytes2,
        )
        .unwrap();

        assert_eq!(
            bytes1, bytes2,
            "no-mine round-trip must produce byte-identical output"
        );
    }

    #[test]
    fn ddr_to_ddr_per_difficulty_mines_round_trip() {
        // Two charts with distinct mine patterns at different
        // difficulties. Write → parse → assert mines attach to
        // the correct chart by `param2`, then write again and
        // confirm byte-equality.
        use crate::model::{Difficulty, NoteKind, Style};

        let basic = mine_only_chart(
            Style::Single,
            Difficulty::Basic,
            &[(1024, 0x01), (2048, 0x04)],
        );
        let expert = mine_only_chart(
            Style::Double,
            Difficulty::Expert,
            &[(0, 0x11), (3072, 0x88)],
        );
        let song = song_with_charts(vec![basic, expert]);
        let events = vec![events::SsqEvent {
            tick: 0,
            code: 1,
            arg: 4,
        }];
        let raw_pairs = vec![(0, 0), (4096, 2000)];

        let mut bytes1 = Vec::new();
        writer::write(&song, &events, &raw_pairs, &mut bytes1).unwrap();

        let parsed = parse(&bytes1).unwrap();
        assert_eq!(parsed.song.charts.len(), 2);

        // First chart (Single Basic) received its 2 mines.
        let basic_parsed = &parsed.song.charts[0];
        assert_eq!(basic_parsed.style, Style::Single);
        assert_eq!(basic_parsed.difficulty, Difficulty::Basic);
        assert_eq!(basic_parsed.notes.len(), 2);
        for n in &basic_parsed.notes {
            assert_eq!(n.kind, NoteKind::Mine);
        }

        // Second chart (Double Expert) received its 2 mines.
        let expert_parsed = &parsed.song.charts[1];
        assert_eq!(expert_parsed.style, Style::Double);
        assert_eq!(expert_parsed.difficulty, Difficulty::Expert);
        assert_eq!(expert_parsed.notes.len(), 2);
        for n in &expert_parsed.notes {
            assert_eq!(n.kind, NoteKind::Mine);
        }

        // Write again — bytes must match the first write.
        let mut bytes2 = Vec::new();
        writer::write(
            &parsed.song,
            &parsed.events,
            &parsed.raw_tempo_pairs,
            &mut bytes2,
        )
        .unwrap();
        assert_eq!(bytes1, bytes2, "per-difficulty mine round-trip byte-equal");
    }

    #[test]
    fn ddr_orphan_mine_chunk_is_warned_and_dropped() {
        // A MINE_DATA chunk whose `param2` is `0x0318` (Double
        // Expert) but the file has only a Single Basic step chunk.
        // The parser must succeed, the Single Basic chart must have
        // no mine notes, and a warn is logged (not asserted here —
        // formal warn coverage is Task 5).
        use crate::model::NoteKind;

        let mine_chunk = (20, 0x0318, 1, mine_body(&[(1024, 0x04)]));
        let bytes = build_ssq(&[
            minimal_tempo_chunk(),
            minimal_events_chunk(),
            (3, 0x0114, 0, vec![]), // Single Basic, empty
            mine_chunk,
        ]);

        let result = parse(&bytes).unwrap();
        assert_eq!(result.song.charts.len(), 1);
        assert_eq!(
            result.song.charts[0].notes.len(),
            0,
            "orphan mine chunk must not attach to the unrelated chart"
        );
        // Sanity: no note of any kind sneaked through.
        for n in &result.song.charts[0].notes {
            assert_ne!(n.kind, NoteKind::Mine);
        }
    }

    #[test]
    fn ddr_duplicate_mine_chunk_keeps_first_drops_second() {
        // Two MINE_DATA chunks with the same `param2` (0x0114 =
        // Single Basic). The parser keeps the first chunk's mines
        // and drops the second with a warn (spec §2.2: DLL stops
        // at first match).
        use crate::model::NoteKind;

        // First chunk: one mine at beat 1024, panel 0x01.
        let chunk_first = (20, 0x0114, 1, mine_body(&[(1024, 0x01)]));
        // Second chunk: a different mine at beat 2048, panel 0x04.
        // If the second chunk were attached, the chart would have
        // two mines; we assert it has only the first chunk's one.
        let chunk_second = (20, 0x0114, 1, mine_body(&[(2048, 0x04)]));
        let bytes = build_ssq(&[
            minimal_tempo_chunk(),
            minimal_events_chunk(),
            (3, 0x0114, 0, vec![]), // Single Basic, empty
            chunk_first,
            chunk_second,
        ]);

        let result = parse(&bytes).unwrap();
        assert_eq!(result.song.charts.len(), 1);
        let chart = &result.song.charts[0];
        // Only the first chunk's mine should be attached.
        let mine_count = chart
            .notes
            .iter()
            .filter(|n| matches!(n.kind, NoteKind::Mine))
            .count();
        assert_eq!(mine_count, 1, "duplicate chunk must be dropped");
        // And it should be the first chunk's mine: tick 1024 panel 0x01.
        let first_mine = chart
            .notes
            .iter()
            .find(|n| matches!(n.kind, NoteKind::Mine))
            .unwrap();
        assert_eq!(first_mine.panels.bits(), 0x01);
        assert_eq!(
            first_mine.beat,
            crate::model::Beat::from_measure_ticks(1024).unwrap()
        );
    }
}
