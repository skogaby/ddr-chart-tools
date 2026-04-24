//! Modern-profile SSQ writer (TPS=1000, chunks 1/2/3 only).
//!
//! Refuses to emit anything else — the modern-subset restriction is
//! enforced at the writer boundary so legacy-profile SSQs cannot be
//! produced by this tool.

use std::io::Write;

use crate::model::{Beat, Chart, NoteKind, Rational, Song};

use super::events::SsqEvent;
use super::SsqError;

const MODERN_TPS: u32 = 1000;

/// Serialize a `Song` plus its preserved SSQ sidecar data into the
/// bytes of a modern-profile SSQ file.
///
/// `raw_tempo_pairs` is the `(time_offset, tempo_data)` list from
/// `SsqParseResult::raw_tempo_pairs`. When non-empty it is emitted
/// verbatim, so DDR→DDR writes reproduce the source tempo chunk
/// byte-for-byte. When empty (SM5→DDR path) the writer synthesizes the
/// pairs from the `Song`'s semantic `tempo_segments` + `stops`.
pub fn write(
    song: &Song,
    events: &[SsqEvent],
    raw_tempo_pairs: &[(i32, i32)],
    out: &mut impl Write,
) -> Result<(), SsqError> {
    if song.tps != MODERN_TPS {
        return Err(SsqError::CannotWriteTps { tps: song.tps });
    }

    write_tempo_chunk(song, raw_tempo_pairs, out)?;
    write_events_chunk(events, out)?;
    for chart in &song.charts {
        write_steps_chunk(chart, out)?;
    }
    // File terminator — a chunk length of 0.
    out.write_all(&0u32.to_le_bytes()).map_err(io_err)?;
    Ok(())
}

fn io_err(err: std::io::Error) -> SsqError {
    SsqError::Write(err.to_string())
}

/// Write a chunk header (12 bytes) given its body length and type params.
fn write_chunk_header(
    out: &mut impl Write,
    body_len: usize,
    ty: u16,
    param2: u16,
    param3: u16,
) -> Result<(), SsqError> {
    let length = (12 + body_len) as u32;
    out.write_all(&length.to_le_bytes()).map_err(io_err)?;
    out.write_all(&ty.to_le_bytes()).map_err(io_err)?;
    out.write_all(&param2.to_le_bytes()).map_err(io_err)?;
    out.write_all(&param3.to_le_bytes()).map_err(io_err)?;
    out.write_all(&0u16.to_le_bytes()).map_err(io_err)?; // param4 always 0
    Ok(())
}

/// Convert a `Beat` to an integer measure-tick count (1024 ticks per
/// beat). Rounds to nearest when the beat doesn't land exactly on a
/// tick boundary (e.g. triplets at 1/3 beat = 341.33 ticks).
fn beat_to_measure_ticks(beat: Beat) -> Result<i32, SsqError> {
    let r = beat.as_rational();
    let num = (r.num() as i128)
        .checked_mul(1024)
        .ok_or_else(|| SsqError::Write("overflow converting beat to measure ticks".to_string()))?;
    let den = r.den() as i128;
    let half = if num >= 0 { den / 2 } else { -(den / 2) };
    let rounded = (num + half) / den;
    i32::try_from(rounded)
        .map_err(|_| SsqError::Write("measure-tick out of i32 range".to_string()))
}

fn write_tempo_chunk(
    song: &Song,
    raw_pairs: &[(i32, i32)],
    out: &mut impl Write,
) -> Result<(), SsqError> {
    let entries: Vec<(i32, i32)> = if !raw_pairs.is_empty() {
        raw_pairs.to_vec()
    } else {
        synthesize_tempo_entries(song)?
    };

    let body_len = entries.len() * 8;
    write_chunk_header(out, body_len, 1, MODERN_TPS as u16, entries.len() as u16)?;
    for (time, _) in &entries {
        out.write_all(&(*time as u32).to_le_bytes())
            .map_err(io_err)?;
    }
    for (_, td) in &entries {
        out.write_all(&(*td as u32).to_le_bytes()).map_err(io_err)?;
    }
    Ok(())
}

/// Synthesize tempo entries from the model's semantic view (used for
/// SM5→DDR where no raw pairs are available). Requires at least one
/// tempo segment. The trailing entry is synthesized at the maximum
/// chart beat so BPM is derivable between every pair.
fn synthesize_tempo_entries(song: &Song) -> Result<Vec<(i32, i32)>, SsqError> {
    if song.tempo_segments.is_empty() {
        return Err(SsqError::Write(
            "cannot synthesize tempo chunk: no tempo segments".to_string(),
        ));
    }

    let tps = Rational::from_integer(MODERN_TPS as i64);
    let sixty = Rational::from_integer(60);

    let mut entries: Vec<(i32, i32)> = Vec::new();
    let mut cur_seconds_ticks = song
        .audio_sync_offset_seconds
        .mul(&tps)
        .map_err(|e| SsqError::Write(format!("sync overflow: {e}")))?;

    let mut prev_beat = Beat::zero();
    let mut prev_bpm: Option<crate::model::Bpm> = None;

    // First entry: (0, td0).
    entries.push((0, rational_to_i32(&cur_seconds_ticks)?));

    // Build a sorted timeline: each segment boundary + each stop.
    enum Ev<'a> {
        Segment(&'a crate::model::TempoSegment),
        Stop(&'a crate::model::Stop),
    }
    let mut timeline: Vec<Ev> = Vec::new();
    for seg in &song.tempo_segments {
        timeline.push(Ev::Segment(seg));
    }
    for stop in &song.stops {
        timeline.push(Ev::Stop(stop));
    }
    timeline.sort_by_key(|e| match e {
        Ev::Segment(s) => s.start_beat,
        Ev::Stop(s) => s.at_beat,
    });

    for e in timeline {
        match e {
            Ev::Segment(seg) => {
                if seg.start_beat == Beat::zero() {
                    // Index 0 already emitted; just set the current BPM.
                    prev_bpm = Some(seg.bpm);
                    continue;
                }
                if let Some(bpm) = prev_bpm {
                    cur_seconds_ticks = advance_seconds_ticks(
                        cur_seconds_ticks,
                        prev_beat,
                        seg.start_beat,
                        bpm,
                        &tps,
                        &sixty,
                    )?;
                }
                let tick = beat_to_measure_ticks(seg.start_beat)?;
                entries.push((tick, rational_to_i32(&cur_seconds_ticks)?));
                prev_beat = seg.start_beat;
                prev_bpm = Some(seg.bpm);
            }
            Ev::Stop(stop) => {
                if let Some(bpm) = prev_bpm {
                    cur_seconds_ticks = advance_seconds_ticks(
                        cur_seconds_ticks,
                        prev_beat,
                        stop.at_beat,
                        bpm,
                        &tps,
                        &sixty,
                    )?;
                }
                let tick = beat_to_measure_ticks(stop.at_beat)?;
                entries.push((tick, rational_to_i32(&cur_seconds_ticks)?));
                cur_seconds_ticks = cur_seconds_ticks
                    .add(
                        &stop
                            .duration_seconds
                            .mul(&tps)
                            .map_err(|e| SsqError::Write(format!("stop overflow: {e}")))?,
                    )
                    .map_err(|e| SsqError::Write(format!("stop add overflow: {e}")))?;
                entries.push((tick, rational_to_i32(&cur_seconds_ticks)?));
                prev_beat = stop.at_beat;
            }
        }
    }

    // Synthesize trailing entry at max chart beat if we still need one
    // to make the last segment's BPM derivable.
    if entries.len() < 2 {
        let end_beat = max_chart_beat(song).unwrap_or(Beat::from_measure_ticks(4096).unwrap());
        if let Some(bpm) = prev_bpm {
            cur_seconds_ticks =
                advance_seconds_ticks(cur_seconds_ticks, prev_beat, end_beat, bpm, &tps, &sixty)?;
        }
        let tick = beat_to_measure_ticks(end_beat)?;
        entries.push((tick, rational_to_i32(&cur_seconds_ticks)?));
    }

    Ok(entries)
}

fn max_chart_beat(song: &Song) -> Option<Beat> {
    song.charts
        .iter()
        .flat_map(|c| c.notes.iter())
        .filter_map(|n| match n.kind {
            NoteKind::HoldHead { length } => {
                let end = n.beat.as_rational().add(&length.as_rational()).ok()?;
                Some(Beat::from_rational(end))
            }
            _ => Some(n.beat),
        })
        .max()
}

fn advance_seconds_ticks(
    cur: Rational,
    from_beat: Beat,
    to_beat: Beat,
    bpm: crate::model::Bpm,
    tps: &Rational,
    sixty: &Rational,
) -> Result<Rational, SsqError> {
    let delta_beats = to_beat
        .as_rational()
        .sub(&from_beat.as_rational())
        .map_err(|e| SsqError::Write(format!("beat delta: {e}")))?;
    let seconds = delta_beats
        .div(&bpm.as_rational())
        .map_err(|e| SsqError::Write(format!("div bpm: {e}")))?
        .mul(sixty)
        .map_err(|e| SsqError::Write(format!("mul 60: {e}")))?;
    let st = seconds
        .mul(tps)
        .map_err(|e| SsqError::Write(format!("mul tps: {e}")))?;
    cur.add(&st)
        .map_err(|e| SsqError::Write(format!("cumulative add: {e}")))
}

/// Round a Rational to the nearest i32 (half-away-from-zero).
///
/// Tempo synthesis from SM5 data often produces fractional seconds-ticks
/// (e.g. BPM=168 → 2500/7 ticks per beat). Sub-tick rounding at
/// TPS=1000 is ±0.5ms — well within acceptable precision.
fn rational_to_i32(r: &Rational) -> Result<i32, SsqError> {
    let num = r.num() as i128;
    let den = r.den() as i128;
    let half = if num >= 0 { den / 2 } else { -(den / 2) };
    let rounded = (num + half) / den;
    i32::try_from(rounded).map_err(|_| SsqError::Write("i32 out of range".to_string()))
}

fn write_events_chunk(events: &[SsqEvent], out: &mut impl Write) -> Result<(), SsqError> {
    let n = events.len();
    let body_unpadded = 6 * n;
    let pad = (4 - ((12 + body_unpadded) % 4)) % 4;
    let body_len = body_unpadded + pad;

    write_chunk_header(out, body_len, 2, 1, n as u16)?;
    for e in events {
        out.write_all(&(e.tick as u32).to_le_bytes())
            .map_err(io_err)?;
    }
    for e in events {
        out.write_all(&[e.code, e.arg]).map_err(io_err)?;
    }
    for _ in 0..pad {
        out.write_all(&[0u8]).map_err(io_err)?;
    }
    Ok(())
}

fn write_steps_chunk(chart: &Chart, out: &mut impl Write) -> Result<(), SsqError> {
    let (time_offsets, step_bytes, freeze_entries) = emit_steps_and_freezes(chart)?;

    let n = time_offsets.len();
    let step_pad = n % 2; // 1 byte if N is odd
    let body_unpadded = 4 * n + n + step_pad + 2 * freeze_entries.len();
    let dword_pad = (4 - ((12 + body_unpadded) % 4)) % 4;
    let body_len = body_unpadded + dword_pad;

    let param2 = difficulty_code(chart.style, chart.difficulty);
    write_chunk_header(out, body_len, 3, param2, n as u16)?;

    for t in &time_offsets {
        out.write_all(&(*t as u32).to_le_bytes()).map_err(io_err)?;
    }
    out.write_all(&step_bytes).map_err(io_err)?;
    for _ in 0..step_pad {
        out.write_all(&[0u8]).map_err(io_err)?;
    }
    for (panels, kind) in &freeze_entries {
        out.write_all(&[*panels, *kind]).map_err(io_err)?;
    }
    for _ in 0..dword_pad {
        out.write_all(&[0u8]).map_err(io_err)?;
    }
    Ok(())
}

/// Convert notes into parallel (time_offsets, step_bytes, freeze_entries)
/// arrays. Hold heads produce two step entries (head + freeze-end) and
/// one freeze entry.
/// Output shape of `emit_steps_and_freezes`: parallel arrays of time
/// offsets, step bytes, and `(panels, kind)` freeze entries.
type StepsBody = (Vec<i32>, Vec<u8>, Vec<(u8, u8)>);

/// Convert notes into parallel time-offset / step-byte / freeze-entry
/// arrays. Hold heads produce two step entries (head + freeze-end) and
/// one freeze entry.
fn emit_steps_and_freezes(chart: &Chart) -> Result<StepsBody, SsqError> {
    use crate::model::ShockSide;

    // Intermediate: (tick, step_byte), plus a parallel list of freeze-end entries.
    let mut rows: Vec<(i32, u8)> = Vec::new();
    let mut freezes: Vec<(i32, u8)> = Vec::new(); // (end_tick, panel_mask)

    for note in &chart.notes {
        let head_tick = beat_to_measure_ticks(note.beat)?;
        match note.kind {
            NoteKind::Tap => {
                rows.push((head_tick, note.panels.bits()));
            }
            NoteKind::Shock { side } => {
                let byte = match side {
                    ShockSide::BothSides => 0xFFu8,
                    ShockSide::P1Only => 0x0F,
                    ShockSide::P2Only => 0xF0,
                };
                rows.push((head_tick, byte));
            }
            NoteKind::HoldHead { length } => {
                rows.push((head_tick, note.panels.bits()));
                let end_beat = Beat::from_rational(
                    note.beat
                        .as_rational()
                        .add(&length.as_rational())
                        .map_err(|e| SsqError::Write(format!("hold length: {e}")))?,
                );
                let end_tick = beat_to_measure_ticks(end_beat)?;
                freezes.push((end_tick, note.panels.bits()));
            }
        }
    }

    // Merge freezes into rows as 0x00 step bytes (in time order, stable
    // across ties).
    rows.sort_by_key(|(t, _)| *t);
    freezes.sort_by_key(|(t, _)| *t);

    // Walk both lists in time order, emitting a combined ordered stream.
    // When a freeze-end tick equals a row tick, the 0x00 step comes
    // AFTER the note at that tick (the parser walks freezes in file
    // order, so ordering matters for freeze matching).
    let mut merged: Vec<(i32, u8)> = Vec::with_capacity(rows.len() + freezes.len());
    let mut freeze_entries: Vec<(u8, u8)> = Vec::with_capacity(freezes.len());
    let mut ri = 0usize;
    let mut fi = 0usize;
    while ri < rows.len() || fi < freezes.len() {
        let take_row = match (rows.get(ri), freezes.get(fi)) {
            (Some((rt, _)), Some((ft, _))) => rt <= ft,
            (Some(_), None) => true,
            (None, Some(_)) => false,
            (None, None) => break,
        };
        if take_row {
            merged.push(rows[ri]);
            ri += 1;
        } else {
            merged.push((freezes[fi].0, 0x00));
            freeze_entries.push((freezes[fi].1, 0x01));
            fi += 1;
        }
    }

    let time_offsets: Vec<i32> = merged.iter().map(|(t, _)| *t).collect();
    let step_bytes: Vec<u8> = merged.iter().map(|(_, b)| *b).collect();
    Ok((time_offsets, step_bytes, freeze_entries))
}

fn difficulty_code(style: crate::model::Style, difficulty: crate::model::Difficulty) -> u16 {
    use crate::model::{Difficulty, Style};
    let style_byte: u16 = match style {
        Style::Single => 0x14,
        Style::Double => 0x18,
    };
    let slot_byte: u16 = match difficulty {
        Difficulty::Basic => 0x01,
        Difficulty::Difficult => 0x02,
        Difficulty::Expert => 0x03,
        Difficulty::Beginner => 0x04,
        Difficulty::Challenge => 0x06,
    };
    (slot_byte << 8) | style_byte
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AudioBuffer, Bpm, Difficulty, Note, PanelSet, PreviewSlice, Style};
    use crate::ssq::parse;

    fn empty_song(tps: u32) -> Song {
        Song {
            title: None,
            artist: None,
            tps,
            tempo_segments: Vec::new(),
            stops: Vec::new(),
            charts: Vec::new(),
            audio: AudioBuffer {
                samples: Vec::new(),
                sample_rate: 0,
                channels: 0,
            },
            audio_sync_offset_seconds: Rational::zero(),
            preview: PreviewSlice::default_window(),
        }
    }

    #[test]
    fn refuses_non_modern_tps() {
        let song = empty_song(150);
        let mut out = Vec::new();
        let err = write(&song, &[], &[], &mut out).unwrap_err();
        assert!(matches!(err, SsqError::CannotWriteTps { tps: 150 }));
    }

    #[test]
    fn round_trip_minimal_ssq() {
        // Build a complete minimal SSQ: tempo + events + one empty chart.
        let mut song = empty_song(1000);
        song.tempo_segments.push(crate::model::TempoSegment {
            start_beat: Beat::zero(),
            bpm: Bpm::from_rational(Rational::from_integer(120)),
        });
        song.charts.push(Chart {
            style: Style::Single,
            difficulty: Difficulty::Basic,
            notes: Vec::new(),
        });
        let events = vec![SsqEvent {
            tick: 0,
            code: 1,
            arg: 4,
        }];

        let mut bytes = Vec::new();
        write(&song, &events, &[], &mut bytes).unwrap();

        let parsed = parse(&bytes).unwrap();
        assert_eq!(parsed.song.tps, 1000);
        assert_eq!(parsed.song.charts.len(), 1);
        assert_eq!(parsed.events.len(), 1);
        assert_eq!(parsed.events[0].code, 1);
    }

    #[test]
    fn round_trip_with_tap_note() {
        let mut song = empty_song(1000);
        song.tempo_segments.push(crate::model::TempoSegment {
            start_beat: Beat::zero(),
            bpm: Bpm::from_rational(Rational::from_integer(120)),
        });
        song.charts.push(Chart {
            style: Style::Single,
            difficulty: Difficulty::Basic,
            notes: vec![Note {
                beat: Beat::from_measure_ticks(1024).unwrap(),
                kind: NoteKind::Tap,
                panels: PanelSet::from_bits(Style::Single, 0x05),
            }],
        });
        let events = vec![SsqEvent {
            tick: 0,
            code: 2,
            arg: 1,
        }];

        let mut bytes = Vec::new();
        write(&song, &events, &[], &mut bytes).unwrap();

        let parsed = parse(&bytes).unwrap();
        assert_eq!(parsed.song.charts.len(), 1);
        assert_eq!(parsed.song.charts[0].notes.len(), 1);
        assert_eq!(parsed.song.charts[0].notes[0].panels.bits(), 0x05);
        assert_eq!(parsed.song.charts[0].notes[0].kind, NoteKind::Tap);
    }

    #[test]
    fn round_trip_with_hold_head() {
        let mut song = empty_song(1000);
        song.tempo_segments.push(crate::model::TempoSegment {
            start_beat: Beat::zero(),
            bpm: Bpm::from_rational(Rational::from_integer(120)),
        });
        song.charts.push(Chart {
            style: Style::Single,
            difficulty: Difficulty::Expert,
            notes: vec![Note {
                beat: Beat::from_measure_ticks(1024).unwrap(),
                kind: NoteKind::HoldHead {
                    length: Beat::from_measure_ticks(1024).unwrap(),
                },
                panels: PanelSet::from_bits(Style::Single, 0x04),
            }],
        });
        let events = vec![SsqEvent {
            tick: 0,
            code: 2,
            arg: 1,
        }];

        let mut bytes = Vec::new();
        write(&song, &events, &[], &mut bytes).unwrap();

        let parsed = parse(&bytes).unwrap();
        let n = &parsed.song.charts[0].notes[0];
        assert_eq!(n.panels.bits(), 0x04);
        match n.kind {
            NoteKind::HoldHead { length } => {
                assert_eq!(length, Beat::from_measure_ticks(1024).unwrap());
            }
            _ => panic!("expected HoldHead, got {:?}", n.kind),
        }
    }

    #[test]
    fn raw_tempo_pairs_roundtrip_byte_exact() {
        // When raw_pairs are provided, they must be emitted verbatim
        // (no recomputation from semantic view).
        let song = {
            let mut s = empty_song(1000);
            s.tempo_segments.push(crate::model::TempoSegment {
                start_beat: Beat::zero(),
                bpm: Bpm::from_rational(Rational::from_integer(120)),
            });
            s.charts.push(Chart {
                style: Style::Single,
                difficulty: Difficulty::Basic,
                notes: Vec::new(),
            });
            s
        };
        let raw = vec![(0, 42), (4096, 2042)]; // arbitrary but well-formed
        let events = vec![SsqEvent {
            tick: 0,
            code: 1,
            arg: 4,
        }];
        let mut bytes = Vec::new();
        write(&song, &events, &raw, &mut bytes).unwrap();
        let parsed = parse(&bytes).unwrap();
        assert_eq!(parsed.raw_tempo_pairs, raw);
    }
}
