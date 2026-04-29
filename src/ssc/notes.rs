//! SSC `#NOTEDATA` block interpretation.

use std::io::Write;

use crate::model::{Beat, Difficulty, Note, NoteKind, PanelSet, Rational, ShockSide, Style};

use super::SscError;

/// Map the `#STEPSTYPE:` value to an internal `Style`. Rejects anything
/// outside dance-single / dance-double — pump/bm/ez2/etc. are out of
/// scope for this tool.
pub fn parse_stepstype(s: &str) -> Result<Style, SscError> {
    match s.trim() {
        "dance-single" => Ok(Style::Single),
        "dance-double" => Ok(Style::Double),
        other => Err(SscError::UnsupportedStepsType(other.to_string())),
    }
}

fn stepstype_name(style: Style) -> &'static str {
    match style {
        Style::Single => "dance-single",
        Style::Double => "dance-double",
    }
}

/// Map the `#DIFFICULTY:` value to an internal `Difficulty`. SM5's slot
/// names are canonical; alternate names (`Standard`, `Heavy`, `Light`,
/// …) that appear in historical simfiles are rejected.
pub fn parse_difficulty(s: &str) -> Result<Difficulty, SscError> {
    match s.trim() {
        "Beginner" => Ok(Difficulty::Beginner),
        "Easy" => Ok(Difficulty::Basic),
        "Medium" => Ok(Difficulty::Difficult),
        "Hard" => Ok(Difficulty::Expert),
        "Challenge" => Ok(Difficulty::Challenge),
        "Edit" => Err(SscError::EditChartSkipped),
        other => Err(SscError::UnknownDifficulty(other.to_string())),
    }
}

fn difficulty_name(d: Difficulty) -> &'static str {
    match d {
        Difficulty::Beginner => "Beginner",
        Difficulty::Basic => "Easy",
        Difficulty::Difficult => "Medium",
        Difficulty::Expert => "Hard",
        Difficulty::Challenge => "Challenge",
    }
}

/// Parse the `#NOTES:` value body — a sequence of measures separated
/// by `,`, each measure a sequence of rows (one line each), each row
/// one character per panel.
///
/// Character semantics (subset we care about):
/// - `0` empty
/// - `1` tap
/// - `2` hold head (paired with a later `3` at the same panel)
/// - `3` hold/roll tail
/// - `4` roll head — **rejected**, not supported
/// - `M` mine — accepted only as a **full-row shock** pattern (every
///   panel on the style, or every P1 panel / every P2 panel on Double).
///   Partial mine patterns are rejected because DDR's shock arrow has
///   no per-panel mine equivalent, and silently dropping them would
///   lie to the user about what their chart does.
/// - `F` fake — dropped silently
/// - `L` lift — dropped silently
pub fn parse_notes_body(body: &str, style: Style) -> Result<Vec<Note>, SscError> {
    let panel_count = style.panel_count() as usize;
    let mut notes: Vec<Note> = Vec::new();
    // Open hold heads by panel index, to match with their `3` tail.
    let mut open_holds: Vec<Option<usize>> = vec![None; panel_count];

    let measures: Vec<&str> = body.split(',').collect();
    for (measure_idx, measure) in measures.iter().enumerate() {
        let rows: Vec<&str> = measure
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .collect();
        if rows.is_empty() {
            continue;
        }
        let rows_in_measure = rows.len();
        for (row_idx, row) in rows.iter().enumerate() {
            if row.len() != panel_count {
                return Err(SscError::BadNoteRow {
                    measure: measure_idx,
                    row: row_idx,
                    reason: format!(
                        "expected {panel_count} chars for {style:?}, got {}: {row:?}",
                        row.len()
                    ),
                });
            }
            decode_row(
                row,
                measure_idx,
                row_idx,
                rows_in_measure,
                style,
                &mut notes,
                &mut open_holds,
            )?;
        }
    }

    // Any open hold heads without a tail are malformed.
    for (panel, open) in open_holds.iter().enumerate() {
        if let Some(note_idx) = open {
            return Err(SscError::UnclosedHold {
                panel: panel as u8,
                note_index: *note_idx,
            });
        }
    }

    Ok(notes)
}

#[allow(clippy::too_many_arguments)]
fn decode_row(
    row: &str,
    measure_idx: usize,
    row_idx: usize,
    rows_in_measure: usize,
    style: Style,
    notes: &mut Vec<Note>,
    open_holds: &mut [Option<usize>],
) -> Result<(), SscError> {
    // Beat of this row: measure_idx * 4 + row_idx * 4 / rows_in_measure,
    // as an exact rational.
    let beat_rational = Rational::new(
        (measure_idx as i64 * 4 * rows_in_measure as i64) + (row_idx as i64 * 4),
        rows_in_measure as i64,
    )
    .map_err(|e| SscError::BadNoteRow {
        measure: measure_idx,
        row: row_idx,
        reason: format!("beat math: {e}"),
    })?;
    let beat = Beat::from_rational(beat_rational);

    // Mine detection: classify the row per `classify_mine_row`.
    // Full-row shock patterns (`MMMM` on Single, `MMMMMMMM` /
    // `MMMM0000` / `0000MMMM` on Double) become a single `Shock`
    // note — matches DDR's shock-arrow encoding. Partial mine
    // patterns become a `Mine` note whose panels carry the mine
    // bitmask; scanning of the row continues so that mixed
    // `M`+`1`/`2`/`3`/`4` rows produce multiple notes at the same
    // beat on different panels.
    let mine_classification = classify_mine_row(row, style);
    if let MineRowKind::FullRowShock(side) = mine_classification {
        notes.push(Note {
            beat,
            kind: NoteKind::Shock { side },
            panels: PanelSet::empty(),
        });
        return Ok(());
    }
    let mine_panel_mask = match mine_classification {
        MineRowKind::PerPanelMines(mask) => mask,
        _ => 0,
    };
    if mine_panel_mask != 0 {
        notes.push(Note {
            beat,
            kind: NoteKind::Mine,
            panels: PanelSet::from_bits(style, mine_panel_mask),
        });
    }

    // Collect all tap/hold-head panels on this row into a single Note
    // (matches SSQ's "one row hits multiple panels → one Note with a
    // multi-bit panel mask"). `3` tails and `F`/`L` don't emit notes.
    // `M` characters are skipped — already consumed above into a Mine
    // or Shock note.
    let mut tap_bits: u8 = 0;
    let mut hold_heads_this_row: Vec<usize> = Vec::new();

    for (panel, ch) in row.chars().enumerate() {
        match ch {
            '0' => {}
            '1' => {
                tap_bits |= 1u8 << panel;
            }
            '2' | '4' => {
                // `2` = hold head, `4` = roll head. DDR has no rolls;
                // treat both as holds. Tail (`3`) closes either.
                tap_bits |= 1u8 << panel;
                hold_heads_this_row.push(panel);
            }
            '3' => {
                let Some(head_idx) = open_holds[panel].take() else {
                    return Err(SscError::BadNoteRow {
                        measure: measure_idx,
                        row: row_idx,
                        reason: format!("hold tail at panel {panel} without matching head"),
                    });
                };
                let length = beat
                    .as_rational()
                    .sub(&notes[head_idx].beat.as_rational())
                    .map_err(|e| SscError::BadNoteRow {
                        measure: measure_idx,
                        row: row_idx,
                        reason: format!("hold length: {e}"),
                    })?;
                notes[head_idx].kind = NoteKind::HoldHead {
                    length: Beat::from_rational(length),
                };
            }
            'M' => {
                // Already consumed into the Mine note (or a
                // FullRowShock above). Nothing to do here.
            }
            'F' | 'L' => {
                // Drop silently — not represented in our model.
            }
            other => {
                return Err(SscError::BadNoteRow {
                    measure: measure_idx,
                    row: row_idx,
                    reason: format!("unknown note character {other:?} at panel {panel}"),
                });
            }
        }
    }

    if tap_bits != 0 {
        let note_idx = notes.len();
        notes.push(Note {
            beat,
            kind: NoteKind::Tap,
            panels: PanelSet::from_bits(style, tap_bits),
        });
        for p in &hold_heads_this_row {
            open_holds[*p] = Some(note_idx);
        }
    }

    Ok(())
}

/// Classification of a single `#NOTES` row's `M` characters.
///
/// The three cases are mutually exclusive:
/// - `FullRowShock(side)`: every panel on one side (or both sides) is
///   a mine and no non-mine characters interfere. Rendered as a DDR
///   shock arrow (step-chunk byte `0xFF` / `0x0F` / `0xF0`).
/// - `PerPanelMines(mask)`: at least one `M` is present, but the
///   pattern is not a full-row shock. The `mask` has one bit per
///   panel set to 1 where the mine sits. The row may ALSO contain
///   `1`/`2`/`3`/`4` characters on other panels — those produce
///   their own notes at the same beat.
/// - `NoMines`: no `M` characters in the row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MineRowKind {
    FullRowShock(ShockSide),
    PerPanelMines(u8),
    NoMines,
}

/// Scan `row` for mines and classify the pattern.
///
/// Full-row shock detection is preserved verbatim from the original
/// `classify_mine_row(bits, style)` helper (Single mask `0x0F`, Double
/// masks `0xFF` / `0x0F` / `0xF0`). Any other set of mine bits is
/// classified as `PerPanelMines` — a per-panel ITG-style mine that
/// maps to a `NoteKind::Mine` downstream (see
/// `docs/ssq_mine_chunk_format.md`).
///
/// A row with no `M` characters returns `NoMines`; the caller then
/// takes the existing tap/hold path.
fn classify_mine_row(row: &str, style: Style) -> MineRowKind {
    let mut mine_bits: u8 = 0;
    for (panel, ch) in row.chars().enumerate() {
        if ch == 'M' {
            mine_bits |= 1u8 << panel;
        }
    }
    if mine_bits == 0 {
        return MineRowKind::NoMines;
    }
    match full_row_shock_side(mine_bits, style) {
        Some(side) => MineRowKind::FullRowShock(side),
        None => MineRowKind::PerPanelMines(mine_bits),
    }
}

/// Decide which `ShockSide` (if any) a mine bitmask represents under a
/// given style. Returns `None` for patterns that don't form a legal
/// full-row shock — those are per-panel mines instead.
fn full_row_shock_side(mine_bits: u8, style: Style) -> Option<ShockSide> {
    match style {
        Style::Single => {
            if mine_bits == 0x0F {
                Some(ShockSide::BothSides)
            } else {
                None
            }
        }
        Style::Double => match mine_bits {
            0xFF => Some(ShockSide::BothSides),
            0x0F => Some(ShockSide::P1Only),
            0xF0 => Some(ShockSide::P2Only),
            _ => None,
        },
    }
}

/// Standard SSC row-quantization values. The writer picks the smallest
/// that divides every row offset in a measure exactly.
const STANDARD_QUANTIZES: [u32; 10] = [4, 8, 12, 16, 24, 32, 48, 64, 96, 192];

/// Serialize one `Chart` as a complete `#NOTEDATA` section (opening
/// `#NOTEDATA:;` marker, per-chart tags, `#NOTES:…;` body).
pub fn write_notedata(chart: &crate::model::Chart, out: &mut impl Write) -> Result<(), SscError> {
    let io = |e: std::io::Error| SscError::Write(e.to_string());
    writeln!(out, "#NOTEDATA:;").map_err(io)?;
    writeln!(out, "#STEPSTYPE:{};", stepstype_name(chart.style)).map_err(io)?;
    writeln!(out, "#DIFFICULTY:{};", difficulty_name(chart.difficulty)).map_err(io)?;
    writeln!(out, "#NOTES:").map_err(io)?;
    write_notes_body(chart, out)?;
    writeln!(out, ";").map_err(io)
}

/// Write just the grid body — used by `write_notedata` above and
/// reusable by tests that want to check the grid in isolation.
pub fn write_notes_body(chart: &crate::model::Chart, out: &mut impl Write) -> Result<(), SscError> {
    let io = |e: std::io::Error| SscError::Write(e.to_string());
    let panel_count = chart.style.panel_count() as usize;

    // Collect every character event (panel, char) by measure index +
    // position-within-measure (as an exact rational in [0, 4)).
    // `(measure, offset_rational, panel, char)`.
    let mut events: Vec<(usize, Rational, usize, char)> = Vec::new();
    let mut max_measure: usize = 0;

    for note in &chart.notes {
        let beat = note.beat.as_rational();
        place_note_events(beat, note, chart.style, &mut events, &mut max_measure)?;
    }

    // Group by measure.
    let mut per_measure: Vec<Vec<(Rational, usize, char)>> =
        (0..=max_measure).map(|_| Vec::new()).collect();
    for (m, off, panel, ch) in events {
        per_measure[m].push((off, panel, ch));
    }

    for (i, measure_events) in per_measure.iter().enumerate() {
        if i > 0 {
            writeln!(out, ",").map_err(io)?;
        }
        let rows = pick_quantize(measure_events, i)?;
        let mut grid: Vec<Vec<char>> = (0..rows).map(|_| vec!['0'; panel_count]).collect();
        for (off, panel, ch) in measure_events {
            // row = off / 4 * rows == off * rows / 4
            let row_num = (off.num() as i128) * (rows as i128);
            let row_den = (off.den() as i128) * 4;
            // Guaranteed exact by pick_quantize; assert defensively.
            debug_assert!(row_num % row_den == 0);
            let row_idx = (row_num / row_den) as usize;
            grid[row_idx][*panel] = *ch;
        }
        for row in &grid {
            let line: String = row.iter().collect();
            writeln!(out, "{line}").map_err(io)?;
        }
    }
    Ok(())
}

/// Expand one `Note` into 1–N `(measure, offset_within_measure, panel, char)` events.
fn place_note_events(
    beat: Rational,
    note: &Note,
    style: Style,
    events: &mut Vec<(usize, Rational, usize, char)>,
    max_measure: &mut usize,
) -> Result<(), SscError> {
    match &note.kind {
        NoteKind::Tap => {
            let (m, off) = split_beat_into_measure(beat)?;
            *max_measure = (*max_measure).max(m);
            for p in active_panels(note.panels, style) {
                events.push((m, off, p, '1'));
            }
        }
        NoteKind::HoldHead { length } => {
            let (mh, offh) = split_beat_into_measure(beat)?;
            *max_measure = (*max_measure).max(mh);
            let tail_beat = beat
                .add(&length.as_rational())
                .map_err(|_| SscError::Write("hold tail beat overflow".to_string()))?;
            let (mt, offt) = split_beat_into_measure(tail_beat)?;
            *max_measure = (*max_measure).max(mt);
            for p in active_panels(note.panels, style) {
                events.push((mh, offh, p, '2'));
                events.push((mt, offt, p, '3'));
            }
        }
        NoteKind::Shock { side } => {
            let (m, off) = split_beat_into_measure(beat)?;
            *max_measure = (*max_measure).max(m);
            let mine_bits = shock_side_to_bits(*side, style)?;
            for p in 0..style.panel_count() as usize {
                if (mine_bits >> p) & 1 != 0 {
                    events.push((m, off, p, 'M'));
                }
            }
        }
        NoteKind::Mine => {
            // Per-panel mine: emit 'M' for each active panel bit.
            // Mirrors the `Tap` arm — mines are "instantaneous hit/miss
            // on these panels at this beat" (see model's NoteKind
            // doc). Coexists with Tap/HoldHead emission on the same
            // row at different panels via the grid-assembly step in
            // `write_notes_body`.
            let (m, off) = split_beat_into_measure(beat)?;
            *max_measure = (*max_measure).max(m);
            for p in active_panels(note.panels, style) {
                events.push((m, off, p, 'M'));
            }
        }
    }
    Ok(())
}

/// Split an absolute beat into `(measure_index, offset_within_measure)`.
/// `offset_within_measure` is in `[0, 4)` as an exact rational.
fn split_beat_into_measure(beat: Rational) -> Result<(usize, Rational), SscError> {
    // measure = floor(beat / 4); offset = beat - 4*measure.
    let num = beat.num() as i128;
    let den = beat.den() as i128;
    if num < 0 {
        return Err(SscError::Write(format!(
            "negative beat {num}/{den} has no measure representation"
        )));
    }
    let measure = (num / den / 4) as usize;
    let four_m = Rational::from_integer(4 * measure as i64);
    let offset = beat
        .sub(&four_m)
        .map_err(|e| SscError::Write(format!("measure offset: {e}")))?;
    Ok((measure, offset))
}

/// Expand a `PanelSet` into the list of active panel indices (0..panel_count).
fn active_panels(panels: PanelSet, style: Style) -> Vec<usize> {
    let n = style.panel_count() as usize;
    (0..n).filter(|p| panels.contains(*p as u8)).collect()
}

/// Convert a `ShockSide` + style into the mine bitmask the shock paints
/// on a single row. Errors if the side mentions a half that doesn't
/// exist in the target style (P1Only/P2Only on Single have no
/// natural mapping, so the caller is told to check input data).
fn shock_side_to_bits(side: ShockSide, style: Style) -> Result<u8, SscError> {
    match (style, side) {
        (Style::Single, ShockSide::BothSides) => Ok(0x0F),
        (Style::Double, ShockSide::BothSides) => Ok(0xFF),
        (Style::Double, ShockSide::P1Only) => Ok(0x0F),
        (Style::Double, ShockSide::P2Only) => Ok(0xF0),
        (Style::Single, s) => Err(SscError::ShockSideIncompatibleWithStyle {
            side: format!("{s:?}"),
        }),
    }
}

/// Pick the smallest standard quantize that represents every event in
/// `measure_events` exactly (i.e. `offset * rows / 4` is an integer
/// for every event).
fn pick_quantize(
    measure_events: &[(Rational, usize, char)],
    measure_idx: usize,
) -> Result<u32, SscError> {
    if measure_events.is_empty() {
        return Ok(4);
    }
    'outer: for &rows in &STANDARD_QUANTIZES {
        for (off, _panel, _ch) in measure_events {
            // off / 4 * rows => off.num * rows / (off.den * 4)
            let num = (off.num() as i128) * (rows as i128);
            let den = (off.den() as i128) * 4;
            if num % den != 0 {
                continue 'outer;
            }
        }
        return Ok(rows);
    }
    // No standard quantize fits — caller has an unusual note in this
    // measure. Surface the first offender.
    let (off, _panel, _ch) = &measure_events[0];
    Err(SscError::UnrepresentableBeat {
        measure: measure_idx,
        num: off.num(),
        den: off.den(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---------- stepstype / difficulty ----------

    #[test]
    fn stepstype_single() {
        assert_eq!(parse_stepstype("dance-single").unwrap(), Style::Single);
    }

    #[test]
    fn stepstype_double() {
        assert_eq!(parse_stepstype("dance-double").unwrap(), Style::Double);
    }

    #[test]
    fn stepstype_unsupported_is_rejected() {
        let err = parse_stepstype("pump-single").unwrap_err();
        assert!(matches!(err, SscError::UnsupportedStepsType(_)));
    }

    #[test]
    fn all_five_difficulties() {
        assert_eq!(parse_difficulty("Beginner").unwrap(), Difficulty::Beginner);
        assert_eq!(parse_difficulty("Easy").unwrap(), Difficulty::Basic);
        assert_eq!(parse_difficulty("Medium").unwrap(), Difficulty::Difficult);
        assert_eq!(parse_difficulty("Hard").unwrap(), Difficulty::Expert);
        assert_eq!(
            parse_difficulty("Challenge").unwrap(),
            Difficulty::Challenge
        );
    }

    #[test]
    fn edit_difficulty_rejected_distinctly() {
        let err = parse_difficulty("Edit").unwrap_err();
        assert!(matches!(err, SscError::EditChartSkipped));
    }

    #[test]
    fn unknown_difficulty_rejected() {
        let err = parse_difficulty("Standard").unwrap_err();
        assert!(matches!(err, SscError::UnknownDifficulty(_)));
    }

    // ---------- parse_notes_body ----------

    #[test]
    fn empty_notes_body() {
        let notes = parse_notes_body("", Style::Single).unwrap();
        assert!(notes.is_empty());
    }

    #[test]
    fn one_measure_four_taps_on_left() {
        let body = "\
1000
1000
1000
1000
";
        let notes = parse_notes_body(body, Style::Single).unwrap();
        assert_eq!(notes.len(), 4);
        for (i, n) in notes.iter().enumerate() {
            assert_eq!(n.panels.bits(), 0x01);
            assert_eq!(n.kind, NoteKind::Tap);
            assert_eq!(n.beat.as_rational(), Rational::from_integer(i as i64));
        }
    }

    #[test]
    fn hold_head_and_tail_form_holdhead_note() {
        let body = "\
2000
0000
0000
3000
";
        let notes = parse_notes_body(body, Style::Single).unwrap();
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].beat, Beat::from_rational(Rational::zero()));
        assert_eq!(notes[0].panels.bits(), 0x01);
        match notes[0].kind {
            NoteKind::HoldHead { length } => {
                assert_eq!(length.as_rational(), Rational::from_integer(3));
            }
            _ => panic!("expected HoldHead, got {:?}", notes[0].kind),
        }
    }

    #[test]
    fn multi_panel_row_produces_one_multi_bit_note() {
        let body = "\
1010
0000
0000
0000
";
        let notes = parse_notes_body(body, Style::Single).unwrap();
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].panels.bits(), 0x05);
    }

    #[test]
    fn double_mode_uses_8_chars_per_row() {
        let body = "\
00000001
";
        let notes = parse_notes_body(body, Style::Double).unwrap();
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].panels.bits(), 0x80);
    }

    #[test]
    fn roll_treated_as_hold() {
        let body = "4000\n0000\n0000\n3000\n";
        let notes = parse_notes_body(body, Style::Single).unwrap();
        assert_eq!(notes.len(), 1);
        assert!(matches!(notes[0].kind, NoteKind::HoldHead { .. }));
    }

    #[test]
    fn single_mine_on_single_produces_mine_note() {
        // `M000` on Single: a single mine on Left (bit 0).
        // Was rejected pre-feature; now a valid `NoteKind::Mine`.
        let body = "M000\n0000\n0000\n0000\n";
        let notes = parse_notes_body(body, Style::Single).unwrap();
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].kind, NoteKind::Mine);
        assert_eq!(notes[0].panels.bits(), 0x01);
    }

    #[test]
    fn wrong_row_width_is_rejected() {
        let body = "100\n100\n100\n100\n";
        let err = parse_notes_body(body, Style::Single).unwrap_err();
        assert!(matches!(err, SscError::BadNoteRow { .. }));
    }

    #[test]
    fn unclosed_hold_is_rejected() {
        let body = "2000\n0000\n0000\n0000\n";
        let err = parse_notes_body(body, Style::Single).unwrap_err();
        assert!(matches!(err, SscError::UnclosedHold { .. }));
    }

    #[test]
    fn tail_without_head_is_rejected() {
        let body = "0000\n3000\n0000\n0000\n";
        let err = parse_notes_body(body, Style::Single).unwrap_err();
        assert!(matches!(err, SscError::BadNoteRow { .. }));
    }

    #[test]
    fn fake_and_lift_are_dropped_silently() {
        let body = "F000\n0L00\n0000\n0000\n";
        let notes = parse_notes_body(body, Style::Single).unwrap();
        assert!(notes.is_empty());
    }

    #[test]
    fn two_measure_body_with_comma_separator() {
        let body = "\
1000
0000
0000
0000
,
0000
0000
0000
0001
";
        let notes = parse_notes_body(body, Style::Single).unwrap();
        assert_eq!(notes.len(), 2);
        assert_eq!(notes[0].beat.as_rational(), Rational::zero());
        assert_eq!(notes[1].beat.as_rational(), Rational::from_integer(7));
        assert_eq!(notes[1].panels.bits(), 0x08);
    }

    // ---------- shock (full-row mine) parsing ----------

    #[test]
    fn full_row_mines_on_single_become_bothsides_shock() {
        let body = "MMMM\n0000\n0000\n0000\n";
        let notes = parse_notes_body(body, Style::Single).unwrap();
        assert_eq!(notes.len(), 1);
        assert_eq!(
            notes[0].kind,
            NoteKind::Shock {
                side: ShockSide::BothSides
            }
        );
    }

    #[test]
    fn all_eight_mines_on_double_is_bothsides_shock() {
        let body = "MMMMMMMM\n00000000\n00000000\n00000000\n";
        let notes = parse_notes_body(body, Style::Double).unwrap();
        assert_eq!(notes.len(), 1);
        assert_eq!(
            notes[0].kind,
            NoteKind::Shock {
                side: ShockSide::BothSides
            }
        );
    }

    #[test]
    fn p1_only_mines_on_double_is_p1_shock() {
        let body = "MMMM0000\n00000000\n00000000\n00000000\n";
        let notes = parse_notes_body(body, Style::Double).unwrap();
        assert_eq!(
            notes[0].kind,
            NoteKind::Shock {
                side: ShockSide::P1Only
            }
        );
    }

    #[test]
    fn p2_only_mines_on_double_is_p2_shock() {
        let body = "0000MMMM\n00000000\n00000000\n00000000\n";
        let notes = parse_notes_body(body, Style::Double).unwrap();
        assert_eq!(
            notes[0].kind,
            NoteKind::Shock {
                side: ShockSide::P2Only
            }
        );
    }

    #[test]
    fn mixed_mine_and_tap_produces_two_notes_at_same_beat() {
        // `M100` on Single: mine on Left (bit 0), tap on Down (bit 1).
        // Both notes at the same beat, different panels.
        let body = "M100\n0000\n0000\n0000\n";
        let notes = parse_notes_body(body, Style::Single).unwrap();
        assert_eq!(notes.len(), 2);
        // Parser emits the mine first, then the tap (decode_row order).
        assert_eq!(notes[0].kind, NoteKind::Mine);
        assert_eq!(notes[0].panels.bits(), 0x01);
        assert_eq!(notes[1].kind, NoteKind::Tap);
        assert_eq!(notes[1].panels.bits(), 0x02);
        // Both at beat 0.
        assert_eq!(notes[0].beat, notes[1].beat);
    }

    #[test]
    fn partial_mines_on_double_produces_mine_note() {
        // 3 mines on P1 isn't any recognized shock side; becomes a
        // per-panel mine with mask 0x07.
        let body = "MMM00000\n00000000\n00000000\n00000000\n";
        let notes = parse_notes_body(body, Style::Double).unwrap();
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].kind, NoteKind::Mine);
        assert_eq!(notes[0].panels.bits(), 0x07);
    }

    #[test]
    fn single_down_mine_on_single_produces_0x02_mask() {
        // `0M00` on Single: mine on Down (bit 1).
        let body = "0M00\n0000\n0000\n0000\n";
        let notes = parse_notes_body(body, Style::Single).unwrap();
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].kind, NoteKind::Mine);
        assert_eq!(notes[0].panels.bits(), 0x02);
    }

    #[test]
    fn mine_mixed_with_taps_on_both_sides_produces_two_notes() {
        // `M10M` on Single: Left mine (bit 0), Down tap (bit 1),
        // Up stays 0, Right mine (bit 3).
        // Expect: Mine with 0x09 (bits 0+3) + Tap with 0x02.
        let body = "M10M\n0000\n0000\n0000\n";
        let notes = parse_notes_body(body, Style::Single).unwrap();
        assert_eq!(notes.len(), 2);
        assert_eq!(notes[0].kind, NoteKind::Mine);
        assert_eq!(notes[0].panels.bits(), 0x09);
        assert_eq!(notes[1].kind, NoteKind::Tap);
        assert_eq!(notes[1].panels.bits(), 0x02);
    }

    #[test]
    fn partial_mines_on_single_below_full_row_produces_mine_note() {
        // `MM00` on Single: two mines on Left + Down.
        // Mask 0x03; not a full-row shock; becomes a Mine note.
        let body = "MM00\n0000\n0000\n0000\n";
        let notes = parse_notes_body(body, Style::Single).unwrap();
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].kind, NoteKind::Mine);
        assert_eq!(notes[0].panels.bits(), 0x03);
    }

    #[test]
    fn single_p1_left_mine_on_double_produces_0x01_mask() {
        // `M0000000` on Double: mine on P1 Left only.
        let body = "M0000000\n00000000\n00000000\n00000000\n";
        let notes = parse_notes_body(body, Style::Double).unwrap();
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].kind, NoteKind::Mine);
        assert_eq!(notes[0].panels.bits(), 0x01);
    }

    // ---------- writer (write_notes_body / write_notedata) ----------

    fn write_body_string(chart: &crate::model::Chart) -> String {
        let mut buf: Vec<u8> = Vec::new();
        write_notes_body(chart, &mut buf).unwrap();
        String::from_utf8(buf).unwrap()
    }

    #[test]
    fn writes_empty_chart_as_single_empty_measure() {
        let chart = crate::model::Chart {
            style: Style::Single,
            difficulty: Difficulty::Basic,
            notes: Vec::new(),
        };
        let body = write_body_string(&chart);
        assert_eq!(body, "0000\n0000\n0000\n0000\n");
    }

    #[test]
    fn writes_single_tap_at_beat_zero() {
        let chart = crate::model::Chart {
            style: Style::Single,
            difficulty: Difficulty::Basic,
            notes: vec![Note {
                beat: Beat::from_rational(Rational::zero()),
                kind: NoteKind::Tap,
                panels: PanelSet::from_bits(Style::Single, 0x01),
            }],
        };
        let body = write_body_string(&chart);
        assert_eq!(body, "1000\n0000\n0000\n0000\n");
    }

    #[test]
    fn writes_multi_panel_tap_row() {
        let chart = crate::model::Chart {
            style: Style::Single,
            difficulty: Difficulty::Basic,
            notes: vec![Note {
                beat: Beat::from_rational(Rational::zero()),
                kind: NoteKind::Tap,
                panels: PanelSet::from_bits(Style::Single, 0x05),
            }],
        };
        let body = write_body_string(&chart);
        assert_eq!(body, "1010\n0000\n0000\n0000\n");
    }

    #[test]
    fn writes_hold_head_with_tail_in_same_measure() {
        let chart = crate::model::Chart {
            style: Style::Single,
            difficulty: Difficulty::Basic,
            notes: vec![Note {
                beat: Beat::from_rational(Rational::zero()),
                kind: NoteKind::HoldHead {
                    length: Beat::from_rational(Rational::from_integer(3)),
                },
                panels: PanelSet::from_bits(Style::Single, 0x01),
            }],
        };
        let body = write_body_string(&chart);
        assert_eq!(body, "2000\n0000\n0000\n3000\n");
    }

    #[test]
    fn writes_hold_spanning_two_measures() {
        // Head at beat 2, length 4 → tail at beat 6 (row 2 of measure 1).
        let chart = crate::model::Chart {
            style: Style::Single,
            difficulty: Difficulty::Basic,
            notes: vec![Note {
                beat: Beat::from_rational(Rational::from_integer(2)),
                kind: NoteKind::HoldHead {
                    length: Beat::from_rational(Rational::from_integer(4)),
                },
                panels: PanelSet::from_bits(Style::Single, 0x01),
            }],
        };
        let body = write_body_string(&chart);
        assert_eq!(body, "0000\n0000\n2000\n0000\n,\n0000\n0000\n3000\n0000\n");
    }

    #[test]
    fn writes_shock_bothsides_as_full_mine_row_single() {
        let chart = crate::model::Chart {
            style: Style::Single,
            difficulty: Difficulty::Basic,
            notes: vec![Note {
                beat: Beat::from_rational(Rational::zero()),
                kind: NoteKind::Shock {
                    side: ShockSide::BothSides,
                },
                panels: PanelSet::empty(),
            }],
        };
        let body = write_body_string(&chart);
        assert_eq!(body, "MMMM\n0000\n0000\n0000\n");
    }

    #[test]
    fn writes_shock_p1_only_as_half_mine_row_double() {
        let chart = crate::model::Chart {
            style: Style::Double,
            difficulty: Difficulty::Expert,
            notes: vec![Note {
                beat: Beat::from_rational(Rational::zero()),
                kind: NoteKind::Shock {
                    side: ShockSide::P1Only,
                },
                panels: PanelSet::empty(),
            }],
        };
        let body = write_body_string(&chart);
        assert_eq!(body, "MMMM0000\n00000000\n00000000\n00000000\n");
    }

    // ---------- Mine writing (Task 5) ----------

    #[test]
    fn writes_single_mine_at_beat_zero() {
        let chart = crate::model::Chart {
            style: Style::Single,
            difficulty: Difficulty::Basic,
            notes: vec![Note {
                beat: Beat::from_rational(Rational::zero()),
                kind: NoteKind::Mine,
                panels: PanelSet::from_bits(Style::Single, 0x01),
            }],
        };
        let body = write_body_string(&chart);
        assert_eq!(body, "M000\n0000\n0000\n0000\n");
    }

    #[test]
    fn writes_multi_panel_mine_on_same_row() {
        // A single Mine note with panels 0x0A (Down + Right) must
        // produce `0M0M` — one 'M' per active panel bit.
        let chart = crate::model::Chart {
            style: Style::Single,
            difficulty: Difficulty::Basic,
            notes: vec![Note {
                beat: Beat::from_rational(Rational::zero()),
                kind: NoteKind::Mine,
                panels: PanelSet::from_bits(Style::Single, 0x0A),
            }],
        };
        let body = write_body_string(&chart);
        assert_eq!(body, "0M0M\n0000\n0000\n0000\n");
    }

    #[test]
    fn writes_mine_and_tap_on_same_row_different_panels() {
        // Mine on Left + Up (0x05) + Tap on Down + Right (0x0A),
        // both at beat 0. Expect row `M1M1`.
        let chart = crate::model::Chart {
            style: Style::Single,
            difficulty: Difficulty::Basic,
            notes: vec![
                Note {
                    beat: Beat::from_rational(Rational::zero()),
                    kind: NoteKind::Mine,
                    panels: PanelSet::from_bits(Style::Single, 0x05),
                },
                Note {
                    beat: Beat::from_rational(Rational::zero()),
                    kind: NoteKind::Tap,
                    panels: PanelSet::from_bits(Style::Single, 0x0A),
                },
            ],
        };
        let body = write_body_string(&chart);
        assert_eq!(body, "M1M1\n0000\n0000\n0000\n");
    }

    #[test]
    fn quantize_unchanged_by_mine_row() {
        // Adding a Mine note to a row that already has a Tap must
        // not change the picked quantize. Both charts have a note
        // at beat 1.0 → expected quantize is 4 (rows=4 per measure).
        let chart_tap_only = crate::model::Chart {
            style: Style::Single,
            difficulty: Difficulty::Basic,
            notes: vec![Note {
                beat: Beat::from_rational(Rational::from_integer(1)),
                kind: NoteKind::Tap,
                panels: PanelSet::from_bits(Style::Single, 0x01),
            }],
        };
        let chart_tap_and_mine = crate::model::Chart {
            style: Style::Single,
            difficulty: Difficulty::Basic,
            notes: vec![
                Note {
                    beat: Beat::from_rational(Rational::from_integer(1)),
                    kind: NoteKind::Tap,
                    panels: PanelSet::from_bits(Style::Single, 0x01),
                },
                Note {
                    beat: Beat::from_rational(Rational::from_integer(1)),
                    kind: NoteKind::Mine,
                    panels: PanelSet::from_bits(Style::Single, 0x02),
                },
            ],
        };
        let body_tap = write_body_string(&chart_tap_only);
        let body_tap_mine = write_body_string(&chart_tap_and_mine);

        // Row count must match — both produce a 4-row measure.
        let rows_tap = body_tap.lines().count();
        let rows_tap_mine = body_tap_mine.lines().count();
        assert_eq!(
            rows_tap, rows_tap_mine,
            "mine note must not change the picked quantize"
        );
        assert_eq!(rows_tap, 4, "sanity: a beat-1.0 note fits a 4-row measure");
        // The tap+mine body must have the mine on row 1 (beat 1.0)
        // at panel 1 (Down) alongside the tap at panel 0 (Left):
        // row = `1M00`.
        let second_row = body_tap_mine.lines().nth(1).unwrap();
        assert_eq!(second_row, "1M00");
    }

    #[test]
    fn picks_8_row_quantize_for_half_beat_note() {
        // Note at beat 0.5 needs rows divisible by 8.
        let chart = crate::model::Chart {
            style: Style::Single,
            difficulty: Difficulty::Basic,
            notes: vec![Note {
                beat: Beat::from_rational(Rational::new(1, 2).unwrap()),
                kind: NoteKind::Tap,
                panels: PanelSet::from_bits(Style::Single, 0x01),
            }],
        };
        let body = write_body_string(&chart);
        let lines: Vec<&str> = body.lines().collect();
        assert_eq!(lines.len(), 8); // 8 rows for the one measure
        assert_eq!(lines[0], "0000");
        assert_eq!(lines[1], "1000");
    }

    #[test]
    fn picks_12_row_quantize_for_third_beat_note() {
        // Note at beat 1/3 needs rows divisible by 12.
        let chart = crate::model::Chart {
            style: Style::Single,
            difficulty: Difficulty::Basic,
            notes: vec![Note {
                beat: Beat::from_rational(Rational::new(1, 3).unwrap()),
                kind: NoteKind::Tap,
                panels: PanelSet::from_bits(Style::Single, 0x01),
            }],
        };
        let body = write_body_string(&chart);
        let lines: Vec<&str> = body.lines().collect();
        assert_eq!(lines.len(), 12);
        assert_eq!(lines[1], "1000");
    }

    // ---------- parse → write → parse round-trip ----------

    #[test]
    fn roundtrip_taps_and_holds_single() {
        let original = "\
1000
0000
0020
0000
,
0000
0030
0000
1000
";
        let notes = parse_notes_body(original, Style::Single).unwrap();
        let chart = crate::model::Chart {
            style: Style::Single,
            difficulty: Difficulty::Basic,
            notes,
        };
        let written = write_body_string(&chart);
        let re_parsed = parse_notes_body(&written, Style::Single).unwrap();
        assert_eq!(re_parsed.len(), chart.notes.len());
        for (a, b) in re_parsed.iter().zip(chart.notes.iter()) {
            assert_eq!(a.beat, b.beat);
            assert_eq!(a.kind, b.kind);
            assert_eq!(a.panels.bits(), b.panels.bits());
        }
    }

    #[test]
    fn roundtrip_shock_bothsides_double() {
        let original = "\
MMMMMMMM
00000000
00000000
00000000
";
        let notes = parse_notes_body(original, Style::Double).unwrap();
        let chart = crate::model::Chart {
            style: Style::Double,
            difficulty: Difficulty::Challenge,
            notes,
        };
        let written = write_body_string(&chart);
        let re_parsed = parse_notes_body(&written, Style::Double).unwrap();
        assert_eq!(re_parsed.len(), 1);
        assert_eq!(
            re_parsed[0].kind,
            NoteKind::Shock {
                side: ShockSide::BothSides
            }
        );
    }

    #[test]
    fn write_notedata_emits_section_with_tags() {
        let chart = crate::model::Chart {
            style: Style::Single,
            difficulty: Difficulty::Basic,
            notes: vec![Note {
                beat: Beat::from_rational(Rational::zero()),
                kind: NoteKind::Tap,
                panels: PanelSet::from_bits(Style::Single, 0x01),
            }],
        };
        let mut buf: Vec<u8> = Vec::new();
        write_notedata(&chart, &mut buf).unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert!(s.starts_with("#NOTEDATA:;\n"));
        assert!(s.contains("#STEPSTYPE:dance-single;\n"));
        assert!(s.contains("#DIFFICULTY:Easy;\n"));
        assert!(s.contains("#NOTES:\n"));
        assert!(s.contains("1000\n"));
        assert!(s.trim_end().ends_with(';'));
    }

    #[test]
    fn shock_p1_only_on_single_style_is_rejected_by_writer() {
        // Construct an invalid combination directly and verify the writer
        // catches it rather than silently producing broken output.
        let chart = crate::model::Chart {
            style: Style::Single,
            difficulty: Difficulty::Basic,
            notes: vec![Note {
                beat: Beat::from_rational(Rational::zero()),
                kind: NoteKind::Shock {
                    side: ShockSide::P1Only,
                },
                panels: PanelSet::empty(),
            }],
        };
        let mut buf: Vec<u8> = Vec::new();
        let err = write_notes_body(&chart, &mut buf).unwrap_err();
        assert!(matches!(
            err,
            SscError::ShockSideIncompatibleWithStyle { .. }
        ));
    }
}
