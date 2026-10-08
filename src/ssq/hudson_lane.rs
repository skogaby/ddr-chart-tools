//! Hudson lane-format type 16 step chunks (DDR Hottest Party 4 and 5).
//!
//! Byte layout and gameplay semantics are specified in
//! `docs/hudson_lane_ssq_format.md`; section numbers below refer to it.
//!
//! Unlike type 3 / type 9, a type 16 row describes **one lane**, not a
//! panel mask: `{tick, lane, item}`. Rows sharing a tick are merged by the
//! game into one step row (§2.2). Item `(0, 0)` is a plain arrow and
//! `(0, 1)` is a freeze-end marker for that lane (§3).
//!
//! Only the foot-panel styles (`param2` low byte `0x14` Single / `0x18`
//! Double) are decoded. The other style codes HP4 ships select Wii
//! Remote / Balance Board modes whose lanes are not dance panels (§1.1);
//! [`is_foot_chart`] tells them apart so the caller can drop them.
//!
//! This module does not own the HP1 / Mario Mix type 9 chart; that is
//! `ssq::hudson`.

use crate::model::{Chart, Style};
use crate::util::io::LeReader;

use super::chunk::ChunkHeader;
use super::steps::{decode_difficulty_code, resolve_notes, FreezeEntry};
use super::SsqError;

/// Lane byte meaning "no note on this row" (§2.1).
const LANE_NONE: u8 = 0xFF;
/// Item type meaning "no note on this row" (§3).
const ITEM_NONE: i16 = -1;
/// Item parameter on a type-0 item marking a freeze end (§3).
const PARAM_FREEZE_END: u16 = 1;
/// Size of one `{i16 type, u16 param}` item record (§2.1).
const ITEM_SIZE: usize = 4;

/// Whether a type 16 chunk is a foot-panel chart this tool can model.
#[must_use]
pub fn is_foot_chart(header: &ChunkHeader) -> bool {
    header.ty == 16 && decode_difficulty_code(header.param2, 0).is_ok()
}

/// What the parser rewrote or skipped, for logging.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LaneReport {
    /// Rows the game ignores: lane `0xFF` or item type `-1`.
    pub skipped_rows: usize,
    /// Rows naming a lane beyond the style's panel count.
    pub out_of_range_lanes: usize,
    /// Arrows carrying a gimmick item (type != 0), kept as plain arrows.
    pub gimmick_arrows: usize,
    /// Type-0 items with a parameter other than 0 or 1, kept as arrows.
    pub unknown_params: usize,
}

/// Decode a type 16 foot chart into a model [`Chart`], logging anything
/// that was not a plain arrow or freeze end.
pub fn parse_steps_chunk(
    header: &ChunkHeader,
    body: &[u8],
    chunk_offset: usize,
) -> Result<Chart, SsqError> {
    let (chart, report) = parse_chunk(header, body, chunk_offset)?;
    if report.gimmick_arrows + report.unknown_params > 0 {
        log::warn!(
            "lane step chunk at byte {chunk_offset} ({:?} {:?}): kept {} gimmick-item arrows and {} unknown-parameter arrows as plain arrows; foot charts in the HP4/HP5 reference corpus never carry these",
            chart.style,
            chart.difficulty,
            report.gimmick_arrows,
            report.unknown_params,
        );
    }
    if report.out_of_range_lanes > 0 {
        log::warn!(
            "lane step chunk at byte {chunk_offset} ({:?} {:?}): dropped {} rows on lanes outside the {}-panel layout",
            chart.style,
            chart.difficulty,
            report.out_of_range_lanes,
            chart.style.panel_count(),
        );
    }
    if report.skipped_rows > 0 {
        log::debug!(
            "lane step chunk at byte {chunk_offset}: skipped {} empty rows as the game does",
            report.skipped_rows
        );
    }
    Ok(chart)
}

/// One decoded row (§2.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LaneRow {
    tick: i32,
    lane: u8,
    item_type: i16,
    item_param: u16,
}

/// Decode a type 16 chunk (§2) and fold its per-lane rows into a chart.
pub fn parse_chunk(
    header: &ChunkHeader,
    body: &[u8],
    chunk_offset: usize,
) -> Result<(Chart, LaneReport), SsqError> {
    let (style, difficulty) = decode_difficulty_code(header.param2, chunk_offset)?;
    let rows = split_body(body, usize::from(header.param3), chunk_offset)?;
    let mut report = LaneReport::default();
    let (ticks, step_bytes, freeze_entries) = fold_rows(style, &rows, &mut report);
    let notes = resolve_notes(style, &ticks, &step_bytes, &freeze_entries, chunk_offset)?;
    Ok((
        Chart {
            style,
            difficulty,
            notes,
        },
        report,
    ))
}

/// Read the three body sections (§2.1): ticks, lane bytes (dword-padded),
/// item records.
fn split_body(
    body: &[u8],
    row_count: usize,
    chunk_offset: usize,
) -> Result<Vec<LaneRow>, SsqError> {
    let malformed = |reason: String| SsqError::MalformedChunk {
        offset: chunk_offset,
        reason,
    };
    let tick_bytes = row_count
        .checked_mul(4)
        .ok_or_else(|| malformed(format!("row count {row_count} overflows body size")))?;
    let lanes_start = tick_bytes;
    let items_start = lanes_start + round_up_to_dword(row_count);
    let items_end = items_start
        .checked_add(row_count * ITEM_SIZE)
        .ok_or_else(|| malformed(format!("row count {row_count} overflows body size")))?;
    if body.len() < items_end {
        return Err(malformed(format!(
            "lane step chunk body size {} is too small for {row_count} rows (needs {items_end})",
            body.len()
        )));
    }

    let mut ticks = LeReader::new(&body[..tick_bytes]);
    let lanes = &body[lanes_start..lanes_start + row_count];
    let mut items = LeReader::new(&body[items_start..items_end]);
    let mut rows = Vec::with_capacity(row_count);
    for &lane in lanes {
        let tick = ticks.read_u32().map_err(SsqError::Io)? as i32;
        let item_type = items.read_u16().map_err(SsqError::Io)? as i16;
        let item_param = items.read_u16().map_err(SsqError::Io)?;
        rows.push(LaneRow {
            tick,
            lane,
            item_type,
            item_param,
        });
    }
    Ok(rows)
}

fn round_up_to_dword(n: usize) -> usize {
    (n + 3) & !3
}

/// Merge per-lane rows into type-3-shaped `(tick, step byte, freeze
/// entry)` sequences the way the game's loader does (§2.2): rows sharing
/// a tick become one step mask and one freeze-end mask. The freeze-end
/// row is emitted before the step row at the same tick so a freeze that
/// ends where a new arrow starts closes the earlier head.
fn fold_rows(
    style: Style,
    rows: &[LaneRow],
    report: &mut LaneReport,
) -> (Vec<i32>, Vec<u8>, Vec<FreezeEntry>) {
    let panel_count = style.panel_count();
    let mut ticks = Vec::new();
    let mut step_bytes = Vec::new();
    let mut freeze_entries = Vec::new();

    let mut i = 0;
    while i < rows.len() {
        let tick = rows[i].tick;
        let mut step_mask = 0u8;
        let mut freeze_mask = 0u8;
        while i < rows.len() && rows[i].tick <= tick {
            let row = rows[i];
            i += 1;
            if row.lane == LANE_NONE || row.item_type == ITEM_NONE {
                report.skipped_rows += 1;
                continue;
            }
            if row.lane >= panel_count {
                report.out_of_range_lanes += 1;
                continue;
            }
            let bit = 1u8 << row.lane;
            match (row.item_type, row.item_param) {
                (0, 0) => step_mask |= bit,
                (0, PARAM_FREEZE_END) => freeze_mask |= bit,
                (0, _) => {
                    report.unknown_params += 1;
                    step_mask |= bit;
                }
                _ => {
                    report.gimmick_arrows += 1;
                    step_mask |= bit;
                }
            }
        }
        if freeze_mask != 0 {
            ticks.push(tick);
            step_bytes.push(0x00);
            freeze_entries.push(FreezeEntry {
                panels: freeze_mask,
                kind: 0x01,
            });
        }
        if step_mask != 0 {
            ticks.push(tick);
            step_bytes.push(step_mask);
        }
    }
    (ticks, step_bytes, freeze_entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Beat, Difficulty, NoteKind, Rational};

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    /// A row: `(tick, lane, item type, item param)`.
    type RowSpec = (i32, u8, i16, u16);

    /// Build a type 16 chunk (header + body) per §2.
    fn build_chunk(param2: u16, rows: &[RowSpec]) -> (ChunkHeader, Vec<u8>) {
        let mut body = Vec::new();
        for (tick, _, _, _) in rows {
            body.extend_from_slice(&tick.to_le_bytes());
        }
        for (_, lane, _, _) in rows {
            body.push(*lane);
        }
        while !body.len().is_multiple_of(4) {
            body.push(0);
        }
        for (_, _, ty, param) in rows {
            body.extend_from_slice(&ty.to_le_bytes());
            body.extend_from_slice(&param.to_le_bytes());
        }
        let header = ChunkHeader {
            length: (12 + body.len()) as u32,
            ty: 16,
            param2,
            param3: rows.len() as u16,
            param4: 0,
        };
        (header, body)
    }

    fn hold(ticks: i64) -> NoteKind {
        NoteKind::HoldHead {
            length: Beat::from_rational(Rational::new(ticks, 1024).unwrap()),
        }
    }

    #[test]
    fn foot_chart_detection_uses_style_code() {
        let (foot, _) = build_chunk(0x0114, &[]);
        let (hand, _) = build_chunk(0x011a, &[]);
        let (balance, _) = build_chunk(0x0324, &[]);
        assert!(is_foot_chart(&foot));
        assert!(!is_foot_chart(&hand));
        assert!(!is_foot_chart(&balance));
    }

    #[test]
    fn rows_sharing_a_tick_become_one_jump() -> TestResult {
        let (h, body) = build_chunk(0x0114, &[(1024, 0, 0, 0), (1024, 3, 0, 0), (2048, 1, 0, 0)]);
        let chart = parse_steps_chunk(&h, &body, 0)?;
        assert_eq!(chart.style, Style::Single);
        assert_eq!(chart.difficulty, Difficulty::Basic);
        assert_eq!(chart.notes.len(), 2);
        assert_eq!(chart.notes[0].panels.bits(), 0x09);
        assert_eq!(chart.notes[0].kind, NoteKind::Tap);
        assert_eq!(chart.notes[1].panels.bits(), 0x02);
        Ok(())
    }

    #[test]
    fn lane_bytes_are_dword_padded() -> TestResult {
        // 5 rows → 3 pad bytes between lanes and items.
        let rows: Vec<RowSpec> = (0..5).map(|i| (i * 512, (i % 4) as u8, 0, 0)).collect();
        let (h, body) = build_chunk(0x0114, &rows);
        assert_eq!(body.len(), 5 * 4 + 8 + 5 * 4);
        let chart = parse_steps_chunk(&h, &body, 0)?;
        assert_eq!(chart.notes.len(), 5);
        Ok(())
    }

    #[test]
    fn freeze_end_item_closes_the_lane_head() -> TestResult {
        let (h, body) = build_chunk(0x0114, &[(1024, 2, 0, 0), (3072, 2, 0, 1)]);
        let chart = parse_steps_chunk(&h, &body, 0)?;
        assert_eq!(chart.notes.len(), 1);
        assert_eq!(chart.notes[0].panels.bits(), 0x04);
        assert_eq!(chart.notes[0].kind, hold(2048));
        Ok(())
    }

    #[test]
    fn freeze_end_at_a_new_arrow_tick_closes_the_earlier_head() -> TestResult {
        // Hold on Left ends exactly where the next Left arrow starts.
        let (h, body) = build_chunk(0x0114, &[(0, 0, 0, 0), (1024, 0, 0, 0), (1024, 0, 0, 1)]);
        let chart = parse_steps_chunk(&h, &body, 0)?;
        assert_eq!(chart.notes.len(), 2);
        assert_eq!(chart.notes[0].kind, hold(1024));
        assert_eq!(chart.notes[1].kind, NoteKind::Tap);
        assert_eq!(chart.notes[1].beat, Beat::from_measure_ticks(1024)?);
        Ok(())
    }

    #[test]
    fn freeze_end_without_head_is_rejected() {
        let (h, body) = build_chunk(0x0114, &[(1024, 1, 0, 1)]);
        let err = parse_steps_chunk(&h, &body, 0).unwrap_err();
        assert!(matches!(err, SsqError::FreezeWithoutHead { .. }));
    }

    #[test]
    fn empty_rows_are_skipped_like_the_game() -> TestResult {
        let (h, body) = build_chunk(
            0x0114,
            &[(0, LANE_NONE, 0, 0), (0, 1, ITEM_NONE, 0), (0, 2, 0, 0)],
        );
        let (chart, report) = parse_chunk(&h, &body, 0)?;
        assert_eq!(report.skipped_rows, 2);
        assert_eq!(chart.notes.len(), 1);
        assert_eq!(chart.notes[0].panels.bits(), 0x04);
        Ok(())
    }

    #[test]
    fn gimmick_items_are_kept_as_arrows_and_counted() -> TestResult {
        let (h, body) = build_chunk(0x0114, &[(0, 0, 3, 7), (512, 1, 0, 9)]);
        let (chart, report) = parse_chunk(&h, &body, 0)?;
        assert_eq!(report.gimmick_arrows, 1);
        assert_eq!(report.unknown_params, 1);
        assert_eq!(chart.notes.len(), 2);
        assert!(chart.notes.iter().all(|n| n.kind == NoteKind::Tap));
        Ok(())
    }

    #[test]
    fn single_drops_lanes_above_three() -> TestResult {
        let (h, body) = build_chunk(0x0114, &[(0, 5, 0, 0), (0, 0, 0, 0)]);
        let (chart, report) = parse_chunk(&h, &body, 0)?;
        assert_eq!(report.out_of_range_lanes, 1);
        assert_eq!(chart.notes[0].panels.bits(), 0x01);
        Ok(())
    }

    #[test]
    fn double_uses_eight_lanes() -> TestResult {
        let (h, body) = build_chunk(0x0218, &[(0, 7, 0, 0), (0, 4, 0, 0)]);
        let chart = parse_steps_chunk(&h, &body, 0)?;
        assert_eq!(chart.style, Style::Double);
        assert_eq!(chart.notes[0].panels.bits(), 0x90);
        Ok(())
    }

    #[test]
    fn short_body_is_malformed() {
        let (h, mut body) = build_chunk(0x0114, &[(0, 0, 0, 0)]);
        body.truncate(body.len() - 1);
        let err = parse_steps_chunk(&h, &body, 0x40).unwrap_err();
        assert!(matches!(err, SsqError::MalformedChunk { offset: 0x40, .. }));
    }

    #[test]
    fn rejects_non_foot_style() {
        let (h, body) = build_chunk(0x011a, &[]);
        let err = parse_steps_chunk(&h, &body, 0).unwrap_err();
        assert!(matches!(err, SsqError::InvalidDifficultyCode { .. }));
    }
}
