//! Modern-profile SSQ writer (TPS=1000, chunks 1/2/3 plus 20 for mines).
//!
//! Refuses to emit anything else — the modern-subset restriction is
//! enforced at the writer boundary so legacy-profile SSQs cannot be
//! produced by this tool.
//!
//! Also owns tempo-chunk synthesis for sources with no raw tempo pairs
//! (SM5): `#BPMS`/`#STOPS` become `(measure_tick, seconds_tick)` anchors,
//! accumulated in fixed point rather than exact rationals because
//! arbitrary decimal BPMs make an exact running sum's denominator
//! overflow (see `synthesize_tempo_entries_until`).

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
    // MINE_DATA chunks are per-difficulty (spec §2.1) and live
    // between the last step chunk and the file terminator.
    // `mines::write_chunk` emits nothing for charts with no
    // `NoteKind::Mine` notes, so mine-free songs produce
    // byte-identical output to the pre-mines-feature writer.
    for chart in &song.charts {
        super::mines::write_chunk(chart, out)?;
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
        .checked_mul(Beat::TICKS_PER_BEAT as i128)
        .ok_or_else(|| SsqError::Write("overflow converting beat to measure ticks".to_string()))?;
    i32::try_from(div_round(num, r.den() as i128))
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
/// tempo segment. The trailing entry is placed at the last chart beat
/// (see [`synthesize_tempo_entries_until`] for why one is always
/// needed); songs with no notes get a trailing entry one measure in.
pub fn synthesize_tempo_entries(song: &Song) -> Result<Vec<(i32, i32)>, SsqError> {
    synthesize_tempo_entries_until(song, max_chart_beat(song))
}

/// Synthesize tempo entries from the model's semantic view, with the
/// trailing entry placed at `end_beat`.
///
/// SSQ has no explicit BPM field: the tempo of a segment is the slope
/// between consecutive `(measure_tick, seconds_tick)` entries. The final
/// `#BPMS` entry therefore only exists in the output if an entry follows
/// it, so a trailing entry is emitted at `end_beat` whenever that lies
/// past the last segment boundary or stop. Without it, a downstream
/// extrapolation (e.g. the FINISH/END guard in the job layer) would have
/// nothing but the *previous* segment's slope to extend, and the chart
/// would run at the wrong tempo from the last BPM change to the end.
///
/// `end_beat` is `None` when the caller has no chart to derive it from;
/// a trailing entry is then only added when the list would otherwise be
/// a single entry (one measure past beat 0), so a bare tempo map still
/// yields a derivable BPM.
///
/// Callers that need extra trailing entries should extend the returned
/// vec and pass the result back into [`write`] as `raw_tempo_pairs`.
pub fn synthesize_tempo_entries_until(
    song: &Song,
    end_beat: Option<Beat>,
) -> Result<Vec<(i32, i32)>, SsqError> {
    let Some(first_segment) = song.tempo_segments.iter().min_by_key(|s| s.start_beat) else {
        return Err(SsqError::Write(
            "cannot synthesize tempo chunk: no tempo segments".to_string(),
        ));
    };

    // Cumulative audio time in sub-ticks (see `SUBTICKS_PER_TICK`). An
    // exact `Rational` accumulator was tried first and overflowed: each
    // segment contributes `Δbeats × 60000 / bpm`, and a `#BPMS` value
    // like `249.999985` (= 49999997/200000) puts a large prime in the
    // denominator. The running sum's denominator is the lcm of all of
    // them and exceeds `u64` after a handful of such changes. Fixed
    // point has no denominator to grow.
    let mut acc = rational_to_subticks(&song.audio_sync_offset_seconds)?;
    let mut prev_beat = Beat::zero();
    // StepMania applies the earliest `#BPMS` entry from beat 0 even when
    // it is written at a later beat, so that is the tempo in force before
    // any segment boundary is reached.
    let mut cur_bpm = first_segment.bpm;

    let mut entries: Vec<(i32, i32)> = vec![(0, subticks_to_i32(acc)?)];

    // Build a sorted timeline: each segment boundary + each stop. The
    // sort is stable and segments are pushed first, so a BPM change and
    // a stop at the same beat apply in that order.
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
                if seg.start_beat > prev_beat {
                    acc = acc
                        .checked_add(beats_to_subticks(prev_beat, seg.start_beat, cur_bpm)?)
                        .ok_or_else(|| SsqError::Write("cumulative time overflow".to_string()))?;
                    prev_beat = seg.start_beat;
                }
                if seg.start_beat > Beat::zero() {
                    push_entry(&mut entries, beat_to_measure_ticks(seg.start_beat)?, acc)?;
                }
                cur_bpm = seg.bpm;
            }
            Ev::Stop(stop) => {
                if stop.at_beat > prev_beat {
                    acc = acc
                        .checked_add(beats_to_subticks(prev_beat, stop.at_beat, cur_bpm)?)
                        .ok_or_else(|| SsqError::Write("cumulative time overflow".to_string()))?;
                    prev_beat = stop.at_beat;
                }
                let tick = beat_to_measure_ticks(stop.at_beat)?;
                push_entry(&mut entries, tick, acc)?;
                acc = acc
                    .checked_add(rational_to_subticks(&stop.duration_seconds)?)
                    .ok_or_else(|| SsqError::Write("cumulative time overflow".to_string()))?;
                push_entry(&mut entries, tick, acc)?;
            }
        }
    }

    // Trailing entry, so the tempo in force at the end is derivable.
    let trailing = match end_beat {
        Some(b) if b > prev_beat => Some(b),
        // A single entry has no slope at all. `prev_beat` is still zero
        // here (anything past it would have pushed a second entry), so
        // one measure in is strictly later.
        _ if entries.len() < 2 => Some(Beat::from_rational(Rational::from_integer(4))),
        _ => None,
    };
    if let Some(end) = trailing {
        acc = acc
            .checked_add(beats_to_subticks(prev_beat, end, cur_bpm)?)
            .ok_or_else(|| SsqError::Write("cumulative time overflow".to_string()))?;
        push_entry(&mut entries, beat_to_measure_ticks(end)?, acc)?;
    }

    Ok(entries)
}

/// Append `(tick, round(acc))` unless it repeats the previous entry. A
/// BPM change and a stop at the same beat both want an anchor there;
/// emitting it twice would encode a zero-length stop.
fn push_entry(entries: &mut Vec<(i32, i32)>, tick: i32, acc: i128) -> Result<(), SsqError> {
    let entry = (tick, subticks_to_i32(acc)?);
    if entries.last() != Some(&entry) {
        entries.push(entry);
    }
    Ok(())
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

/// Resolution of the seconds-tick accumulator used by tempo synthesis:
/// one seconds-tick (1 ms at TPS=1000) is this many accumulator units.
/// Every term is rounded to the nearest unit, so the error per term is
/// at most half a nanosecond and even a `u16::MAX`-entry chunk stays
/// four orders of magnitude inside the ±0.5 ms output quantum.
const SUBTICKS_PER_TICK: i128 = 1_000_000;

/// Divide, rounding half away from zero. `den` must be positive.
fn div_round(num: i128, den: i128) -> i128 {
    let half = den / 2;
    if num >= 0 {
        (num + half) / den
    } else {
        (num - half) / den
    }
}

/// Seconds → sub-ticks: `r × TPS × SUBTICKS_PER_TICK`, rounded.
fn rational_to_subticks(r: &Rational) -> Result<i128, SsqError> {
    let scale = i128::from(MODERN_TPS) * SUBTICKS_PER_TICK;
    let num = (r.num() as i128)
        .checked_mul(scale)
        .ok_or_else(|| SsqError::Write("seconds value overflow".to_string()))?;
    Ok(div_round(num, r.den() as i128))
}

/// Sub-ticks elapsed between two beats at `bpm`:
/// `Δbeats × 60 × TPS × SUBTICKS_PER_TICK / bpm`, rounded.
///
/// Computed directly in `i128` rather than through `Rational` so that
/// the BPM's denominator never has to fit alongside everything else.
fn beats_to_subticks(
    from_beat: Beat,
    to_beat: Beat,
    bpm: crate::model::Bpm,
) -> Result<i128, SsqError> {
    let delta = to_beat
        .as_rational()
        .sub(&from_beat.as_rational())
        .map_err(|e| SsqError::Write(format!("beat delta: {e}")))?;
    let bpm = bpm.as_rational();
    if bpm.num() <= 0 {
        return Err(SsqError::Write(format!(
            "tempo must be positive, got {}/{} BPM",
            bpm.num(),
            bpm.den()
        )));
    }
    let overflow = || SsqError::Write("segment duration overflow".to_string());
    let scale = 60 * i128::from(MODERN_TPS) * SUBTICKS_PER_TICK;
    let num = (delta.num() as i128)
        .checked_mul(bpm.den() as i128)
        .and_then(|n| n.checked_mul(scale))
        .ok_or_else(overflow)?;
    let den = (delta.den() as i128)
        .checked_mul(bpm.num() as i128)
        .ok_or_else(overflow)?;
    Ok(div_round(num, den))
}

/// Sub-ticks → whole seconds-ticks as `i32`, rounded half away from zero.
fn subticks_to_i32(acc: i128) -> Result<i32, SsqError> {
    i32::try_from(div_round(acc, SUBTICKS_PER_TICK))
        .map_err(|_| SsqError::Write("seconds-tick out of i32 range".to_string()))
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
///
/// A step chunk has one entry per tick, so notes the model keeps
/// separate at the same beat — a tap beside a hold head, or two holds
/// with different tails — are OR-ed into a single step byte here; the
/// freeze block carries which of that byte's panels are held (§5.4).
/// Freeze-ends that coincide are likewise folded into one `0x00` step
/// with a multi-bit panel mask.
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
            NoteKind::Mine => {
                // Mines travel in a dedicated MINE_DATA chunk, not in the step chunk.
                // Emitting them into `rows` here would misclassify them as taps
                // at load time. A separate writer pass handles the mine chunk.
            }
        }
    }

    // One step byte per tick, one freeze entry per freeze-end tick.
    rows.sort_by_key(|(t, _)| *t);
    freezes.sort_by_key(|(t, _)| *t);
    let rows = or_same_tick(rows);
    let freezes = or_same_tick(freezes);

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

/// Fold consecutive `(tick, mask)` pairs that share a tick into one pair
/// with the masks OR-ed. Input must already be sorted by tick.
fn or_same_tick(sorted: Vec<(i32, u8)>) -> Vec<(i32, u8)> {
    let mut out: Vec<(i32, u8)> = Vec::with_capacity(sorted.len());
    for (tick, mask) in sorted {
        match out.last_mut() {
            Some((t, m)) if *t == tick => *m |= mask,
            _ => out.push((tick, mask)),
        }
    }
    out
}

pub(super) fn difficulty_code(
    style: crate::model::Style,
    difficulty: crate::model::Difficulty,
) -> u16 {
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

    // ---------- tempo synthesis ----------

    fn seg(beat: Rational, bpm: Rational) -> crate::model::TempoSegment {
        crate::model::TempoSegment {
            start_beat: Beat::from_rational(beat),
            bpm: Bpm::from_rational(bpm),
        }
    }

    fn tap_at(beat: i64) -> Note {
        Note {
            beat: Beat::from_rational(Rational::from_integer(beat)),
            kind: NoteKind::Tap,
            panels: PanelSet::from_bits(Style::Single, 0x01),
        }
    }

    /// BPM implied by two consecutive tempo pairs, as the game derives it.
    fn slope_bpm(a: (i32, i32), b: (i32, i32)) -> f64 {
        240.0 * 1000.0 * f64::from(b.0 - a.0) / (4096.0 * f64::from(b.1 - a.1))
    }

    #[test]
    fn tempo_synthesis_survives_awkward_bpm_decimals() {
        // Regression: `#BPMS` values like 249.999985 (= 49999997/200000)
        // put large primes in the running sum's denominator. With an
        // exact Rational accumulator the lcm blew past u64 after a few
        // such changes and the whole conversion failed with
        // "cumulative add: arithmetic overflow". Shape modelled on the
        // raputa / Bob-Omb charts that reported it.
        let mut song = empty_song(1000);
        let bpms = [
            "230",
            "460",
            "230",
            "115",
            "230",
            "240",
            "249.999985",
            "260",
            "270",
            "1080",
            "115",
            "230",
            "238",
            "246",
            "251.999985",
            "258",
            "266",
            "287",
            "298",
            "309",
            "320",
            "118.3",
            "115.03",
            "111.06",
            "107.7",
            "107",
            "53.5",
        ];
        for (i, bpm) in bpms.iter().enumerate() {
            let (int, frac) = bpm.split_once('.').unwrap_or((bpm, ""));
            let den = 10i64.pow(frac.len() as u32);
            let num = int.parse::<i64>().unwrap() * den + frac.parse::<i64>().unwrap_or(0);
            song.tempo_segments.push(seg(
                Rational::new(i as i64 * 7 + i as i64 % 3, 8).unwrap(),
                Rational::new(num, den).unwrap(),
            ));
            song.stops.push(crate::model::Stop {
                at_beat: Beat::from_rational(Rational::new(i as i64 * 7 + 3, 8).unwrap()),
                duration_seconds: Rational::new(65_217 + i as i64, 1_000_000).unwrap(),
            });
        }
        let entries = synthesize_tempo_entries_until(
            &song,
            Some(Beat::from_rational(Rational::from_integer(600))),
        )
        .expect("fixed-point accumulation must not overflow");
        assert_eq!(entries.last().unwrap().0, 600 * 1024);
        // Monotone in both axes.
        for w in entries.windows(2) {
            assert!(w[1].0 >= w[0].0, "ticks must not go backwards: {w:?}");
            assert!(w[1].1 >= w[0].1, "time must not go backwards: {w:?}");
        }
    }

    #[test]
    fn trailing_entry_encodes_final_bpm_change() {
        // Regression (Mukade): 160 → 1280 at beat 180 with a 2.25 s stop
        // there, back to 160 at beat 196, notes to beat 324. The final
        // `#BPMS` entry only exists in the SSQ as the slope to a later
        // pair, so a trailing pair must follow it; without one the job
        // layer extrapolated the *1280* slope to the end of the song.
        let mut song = empty_song(1000);
        song.tempo_segments = vec![
            seg(Rational::from_integer(0), Rational::from_integer(160)),
            seg(Rational::from_integer(180), Rational::from_integer(1280)),
            seg(Rational::from_integer(196), Rational::from_integer(160)),
        ];
        song.stops = vec![crate::model::Stop {
            at_beat: Beat::from_rational(Rational::from_integer(180)),
            duration_seconds: Rational::new(9, 4).unwrap(),
        }];
        song.charts.push(Chart {
            style: Style::Single,
            difficulty: Difficulty::Expert,
            notes: vec![tap_at(324)],
        });

        let entries = synthesize_tempo_entries(&song).unwrap();
        // 180 beats @160 = 67 500 ms; stop → 69 750; 16 beats @1280 =
        // 750 ms → 70 500 at beat 196; 128 beats @160 = 48 000 ms.
        assert_eq!(
            entries,
            vec![
                (0, 0),
                (184_320, 67_500),
                (184_320, 69_750),
                (200_704, 70_500),
                (331_776, 118_500),
            ]
        );
        let n = entries.len();
        assert!((slope_bpm(entries[n - 2], entries[n - 1]) - 160.0).abs() < 1e-9);
    }

    #[test]
    fn bpm_change_and_stop_at_same_beat_share_one_anchor() {
        // A segment boundary and a stop at the same beat used to emit the
        // anchor twice, encoding a zero-length stop between them.
        let mut song = empty_song(1000);
        song.tempo_segments = vec![
            seg(Rational::from_integer(0), Rational::from_integer(120)),
            seg(Rational::from_integer(4), Rational::from_integer(240)),
        ];
        song.stops = vec![crate::model::Stop {
            at_beat: Beat::from_rational(Rational::from_integer(4)),
            duration_seconds: Rational::new(1, 2).unwrap(),
        }];
        song.charts.push(Chart {
            style: Style::Single,
            difficulty: Difficulty::Basic,
            notes: vec![tap_at(8)],
        });
        let entries = synthesize_tempo_entries(&song).unwrap();
        assert_eq!(
            entries,
            vec![(0, 0), (4096, 2000), (4096, 2500), (8192, 3500)],
            "no duplicate anchor at beat 4; trailing entry at the last note"
        );
    }

    #[test]
    fn explicit_end_beat_places_trailing_entry_there() {
        let mut song = empty_song(1000);
        song.tempo_segments = vec![seg(Rational::from_integer(0), Rational::from_integer(120))];
        song.charts.push(Chart {
            style: Style::Single,
            difficulty: Difficulty::Basic,
            notes: vec![tap_at(10)],
        });
        let end = Beat::from_rational(Rational::from_integer(24));
        let entries = synthesize_tempo_entries_until(&song, Some(end)).unwrap();
        assert_eq!(entries, vec![(0, 0), (24 * 1024, 12_000)]);
    }

    #[test]
    fn no_trailing_entry_when_end_beat_precedes_last_anchor() {
        // The last note is before the last BPM change: nothing follows
        // that change, so there is nothing to encode after it.
        let mut song = empty_song(1000);
        song.tempo_segments = vec![
            seg(Rational::from_integer(0), Rational::from_integer(120)),
            seg(Rational::from_integer(100), Rational::from_integer(240)),
        ];
        let end = Beat::from_rational(Rational::from_integer(50));
        let entries = synthesize_tempo_entries_until(&song, Some(end)).unwrap();
        assert_eq!(entries, vec![(0, 0), (102_400, 50_000)]);
    }

    #[test]
    fn first_bpm_applies_from_beat_zero_when_written_later() {
        // `#BPMS:1.000=120.000;` — StepMania uses the first entry for
        // everything before it. Beat 1 must land at 500 ms, not 0.
        let mut song = empty_song(1000);
        song.tempo_segments = vec![seg(Rational::from_integer(1), Rational::from_integer(120))];
        let end = Beat::from_rational(Rational::from_integer(3));
        let entries = synthesize_tempo_entries_until(&song, Some(end)).unwrap();
        assert_eq!(entries, vec![(0, 0), (1024, 500), (3072, 1500)]);
    }

    #[test]
    fn tempo_synthesis_rounds_half_away_from_zero_like_before() {
        // 168 BPM → 2500/7 ms per beat. Beat 7 is exactly 2500 ms; beat 1
        // is 357.142… → 357. Locks the fixed-point path to the values the
        // Rational path produced.
        let mut song = empty_song(1000);
        song.tempo_segments = vec![
            seg(Rational::from_integer(0), Rational::from_integer(168)),
            seg(Rational::from_integer(1), Rational::from_integer(168)),
            seg(Rational::from_integer(7), Rational::from_integer(168)),
        ];
        let entries = synthesize_tempo_entries_until(&song, None).unwrap();
        assert_eq!(entries, vec![(0, 0), (1024, 357), (7168, 2500)]);
    }

    // ---------- same-beat notes ----------

    #[test]
    fn tap_and_hold_at_same_beat_share_one_step_byte() {
        // Model: tap Left + hold Right at beat 1. The SSQ has one step
        // byte (0x09) at that tick and a freeze-end naming only Right;
        // re-parsing must give the two notes back, not a two-panel hold.
        let mut song = empty_song(1000);
        song.tempo_segments
            .push(seg(Rational::zero(), Rational::from_integer(120)));
        song.charts.push(Chart {
            style: Style::Single,
            difficulty: Difficulty::Expert,
            notes: vec![
                Note {
                    beat: Beat::from_measure_ticks(1024).unwrap(),
                    kind: NoteKind::Tap,
                    panels: PanelSet::from_bits(Style::Single, 0x01),
                },
                Note {
                    beat: Beat::from_measure_ticks(1024).unwrap(),
                    kind: NoteKind::HoldHead {
                        length: Beat::from_measure_ticks(2048).unwrap(),
                    },
                    panels: PanelSet::from_bits(Style::Single, 0x08),
                },
            ],
        });

        let (ticks, steps, freezes) = emit_steps_and_freezes(&song.charts[0]).unwrap();
        assert_eq!(ticks, vec![1024, 3072]);
        assert_eq!(steps, vec![0x09, 0x00]);
        assert_eq!(freezes, vec![(0x08, 0x01)]);

        let mut bytes = Vec::new();
        write(&song, &[], &[], &mut bytes).unwrap();
        let parsed = parse(&bytes).unwrap();
        let notes = &parsed.song.charts[0].notes;
        assert_eq!(notes.len(), 2);
        assert_eq!(notes[0].kind, NoteKind::Tap);
        assert_eq!(notes[0].panels.bits(), 0x01);
        assert_eq!(
            notes[1].kind,
            NoteKind::HoldHead {
                length: Beat::from_measure_ticks(2048).unwrap()
            }
        );
        assert_eq!(notes[1].panels.bits(), 0x08);
    }

    #[test]
    fn holds_ending_together_share_one_freeze_end() {
        // Two single-panel holds at the same beat with the same length
        // collapse to one step byte and one freeze entry (0x03).
        let mut song = empty_song(1000);
        song.tempo_segments
            .push(seg(Rational::zero(), Rational::from_integer(120)));
        let hold = |panel: u8| Note {
            beat: Beat::from_measure_ticks(1024).unwrap(),
            kind: NoteKind::HoldHead {
                length: Beat::from_measure_ticks(1024).unwrap(),
            },
            panels: PanelSet::from_bits(Style::Single, panel),
        };
        song.charts.push(Chart {
            style: Style::Single,
            difficulty: Difficulty::Expert,
            notes: vec![hold(0x01), hold(0x02)],
        });
        let (ticks, steps, freezes) = emit_steps_and_freezes(&song.charts[0]).unwrap();
        assert_eq!(ticks, vec![1024, 2048]);
        assert_eq!(steps, vec![0x03, 0x00]);
        assert_eq!(freezes, vec![(0x03, 0x01)]);
    }
}
