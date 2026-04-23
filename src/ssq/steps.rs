//! Type-3 step-chunk parser (spec §5).
//!
//! Decodes a single chart: difficulty code, time offsets, step bytes,
//! and the freeze block. Produces a [`Chart`] whose notes are ordered
//! by ascending beat.

use crate::model::{Beat, Chart, Difficulty, Note, NoteKind, PanelSet, ShockSide, Style};
use crate::util::io::LeReader;

use super::chunk::ChunkHeader;
use super::SsqError;

/// Decode the 16-bit difficulty code in `param2` into (style, difficulty).
///
/// Low byte is the style (0x14 Single, 0x18 Double); high byte is the
/// difficulty slot (0x01 Basic, 0x02 Difficult, 0x03 Expert, 0x04
/// Beginner, 0x06 Challenge). Any other combination is rejected — the
/// DDR World dispatcher only accepts this exact set (spec §5.1).
fn decode_difficulty_code(code: u16, chunk_offset: usize) -> Result<(Style, Difficulty), SsqError> {
    let style = match code & 0x00FF {
        0x14 => Style::Single,
        0x18 => Style::Double,
        _ => {
            return Err(SsqError::InvalidDifficultyCode {
                code,
                offset: chunk_offset,
            });
        }
    };
    let difficulty = match (code & 0xFF00) >> 8 {
        0x01 => Difficulty::Basic,
        0x02 => Difficulty::Difficult,
        0x03 => Difficulty::Expert,
        0x04 => Difficulty::Beginner,
        0x06 => Difficulty::Challenge,
        _ => {
            return Err(SsqError::InvalidDifficultyCode {
                code,
                offset: chunk_offset,
            });
        }
    };
    Ok((style, difficulty))
}

/// Parse a type-3 step chunk into a [`Chart`].
pub fn parse_steps_chunk(
    header: &ChunkHeader,
    body: &[u8],
    chunk_offset: usize,
) -> Result<Chart, SsqError> {
    let (style, difficulty) = decode_difficulty_code(header.param2, chunk_offset)?;
    let entry_count = usize::from(header.param3);

    let (time_offsets, step_bytes, freeze_entries) = split_body(body, entry_count, chunk_offset)?;

    let notes = resolve_notes(
        style,
        &time_offsets,
        &step_bytes,
        &freeze_entries,
        chunk_offset,
    )?;

    Ok(Chart {
        style,
        difficulty,
        notes,
    })
}

/// One entry from the freeze block: panel mask + kind byte.
#[derive(Debug, Clone, Copy)]
struct FreezeEntry {
    panels: u8,
    kind: u8,
}

/// Split the chunk body into its three sections.
type SplitBody = (Vec<i32>, Vec<u8>, Vec<FreezeEntry>);

/// Split the chunk body into time-offsets, step-bytes, and freeze entries.
///
/// Body layout per spec §5.2:
/// - N × i32 time offsets (4N bytes)
/// - N × u8 step bytes (N bytes)
/// - 0 or 1 byte of pad (if N is odd) so freeze block is 2-byte-aligned
/// - F × 2 bytes of freeze entries (F = count of 0x00 step bytes)
/// - 0 or 2 bytes of trailing pad so total chunk length is dword-aligned
fn split_body(body: &[u8], entry_count: usize, chunk_offset: usize) -> Result<SplitBody, SsqError> {
    let time_bytes = entry_count.checked_mul(4).ok_or(SsqError::MalformedChunk {
        offset: chunk_offset,
        reason: format!("step entry count {entry_count} overflows body size"),
    })?;
    let steps_start = time_bytes;
    let steps_end = steps_start + entry_count;
    let freeze_start = steps_start + round_up_to_even(entry_count);

    if body.len() < freeze_start {
        return Err(SsqError::MalformedChunk {
            offset: chunk_offset,
            reason: format!(
                "step chunk body size {} is too small for {entry_count} time+step entries",
                body.len()
            ),
        });
    }

    let mut reader = LeReader::new(&body[..time_bytes]);
    let mut time_offsets = Vec::with_capacity(entry_count);
    for _ in 0..entry_count {
        time_offsets.push(reader.read_u32().map_err(SsqError::Io)? as i32);
    }
    let step_bytes = body[steps_start..steps_end].to_vec();

    let freeze_count = step_bytes.iter().filter(|b| **b == 0x00).count();
    let freeze_bytes_needed = freeze_count
        .checked_mul(2)
        .ok_or(SsqError::MalformedChunk {
            offset: chunk_offset,
            reason: format!("freeze count {freeze_count} overflows body size"),
        })?;
    if body.len() < freeze_start + freeze_bytes_needed {
        return Err(SsqError::MalformedChunk {
            offset: chunk_offset,
            reason: format!(
                "step chunk body size {} cannot hold {freeze_count} freeze entries after freeze block start {freeze_start}",
                body.len()
            ),
        });
    }
    let mut freeze_entries = Vec::with_capacity(freeze_count);
    for i in 0..freeze_count {
        let off = freeze_start + 2 * i;
        freeze_entries.push(FreezeEntry {
            panels: body[off],
            kind: body[off + 1],
        });
    }

    Ok((time_offsets, step_bytes, freeze_entries))
}

fn round_up_to_even(n: usize) -> usize {
    (n + 1) & !1
}

/// Classify a non-zero step byte. Returns the kind and panel-mask for
/// the note at this tick. `None` indicates a freeze-end marker (`0x00`),
/// which is handled separately.
fn classify_step_byte(byte: u8, style: Style) -> Option<(NoteKind, PanelSet)> {
    match byte {
        0x00 => None,
        0xFF => Some((
            NoteKind::Shock {
                side: ShockSide::BothSides,
            },
            PanelSet::from_bits(style, 0xFF),
        )),
        0x0F => Some((
            NoteKind::Shock {
                side: ShockSide::P1Only,
            },
            PanelSet::from_bits(style, 0x0F),
        )),
        0xF0 => Some((
            NoteKind::Shock {
                side: ShockSide::P2Only,
            },
            PanelSet::from_bits(style, 0xF0),
        )),
        bits => Some((NoteKind::Tap, PanelSet::from_bits(style, bits))),
    }
}

/// Build the note list, resolving freeze-ends per spec §5.4.
///
/// Each non-zero step byte becomes a [`Note`] at its tick. Each zero
/// step byte consumes one freeze entry from `freeze_entries` in file
/// order; for each set bit in that entry's panel mask (when
/// `kind == 0x01`), walk the built notes backward to find the most
/// recent tap that hit that panel and promote it to a [`HoldHead`] with
/// length = current_tick − head_tick. Other kind values are silently
/// ignored (spec §5.4).
fn resolve_notes(
    style: Style,
    time_offsets: &[i32],
    step_bytes: &[u8],
    freeze_entries: &[FreezeEntry],
    chunk_offset: usize,
) -> Result<Vec<Note>, SsqError> {
    let mut notes: Vec<Note> = Vec::with_capacity(step_bytes.len());
    let mut freeze_index = 0usize;

    for (i, &byte) in step_bytes.iter().enumerate() {
        let tick = time_offsets[i];
        match classify_step_byte(byte, style) {
            Some((kind, panels)) => {
                notes.push(Note {
                    beat: Beat::from_measure_ticks(i64::from(tick)).map_err(|e| {
                        SsqError::MalformedChunk {
                            offset: chunk_offset,
                            reason: format!("invalid tick {tick}: {e}"),
                        }
                    })?,
                    kind,
                    panels,
                });
            }
            None => {
                // Freeze-end marker: consume one freeze entry regardless of kind.
                let entry = freeze_entries.get(freeze_index).copied().ok_or_else(|| {
                    SsqError::MalformedChunk {
                        offset: chunk_offset,
                        reason: format!(
                            "0x00 step at entry {i} has no matching freeze entry (index {freeze_index})"
                        ),
                    }
                })?;
                freeze_index += 1;

                if entry.kind != 0x01 {
                    continue;
                }

                let end_beat = Beat::from_measure_ticks(i64::from(tick)).map_err(|e| {
                    SsqError::MalformedChunk {
                        offset: chunk_offset,
                        reason: format!("invalid freeze-end tick {tick}: {e}"),
                    }
                })?;
                resolve_freeze_end(&mut notes, entry.panels, end_beat, tick, chunk_offset)?;
            }
        }
    }

    Ok(notes)
}

/// For each set bit in `panels_mask`, walk `notes` backward to find the
/// most recent note hitting that panel. Promote that note to a
/// [`HoldHead`] with the computed length. When a note's `kind` is
/// already `HoldHead` (from an earlier bit in the same freeze-end), it
/// is left alone — each bit closes at most one head.
fn resolve_freeze_end(
    notes: &mut [Note],
    panels_mask: u8,
    end_beat: Beat,
    end_tick: i32,
    chunk_offset: usize,
) -> Result<(), SsqError> {
    let mut pending = panels_mask;
    let mut idx = notes.len();
    while pending != 0 && idx > 0 {
        idx -= 1;
        let note_bits = notes[idx].panels.bits();
        let overlap = note_bits & pending;
        if overlap == 0 {
            continue;
        }

        // This earlier note hits one or more panels we're looking for.
        // Convert it to a HoldHead if it's still a Tap; shock heads are
        // not promoted (shocks don't form freezes per §5.3).
        let note_tick_beat = notes[idx].beat;
        let length = end_beat
            .as_rational()
            .sub(&note_tick_beat.as_rational())
            .map_err(|e| SsqError::MalformedChunk {
                offset: chunk_offset,
                reason: format!("freeze length math: {e}"),
            })?;
        let length_beat = Beat::from_rational(length);

        if matches!(notes[idx].kind, NoteKind::Tap) {
            notes[idx].kind = NoteKind::HoldHead {
                length: length_beat,
            };
        }
        // Mark these panels as resolved even if the note was already a
        // HoldHead or Shock — the spec says each panel bit matches at
        // most one earlier note.
        pending &= !overlap;
    }

    if pending != 0 {
        // Some bits had no earlier head. Report the first unmatched bit.
        let panel = pending.trailing_zeros() as u8;
        return Err(SsqError::FreezeWithoutHead {
            freeze_offset: chunk_offset,
            tick: end_tick,
            panel,
        });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Rational;

    /// Build a step chunk header + body from ticks, step bytes, and freeze entries.
    /// Caller is responsible for making freeze count match the 0x00 step bytes.
    fn build_steps_chunk(
        param2: u16,
        ticks: &[i32],
        step_bytes: &[u8],
        freezes: &[(u8, u8)],
    ) -> (ChunkHeader, Vec<u8>) {
        assert_eq!(ticks.len(), step_bytes.len());
        let n = ticks.len();
        let mut body = Vec::new();
        for t in ticks {
            body.extend_from_slice(&(*t as u32).to_le_bytes());
        }
        body.extend_from_slice(step_bytes);
        // Pad step block to 2-byte boundary if N is odd.
        if !n.is_multiple_of(2) {
            body.push(0);
        }
        for (panels, kind) in freezes {
            body.push(*panels);
            body.push(*kind);
        }
        // Trailing pad to dword-align the chunk length.
        while !(12 + body.len()).is_multiple_of(4) {
            body.push(0);
        }
        let header = ChunkHeader {
            length: (12 + body.len()) as u32,
            ty: 3,
            param2,
            param3: n as u16,
            param4: 0,
        };
        (header, body)
    }

    #[test]
    fn difficulty_single_basic() {
        assert_eq!(
            decode_difficulty_code(0x0114, 0).unwrap(),
            (Style::Single, Difficulty::Basic)
        );
    }

    #[test]
    fn difficulty_double_expert() {
        assert_eq!(
            decode_difficulty_code(0x0318, 0).unwrap(),
            (Style::Double, Difficulty::Expert)
        );
    }

    #[test]
    fn difficulty_all_valid_codes() {
        let cases = [
            (0x0114, Style::Single, Difficulty::Basic),
            (0x0214, Style::Single, Difficulty::Difficult),
            (0x0314, Style::Single, Difficulty::Expert),
            (0x0414, Style::Single, Difficulty::Beginner),
            (0x0614, Style::Single, Difficulty::Challenge),
            (0x0118, Style::Double, Difficulty::Basic),
            (0x0218, Style::Double, Difficulty::Difficult),
            (0x0318, Style::Double, Difficulty::Expert),
            (0x0418, Style::Double, Difficulty::Beginner),
            (0x0618, Style::Double, Difficulty::Challenge),
        ];
        for (code, style, diff) in cases {
            assert_eq!(decode_difficulty_code(code, 0).unwrap(), (style, diff));
        }
    }

    #[test]
    fn difficulty_rejects_slot_5() {
        // Slot 0x05 (between Beginner and Challenge) is NOT accepted by DDR World.
        let err = decode_difficulty_code(0x0514, 0).unwrap_err();
        assert!(matches!(
            err,
            SsqError::InvalidDifficultyCode { code: 0x0514, .. }
        ));
    }

    #[test]
    fn difficulty_rejects_unknown_style() {
        let err = decode_difficulty_code(0x0199, 0).unwrap_err();
        assert!(matches!(err, SsqError::InvalidDifficultyCode { .. }));
    }

    #[test]
    fn single_tap_note_decodes_panel_mask() {
        // Step byte 0x05 = Left (bit 0) + Up (bit 2) at tick 1024 (1 beat).
        let (h, body) = build_steps_chunk(0x0114, &[1024], &[0x05], &[]);
        let chart = parse_steps_chunk(&h, &body, 0).unwrap();
        assert_eq!(chart.style, Style::Single);
        assert_eq!(chart.notes.len(), 1);
        assert_eq!(chart.notes[0].kind, NoteKind::Tap);
        assert_eq!(chart.notes[0].panels.bits(), 0x05);
        assert_eq!(chart.notes[0].beat, Beat::from_measure_ticks(1024).unwrap());
    }

    #[test]
    fn double_tap_uses_full_8_bit_mask() {
        // Double mode, step byte 0x88 = P1 Right (bit 3) + P2 Right (bit 7).
        let (h, body) = build_steps_chunk(0x0118, &[0], &[0x88], &[]);
        let chart = parse_steps_chunk(&h, &body, 0).unwrap();
        assert_eq!(chart.style, Style::Double);
        assert_eq!(chart.notes[0].panels.bits(), 0x88);
    }

    #[test]
    fn single_tap_masks_high_nibble() {
        // In Single mode, bits 4-7 are masked off by PanelSet::from_bits.
        let (h, body) = build_steps_chunk(0x0114, &[0], &[0x15], &[]);
        let chart = parse_steps_chunk(&h, &body, 0).unwrap();
        // 0x15 & 0x0F = 0x05
        assert_eq!(chart.notes[0].panels.bits(), 0x05);
    }

    #[test]
    fn shock_both_sides_decodes_0xff() {
        let (h, body) = build_steps_chunk(0x0618, &[100], &[0xFF], &[]);
        let chart = parse_steps_chunk(&h, &body, 0).unwrap();
        assert_eq!(
            chart.notes[0].kind,
            NoteKind::Shock {
                side: ShockSide::BothSides
            }
        );
    }

    #[test]
    fn shock_p1_only_decodes_0x0f() {
        let (h, body) = build_steps_chunk(0x0618, &[100], &[0x0F], &[]);
        let chart = parse_steps_chunk(&h, &body, 0).unwrap();
        assert_eq!(
            chart.notes[0].kind,
            NoteKind::Shock {
                side: ShockSide::P1Only
            }
        );
    }

    #[test]
    fn shock_p2_only_decodes_0xf0() {
        let (h, body) = build_steps_chunk(0x0618, &[100], &[0xF0], &[]);
        let chart = parse_steps_chunk(&h, &body, 0).unwrap();
        assert_eq!(
            chart.notes[0].kind,
            NoteKind::Shock {
                side: ShockSide::P2Only
            }
        );
    }

    #[test]
    fn simple_freeze_promotes_tap_to_holdhead() {
        // Step byte 0x04 (Up) at tick 100, then 0x00 at tick 500, freeze (0x04, 0x01).
        // Expected: one HoldHead at tick 100 with length 400.
        let (h, body) = build_steps_chunk(0x0114, &[100, 500], &[0x04, 0x00], &[(0x04, 0x01)]);
        let chart = parse_steps_chunk(&h, &body, 0).unwrap();
        assert_eq!(chart.notes.len(), 1);
        assert_eq!(chart.notes[0].beat, Beat::from_measure_ticks(100).unwrap());
        let expected_length = Beat::from_rational(Rational::new(400, 1024).unwrap());
        assert_eq!(
            chart.notes[0].kind,
            NoteKind::HoldHead {
                length: expected_length
            }
        );
    }

    #[test]
    fn freeze_with_non_1_kind_is_ignored() {
        // freeze kind=0x00 → spec says silently consume without effect.
        // The tap at tick 100 remains a plain Tap.
        let (h, body) = build_steps_chunk(0x0114, &[100, 500], &[0x04, 0x00], &[(0x04, 0x00)]);
        let chart = parse_steps_chunk(&h, &body, 0).unwrap();
        assert_eq!(chart.notes.len(), 1);
        assert_eq!(chart.notes[0].kind, NoteKind::Tap);
    }

    #[test]
    fn freeze_end_with_multiple_bits_closes_heads_at_different_ticks() {
        // Left freeze opens at tick 100, Up freeze opens at tick 200,
        // both close at tick 500 via one freeze entry (0x05, 0x01).
        let (h, body) = build_steps_chunk(
            0x0114,
            &[100, 200, 500],
            &[0x01, 0x04, 0x00],
            &[(0x05, 0x01)],
        );
        let chart = parse_steps_chunk(&h, &body, 0).unwrap();
        assert_eq!(chart.notes.len(), 2);
        // First note: Left at tick 100, length 400
        assert_eq!(chart.notes[0].panels.bits(), 0x01);
        assert_eq!(
            chart.notes[0].kind,
            NoteKind::HoldHead {
                length: Beat::from_rational(Rational::new(400, 1024).unwrap())
            }
        );
        // Second note: Up at tick 200, length 300
        assert_eq!(chart.notes[1].panels.bits(), 0x04);
        assert_eq!(
            chart.notes[1].kind,
            NoteKind::HoldHead {
                length: Beat::from_rational(Rational::new(300, 1024).unwrap())
            }
        );
    }

    #[test]
    fn freeze_end_without_matching_head_is_rejected() {
        // 0x00 step at tick 500 with freeze entry for Right (0x08), but no
        // earlier note hits Right.
        let (h, body) = build_steps_chunk(0x0114, &[100, 500], &[0x01, 0x00], &[(0x08, 0x01)]);
        let err = parse_steps_chunk(&h, &body, 0).unwrap_err();
        assert!(matches!(err, SsqError::FreezeWithoutHead { .. }));
    }

    #[test]
    fn empty_chart_has_no_notes() {
        let (h, body) = build_steps_chunk(0x0114, &[], &[], &[]);
        let chart = parse_steps_chunk(&h, &body, 0).unwrap();
        assert!(chart.notes.is_empty());
    }

    #[test]
    fn fizz_single_basic_shape() {
        // Abbreviated shape from spec §5.6: 3 notes + 1 freeze-end + 1 freeze entry.
        // Left freeze at tick 1000 closes at tick 2000 via freeze (0x01, 0x01).
        let (h, body) = build_steps_chunk(
            0x0114,
            &[500, 1000, 2000],
            &[0x02, 0x01, 0x00],
            &[(0x01, 0x01)],
        );
        let chart = parse_steps_chunk(&h, &body, 0).unwrap();
        assert_eq!(chart.style, Style::Single);
        assert_eq!(chart.difficulty, Difficulty::Basic);
        assert_eq!(chart.notes.len(), 2);
        // First: plain Down tap at 500
        assert_eq!(chart.notes[0].kind, NoteKind::Tap);
        assert_eq!(chart.notes[0].panels.bits(), 0x02);
        // Second: Left HoldHead at 1000 with length 1000 ticks
        assert!(matches!(chart.notes[1].kind, NoteKind::HoldHead { .. }));
        assert_eq!(chart.notes[1].panels.bits(), 0x01);
    }
}
