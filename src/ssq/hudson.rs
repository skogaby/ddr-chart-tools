//! Hudson-format type 9 step chunks (DDR Hottest Party 1, DDR Mario Mix).
//!
//! Byte layout and gameplay semantics are specified in
//! `docs/hudson_ssq_format.md`; section numbers below refer to it.
//!
//! This module owns two steps:
//!
//! 1. [`parse_chunk`] decodes the chunk into a [`HudsonChart`]: rows, the
//!    `CHAR` chara-note table, and the `FREZ` freeze-end table.
//! 2. [`HudsonChart::neutralize`] turns that into a model [`Chart`] the way
//!    the game does with Groove Gimmick and hand markers off (§4). Gimmick
//!    items become plain arrows, "Koopa" items gain a one-beat echo arrow,
//!    hazards are removed, and freezes are resolved in game note-list order.
//!
//! It does not own the arcade type 9 metadata chunk (`ssq_format.md` §8). Use
//! [`is_step_chunk`] to tell the two apart before calling in here.

use std::collections::BTreeMap;

use crate::model::{Beat, Chart, Difficulty, Note, NoteKind, PanelSet, Style};
use crate::util::io::LeReader;

use super::chunk::ChunkHeader;
use super::steps::decode_difficulty_code;
use super::SsqError;

/// Sub-block magic of the chara-note table (§3.1).
const CHAR_MAGIC: [u8; 4] = *b"CHAR";
/// Sub-block magic of the freeze-end table (§3.2).
const FREZ_MAGIC: [u8; 4] = *b"FREZ";
/// Magic of the extra block (§3). The game does not check it.
const EXDT_MAGIC: [u8; 4] = *b"EXDT";
/// Size of the `{magic, length}` header on the extra block and sub-blocks.
const BLOCK_HEADER_SIZE: usize = 8;
/// Size of one `CHAR` entry (§3.1).
const CHAR_ENTRY_SIZE: usize = 12;
/// Size of one `FREZ` entry (§3.2).
const FREZ_ENTRY_SIZE: usize = 3;

/// Item type 1: Wii Remote hand marker. A plain arrow with markers off.
const ITEM_HAND_MARKER: u16 = 1;
/// Item type 2: "Koopa" double arrow; gains an echo arrow (§4).
const ITEM_ECHO: u16 = 2;
/// Item types 5 and 14: hazards, removed with gimmicks off (§4).
const HAZARD_ITEMS: [u16; 2] = [5, 14];
/// Echo arrows land this many measure-ticks (one beat) after their source.
const ECHO_DELAY_TICKS: i32 = 0x400;
/// The game builds at most this many echo arrows per row (§4).
const MAX_ECHOES_PER_ROW: usize = 2;

/// One row of a Hudson step chunk (§2.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HudsonRow {
    /// A step row. `items[bit]` is the item byte for the arrow on panel bit
    /// `bit`: `0` = plain arrow, `k >= 1` = `chara_notes[k - 1]`. Entries
    /// for unset bits are 0.
    Step {
        tick: i32,
        panels: u8,
        items: [u8; 8],
    },
    /// A freeze-end row. `panels` comes from the matching `FREZ` entry, or
    /// is 0 when the chunk has no `FREZ` table.
    FreezeEnd { tick: i32, panels: u8 },
}

/// One entry of the `CHAR` table (§3.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CharaNote {
    /// Item type (§4).
    pub item: u16,
    pub param1: i32,
    pub param2: i32,
}

/// A decoded Hudson type 9 step chunk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HudsonChart {
    pub style: Style,
    pub difficulty: Difficulty,
    pub rows: Vec<HudsonRow>,
    pub chara_notes: Vec<CharaNote>,
    /// True when freeze-end rows exist but the chunk has no `FREZ` table.
    /// The game treats those rows as no-ops.
    pub missing_freeze_table: bool,
}

/// What [`HudsonChart::neutralize`] changed, for logging.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NeutralizeReport {
    /// Hand-marker arrows (item 1) kept as plain arrows.
    pub hand_markers: usize,
    /// Other gimmick-item arrows kept as plain arrows.
    pub gimmick_arrows: usize,
    /// Echo arrows added for item 2.
    pub echoes_added: usize,
    /// Hazard arrows (items 5 and 14) removed.
    pub hazards_removed: usize,
    /// Freezes lost because their head was a removed hazard or the freeze
    /// had no length.
    pub freezes_dropped: usize,
    /// Freeze-end panels with no earlier note on that panel.
    pub orphan_freeze_ends: usize,
    /// Arrows that landed on a panel and tick already taken, merged away.
    pub collisions_merged: usize,
}

impl NeutralizeReport {
    /// Whether any source data was lost (as opposed to rewritten).
    #[must_use]
    pub fn dropped_anything(&self) -> bool {
        self.hazards_removed
            + self.freezes_dropped
            + self.orphan_freeze_ends
            + self.collisions_merged
            > 0
    }
}

/// Whether a type 9 chunk is a Hudson step chunk rather than the arcade
/// metadata chunk. Hudson charts carry a step difficulty code in `param2`
/// (§1); the arcade chunk does not.
#[must_use]
pub fn is_step_chunk(header: &ChunkHeader) -> bool {
    header.ty == 9 && decode_difficulty_code(header.param2, 0).is_ok()
}

/// Decode and neutralize a Hudson step chunk into a model [`Chart`],
/// logging what the neutralization changed.
pub fn parse_steps_chunk(
    header: &ChunkHeader,
    body: &[u8],
    chunk_offset: usize,
) -> Result<Chart, SsqError> {
    let hudson = parse_chunk(header, body, chunk_offset)?;
    if hudson.missing_freeze_table {
        log::warn!(
            "Hudson step chunk at byte {chunk_offset}: freeze-end rows without a FREZ table; ignoring them as the game does"
        );
    }
    let (chart, report) = hudson.neutralize(chunk_offset)?;
    log::info!(
        "Hudson step chunk at byte {chunk_offset} ({:?} {:?}): gimmicks off — {} hand markers and {} gimmick arrows kept as arrows, {} echo arrows added",
        chart.style,
        chart.difficulty,
        report.hand_markers,
        report.gimmick_arrows,
        report.echoes_added,
    );
    if report.dropped_anything() {
        log::warn!(
            "Hudson step chunk at byte {chunk_offset} ({:?} {:?}): dropped {} hazard arrows, {} freezes, {} orphan freeze ends, {} colliding arrows",
            chart.style,
            chart.difficulty,
            report.hazards_removed,
            report.freezes_dropped,
            report.orphan_freeze_ends,
            report.collisions_merged,
        );
    }
    Ok(chart)
}

/// Decode a Hudson type 9 step chunk (§1–§3).
///
/// `body` is the chunk after its 12-byte header; `chunk_offset` is the
/// chunk's byte offset in the file, used for error locations.
pub fn parse_chunk(
    header: &ChunkHeader,
    body: &[u8],
    chunk_offset: usize,
) -> Result<HudsonChart, SsqError> {
    let (style, difficulty) = decode_difficulty_code(header.param2, chunk_offset)?;
    let row_count = usize::from(header.param3);
    let malformed = |reason: String| SsqError::MalformedChunk {
        offset: chunk_offset,
        reason,
    };

    let mut reader = LeReader::new(body);
    let extra_offset = reader.read_u32().map_err(SsqError::Io)? as usize;
    let mut ticks = Vec::with_capacity(row_count);
    for _ in 0..row_count {
        ticks.push(reader.read_u32().map_err(SsqError::Io)? as i32);
    }

    let mut rows = Vec::with_capacity(row_count);
    let mut freeze_rows = 0usize;
    for (i, &tick) in ticks.iter().enumerate() {
        let panels = reader
            .read_u8()
            .map_err(|e| malformed(format!("row {i} mask: {e}")))?;
        if panels == 0 {
            freeze_rows += 1;
            rows.push(HudsonRow::FreezeEnd { tick, panels: 0 });
            continue;
        }
        let mut items = [0u8; 8];
        for (bit, item) in items.iter_mut().enumerate() {
            if panels & (1 << bit) != 0 {
                *item = reader.read_u8().map_err(|e| {
                    malformed(format!("row {i} item byte for panel bit {bit}: {e}"))
                })?;
            }
        }
        rows.push(HudsonRow::Step {
            tick,
            panels,
            items,
        });
    }

    let SubBlocks {
        chara: chara_bytes,
        frez: frez_bytes,
    } = if extra_offset == 0 {
        SubBlocks::default()
    } else {
        locate_sub_blocks(body, extra_offset, chunk_offset)?
    };

    let chara_notes = match chara_bytes {
        Some(bytes) => parse_chara_table(bytes, chunk_offset)?,
        None => Vec::new(),
    };
    for (i, row) in rows.iter().enumerate() {
        if let HudsonRow::Step { items, .. } = row {
            if let Some(&bad) = items.iter().find(|&&k| usize::from(k) > chara_notes.len()) {
                return Err(malformed(format!(
                    "row {i} references chara note {bad}, but the CHAR table has {} entries",
                    chara_notes.len()
                )));
            }
        }
    }

    let missing_freeze_table = freeze_rows > 0 && frez_bytes.is_none();
    if let Some(bytes) = frez_bytes {
        let needed = freeze_rows * FREZ_ENTRY_SIZE;
        if bytes.len() < needed {
            return Err(malformed(format!(
                "FREZ table holds {} bytes but {freeze_rows} freeze-end rows need {needed}",
                bytes.len()
            )));
        }
        let mut entries = bytes.chunks_exact(FREZ_ENTRY_SIZE);
        for row in &mut rows {
            if let HudsonRow::FreezeEnd { panels, .. } = row {
                // `needed <= bytes.len()` above guarantees one entry per row.
                if let Some(entry) = entries.next() {
                    *panels = entry[0];
                }
            }
        }
    }

    Ok(HudsonChart {
        style,
        difficulty,
        rows,
        chara_notes,
        missing_freeze_table,
    })
}

/// Payloads of the extra block's sub-blocks (§3), each absent if the chunk
/// does not carry it.
#[derive(Debug, Default)]
struct SubBlocks<'a> {
    chara: Option<&'a [u8]>,
    frez: Option<&'a [u8]>,
}

/// Find the `CHAR` and `FREZ` payloads inside the extra block (§3).
/// `extra_offset` is relative to chunk start + 0x10, i.e. body + 4.
fn locate_sub_blocks(
    body: &[u8],
    extra_offset: usize,
    chunk_offset: usize,
) -> Result<SubBlocks<'_>, SsqError> {
    let malformed = |reason: String| SsqError::MalformedChunk {
        offset: chunk_offset,
        reason,
    };
    let start = extra_offset
        .checked_add(4)
        .ok_or_else(|| malformed(format!("extra block offset {extra_offset} overflows")))?;
    let (magic, block_len) = read_block_header(body, start).ok_or_else(|| {
        malformed(format!(
            "extra block header at body offset {start} is out of bounds"
        ))
    })?;
    if magic != EXDT_MAGIC {
        log::warn!(
            "Hudson step chunk at byte {chunk_offset}: extra block magic is {magic:?}, expected \"EXDT\"; continuing as the game does"
        );
    }
    let end = start
        .checked_add(block_len)
        .filter(|&end| block_len >= BLOCK_HEADER_SIZE && end <= body.len())
        .ok_or_else(|| {
            malformed(format!(
                "extra block at body offset {start} declares length {block_len}, past the {}-byte body",
                body.len()
            ))
        })?;

    let mut blocks = SubBlocks::default();
    let mut pos = start + BLOCK_HEADER_SIZE;
    while pos < end {
        let (magic, sub_len) = read_block_header(&body[..end], pos).ok_or_else(|| {
            malformed(format!(
                "extra sub-block header at body offset {pos} is out of bounds"
            ))
        })?;
        let sub_end = pos
            .checked_add(sub_len)
            .filter(|&e| sub_len >= BLOCK_HEADER_SIZE && e <= end)
            .ok_or_else(|| {
                malformed(format!(
                    "extra sub-block {magic:?} at body offset {pos} declares length {sub_len}, outside the extra block"
                ))
            })?;
        let payload = &body[pos + BLOCK_HEADER_SIZE..sub_end];
        // The game returns the first sub-block with a matching magic.
        match magic {
            CHAR_MAGIC if blocks.chara.is_none() => blocks.chara = Some(payload),
            FREZ_MAGIC if blocks.frez.is_none() => blocks.frez = Some(payload),
            _ => log::debug!(
                "Hudson step chunk at byte {chunk_offset}: skipping extra sub-block {magic:?}"
            ),
        }
        pos = sub_end;
    }
    Ok(blocks)
}

/// Read a `{4-byte magic, u32 length}` block header at `pos`, or `None` if
/// it does not fit.
fn read_block_header(bytes: &[u8], pos: usize) -> Option<([u8; 4], usize)> {
    let header = bytes.get(pos..pos.checked_add(BLOCK_HEADER_SIZE)?)?;
    let magic = [header[0], header[1], header[2], header[3]];
    let len = u32::from_le_bytes([header[4], header[5], header[6], header[7]]);
    Some((magic, len as usize))
}

/// Decode the `CHAR` payload into entries (§3.1).
fn parse_chara_table(bytes: &[u8], chunk_offset: usize) -> Result<Vec<CharaNote>, SsqError> {
    if !bytes.len().is_multiple_of(CHAR_ENTRY_SIZE) {
        return Err(SsqError::MalformedChunk {
            offset: chunk_offset,
            reason: format!(
                "CHAR table payload of {} bytes is not a whole number of {CHAR_ENTRY_SIZE}-byte entries",
                bytes.len()
            ),
        });
    }
    let mut reader = LeReader::new(bytes);
    let mut notes = Vec::with_capacity(bytes.len() / CHAR_ENTRY_SIZE);
    for _ in 0..bytes.len() / CHAR_ENTRY_SIZE {
        let item = reader.read_u16().map_err(SsqError::Io)?;
        let _unused = reader.read_u16().map_err(SsqError::Io)?;
        let param1 = reader.read_u32().map_err(SsqError::Io)? as i32;
        let param2 = reader.read_u32().map_err(SsqError::Io)? as i32;
        notes.push(CharaNote {
            item,
            param1,
            param2,
        });
    }
    Ok(notes)
}

/// How the game treats one arrow with gimmicks and hand markers off (§4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Disposition {
    Arrow,
    ArrowWithEcho,
    Removed,
}

fn gimmicks_off_disposition(item: Option<u16>) -> Disposition {
    match item {
        Some(ITEM_ECHO) => Disposition::ArrowWithEcho,
        Some(t) if HAZARD_ITEMS.contains(&t) => Disposition::Removed,
        _ => Disposition::Arrow,
    }
}

/// One single-panel note in the game's note list, before sorting.
#[derive(Debug, Clone, Copy)]
struct ListNote {
    tick: i32,
    panel: u8,
    removed: bool,
    hold_end: Option<i32>,
}

impl HudsonChart {
    /// Convert to a model chart with Groove Gimmick and hand markers off
    /// (§4), mirroring the game's note builder (`FUN_8016137c`) and freeze
    /// resolver (`FUN_80161c08`).
    ///
    /// `chunk_offset` is used only for error locations.
    pub fn neutralize(&self, chunk_offset: usize) -> Result<(Chart, NeutralizeReport), SsqError> {
        let mut report = NeutralizeReport::default();
        let panel_count = self.style.panel_count();
        let mut list: Vec<ListNote> = Vec::new();

        for row in &self.rows {
            match *row {
                HudsonRow::Step {
                    tick,
                    panels,
                    items,
                } => {
                    let mut echoes = Vec::new();
                    for panel in (0..panel_count).filter(|p| panels & (1 << p) != 0) {
                        let item = match items[usize::from(panel)] {
                            0 => None,
                            // `parse_chunk` checked every index against the table.
                            k => Some(self.chara_notes[usize::from(k) - 1].item),
                        };
                        match item {
                            Some(ITEM_HAND_MARKER) => report.hand_markers += 1,
                            Some(t) if !HAZARD_ITEMS.contains(&t) => report.gimmick_arrows += 1,
                            _ => {}
                        }
                        let disposition = gimmicks_off_disposition(item);
                        if disposition == Disposition::ArrowWithEcho
                            && echoes.len() < MAX_ECHOES_PER_ROW
                        {
                            let echo_tick = tick.checked_add(ECHO_DELAY_TICKS).ok_or(
                                SsqError::MalformedChunk {
                                    offset: chunk_offset,
                                    reason: format!("echo arrow after tick {tick} overflows"),
                                },
                            )?;
                            echoes.push(ListNote {
                                tick: echo_tick,
                                panel,
                                removed: false,
                                hold_end: None,
                            });
                        }
                        if disposition == Disposition::Removed {
                            report.hazards_removed += 1;
                        }
                        list.push(ListNote {
                            tick,
                            panel,
                            removed: disposition == Disposition::Removed,
                            hold_end: None,
                        });
                    }
                    report.echoes_added += echoes.len();
                    list.extend(echoes);
                }
                HudsonRow::FreezeEnd { tick, panels } => {
                    for panel in (0..panel_count).filter(|p| panels & (1 << p) != 0) {
                        match list.iter_mut().rev().find(|n| n.panel == panel) {
                            // The game rewrites the length if a head is closed twice.
                            Some(head) => head.hold_end = Some(tick),
                            None => report.orphan_freeze_ends += 1,
                        }
                    }
                }
            }
        }

        let notes = build_notes(self.style, &list, &mut report, chunk_offset)?;
        Ok((
            Chart {
                style: self.style,
                difficulty: self.difficulty,
                notes,
            },
            report,
        ))
    }
}

/// Collapse the game note list into beat-sorted model notes: taps sharing
/// a tick become one row, holds sharing a tick and end become one hold.
fn build_notes(
    style: Style,
    list: &[ListNote],
    report: &mut NeutralizeReport,
    chunk_offset: usize,
) -> Result<Vec<Note>, SsqError> {
    // (tick, panel) -> hold end, if any. A hold wins over a tap on the
    // same cell.
    let mut cells: BTreeMap<(i32, u8), Option<i32>> = BTreeMap::new();
    for note in list {
        if note.removed {
            if note.hold_end.is_some() {
                report.freezes_dropped += 1;
            }
            continue;
        }
        let hold_end = match note.hold_end {
            Some(end) if end > note.tick => Some(end),
            Some(_) => {
                report.freezes_dropped += 1;
                None
            }
            None => None,
        };
        match cells.get_mut(&(note.tick, note.panel)) {
            Some(existing) => {
                report.collisions_merged += 1;
                if existing.is_none() {
                    *existing = hold_end;
                }
            }
            None => {
                cells.insert((note.tick, note.panel), hold_end);
            }
        }
    }

    // (tick, kind order: taps then holds, hold end) -> panel mask.
    let mut rows: BTreeMap<(i32, u8, i32), u8> = BTreeMap::new();
    for (&(tick, panel), &hold_end) in &cells {
        let key = match hold_end {
            None => (tick, 0, 0),
            Some(end) => (tick, 1, end),
        };
        *rows.entry(key).or_insert(0) |= 1 << panel;
    }

    let beat = |ticks: i64| {
        Beat::from_measure_ticks(ticks).map_err(|e| SsqError::MalformedChunk {
            offset: chunk_offset,
            reason: format!("invalid tick {ticks}: {e}"),
        })
    };
    rows.into_iter()
        .map(|((tick, order, end), mask)| {
            let kind = if order == 0 {
                NoteKind::Tap
            } else {
                NoteKind::HoldHead {
                    length: beat(i64::from(end) - i64::from(tick))?,
                }
            };
            Ok(Note {
                beat: beat(i64::from(tick))?,
                kind,
                panels: PanelSet::from_bits(style, mask),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    /// A row for [`build_chunk`]: `(tick, mask, item bytes in bit order)`.
    /// Mask 0 is a freeze-end row and takes no item bytes.
    type RowSpec = (i32, u8, Vec<u8>);

    /// Build a Hudson type 9 chunk (header + body) per §1–§3. `chara` lists
    /// item types; `frez` lists freeze-end panel masks in row order.
    /// Sub-blocks are emitted only when non-empty.
    fn build_chunk(
        param2: u16,
        rows: &[RowSpec],
        chara: &[u16],
        frez: &[u8],
    ) -> (ChunkHeader, Vec<u8>) {
        let mut body = vec![0u8; 4]; // extra_offset, patched below
        for (tick, _, _) in rows {
            body.extend_from_slice(&tick.to_le_bytes());
        }
        for (_, mask, items) in rows {
            body.push(*mask);
            body.extend_from_slice(items);
        }
        while !body.len().is_multiple_of(4) {
            body.push(0);
        }
        if !chara.is_empty() || !frez.is_empty() {
            let extra_start = body.len();
            let mut subs = Vec::new();
            if !chara.is_empty() {
                subs.extend_from_slice(&CHAR_MAGIC);
                subs.extend_from_slice(&((8 + 12 * chara.len()) as u32).to_le_bytes());
                for item in chara {
                    subs.extend_from_slice(&item.to_le_bytes());
                    subs.extend_from_slice(&[0u8; 10]);
                }
            }
            if !frez.is_empty() {
                let mut payload = Vec::new();
                for mask in frez {
                    payload.extend_from_slice(&[*mask, 1, 0]);
                }
                while !payload.len().is_multiple_of(4) {
                    payload.push(0);
                }
                subs.extend_from_slice(&FREZ_MAGIC);
                subs.extend_from_slice(&((8 + payload.len()) as u32).to_le_bytes());
                subs.extend_from_slice(&payload);
            }
            body.extend_from_slice(&EXDT_MAGIC);
            body.extend_from_slice(&((8 + subs.len()) as u32).to_le_bytes());
            body.extend_from_slice(&subs);
            let rel = (extra_start - 4) as u32;
            body[..4].copy_from_slice(&rel.to_le_bytes());
        }
        let header = ChunkHeader {
            length: (12 + body.len()) as u32,
            ty: 9,
            param2,
            param3: rows.len() as u16,
            param4: 0,
        };
        (header, body)
    }

    fn neutralize(
        rows: &[RowSpec],
        chara: &[u16],
        frez: &[u8],
    ) -> Result<(Chart, NeutralizeReport), SsqError> {
        let (h, body) = build_chunk(0x0114, rows, chara, frez);
        parse_chunk(&h, &body, 0)?.neutralize(0)
    }

    fn summary(chart: &Chart) -> Vec<(i64, u8, Option<i64>)> {
        chart
            .notes
            .iter()
            .map(|n| {
                let ticks = |b: Beat| {
                    let r = b.as_rational();
                    r.num() * Beat::TICKS_PER_BEAT / r.den() as i64
                };
                let len = match n.kind {
                    NoteKind::HoldHead { length } => Some(ticks(length)),
                    _ => None,
                };
                (ticks(n.beat), n.panels.bits(), len)
            })
            .collect()
    }

    #[test]
    fn step_chunk_gate_accepts_difficulty_codes_only() {
        let (mut h, _) = build_chunk(0x0314, &[], &[], &[]);
        assert!(is_step_chunk(&h));
        h.param2 = 0x0000; // arcade metadata chunk shape
        assert!(!is_step_chunk(&h));
        h.param2 = 0x0314;
        h.ty = 3;
        assert!(!is_step_chunk(&h));
    }

    #[test]
    fn decodes_rows_items_and_tables() -> TestResult {
        let rows = vec![
            (0, 0x03, vec![0, 1]),
            (1024, 0x08, vec![2]),
            (2048, 0, vec![]),
        ];
        let (h, body) = build_chunk(0x0214, &rows, &[9, 5], &[0x08]);
        let c = parse_chunk(&h, &body, 0)?;
        assert_eq!(
            (c.style, c.difficulty),
            (Style::Single, Difficulty::Difficult)
        );
        assert_eq!(
            c.chara_notes.iter().map(|n| n.item).collect::<Vec<_>>(),
            [9, 5]
        );
        let mut items = [0u8; 8];
        items[1] = 1;
        assert_eq!(
            c.rows[0],
            HudsonRow::Step {
                tick: 0,
                panels: 0x03,
                items
            }
        );
        assert_eq!(
            c.rows[2],
            HudsonRow::FreezeEnd {
                tick: 2048,
                panels: 0x08
            }
        );
        assert!(!c.missing_freeze_table);
        Ok(())
    }

    #[test]
    fn plain_chart_without_extra_block_decodes() -> TestResult {
        let (chart, report) = neutralize(&[(0, 0x01, vec![0]), (512, 0x06, vec![0, 0])], &[], &[])?;
        assert_eq!(summary(&chart), [(0, 0x01, None), (512, 0x06, None)]);
        assert_eq!(report, NeutralizeReport::default());
        Ok(())
    }

    #[test]
    fn gimmick_items_and_hand_markers_become_arrows() -> TestResult {
        // Item 1 (hand) on Left, items 3/4/7/9/10 elsewhere.
        let rows = vec![
            (0, 0x01, vec![1]),
            (256, 0x0A, vec![2, 3]),
            (512, 0x04, vec![4]),
            (768, 0x08, vec![5]),
        ];
        let (chart, report) = neutralize(&rows, &[1, 3, 4, 7, 10], &[])?;
        assert_eq!(
            summary(&chart),
            [
                (0, 0x01, None),
                (256, 0x0A, None),
                (512, 0x04, None),
                (768, 0x08, None)
            ]
        );
        assert_eq!(report.hand_markers, 1);
        assert_eq!(report.gimmick_arrows, 4);
        assert!(!report.dropped_anything());
        Ok(())
    }

    #[test]
    fn hazards_are_removed() -> TestResult {
        // Down + Up at tick 0; Up is a hazard (item 5). Right at 512 is item 14.
        let rows = vec![(0, 0x06, vec![0, 1]), (512, 0x08, vec![2])];
        let (chart, report) = neutralize(&rows, &[5, 14], &[])?;
        assert_eq!(summary(&chart), [(0, 0x02, None)]);
        assert_eq!(report.hazards_removed, 2);
        assert!(report.dropped_anything());
        Ok(())
    }

    #[test]
    fn echo_item_adds_arrow_one_beat_later() -> TestResult {
        let (chart, report) = neutralize(&[(100, 0x04, vec![1])], &[2], &[])?;
        assert_eq!(summary(&chart), [(100, 0x04, None), (1124, 0x04, None)]);
        assert_eq!(report.echoes_added, 1);
        Ok(())
    }

    #[test]
    fn echoes_are_capped_at_two_per_row() -> TestResult {
        let (chart, report) = neutralize(&[(0, 0x07, vec![1, 1, 1])], &[2], &[])?;
        // Echoes go to the first two panels in bit order (Left, Down).
        assert_eq!(summary(&chart), [(0, 0x07, None), (1024, 0x03, None)]);
        assert_eq!(report.echoes_added, 2);
        Ok(())
    }

    #[test]
    fn freeze_on_gimmick_head_survives() -> TestResult {
        let rows = vec![(0, 0x01, vec![1]), (2048, 0, vec![])];
        let (chart, _) = neutralize(&rows, &[1], &[0x01])?;
        assert_eq!(summary(&chart), [(0, 0x01, Some(2048))]);
        Ok(())
    }

    #[test]
    fn freeze_end_splits_row_into_tap_and_hold() -> TestResult {
        // Left+Right at 0; only Right is held.
        let rows = vec![(0, 0x09, vec![0, 0]), (1024, 0, vec![])];
        let (chart, _) = neutralize(&rows, &[], &[0x08])?;
        assert_eq!(summary(&chart), [(0, 0x01, None), (0, 0x08, Some(1024))]);
        Ok(())
    }

    #[test]
    fn freeze_resolves_in_game_list_order_not_time_order() -> TestResult {
        // Echo of Left lands at 1024 but is appended before the Left tap
        // at 512, so the list order is [0, echo@1024, 512]. A freeze end at
        // 2048 closes the most recent *list* entry: the tap at 512.
        let rows = vec![(0, 0x01, vec![1]), (512, 0x01, vec![0]), (2048, 0, vec![])];
        let (chart, _) = neutralize(&rows, &[2], &[0x01])?;
        assert_eq!(
            summary(&chart),
            [(0, 0x01, None), (512, 0x01, Some(1536)), (1024, 0x01, None)]
        );
        Ok(())
    }

    #[test]
    fn freeze_on_removed_hazard_is_dropped() -> TestResult {
        let rows = vec![(0, 0x02, vec![1]), (1024, 0, vec![])];
        let (chart, report) = neutralize(&rows, &[14], &[0x02])?;
        assert!(chart.notes.is_empty());
        assert_eq!(report.freezes_dropped, 1);
        Ok(())
    }

    #[test]
    fn orphan_freeze_end_is_counted_not_fatal() -> TestResult {
        let rows = vec![(0, 0x01, vec![0]), (1024, 0, vec![])];
        let (chart, report) = neutralize(&rows, &[], &[0x08])?;
        assert_eq!(summary(&chart), [(0, 0x01, None)]);
        assert_eq!(report.orphan_freeze_ends, 1);
        Ok(())
    }

    #[test]
    fn echo_colliding_with_existing_arrow_is_merged() -> TestResult {
        let rows = vec![(0, 0x01, vec![1]), (1024, 0x01, vec![0])];
        let (chart, report) = neutralize(&rows, &[2], &[])?;
        assert_eq!(summary(&chart), [(0, 0x01, None), (1024, 0x01, None)]);
        assert_eq!(report.collisions_merged, 1);
        Ok(())
    }

    #[test]
    fn freeze_rows_without_frez_table_are_ignored() -> TestResult {
        let rows = vec![(0, 0x01, vec![1]), (1024, 0, vec![])];
        let (h, body) = build_chunk(0x0114, &rows, &[3], &[]);
        let c = parse_chunk(&h, &body, 0)?;
        assert!(c.missing_freeze_table);
        let (chart, _) = c.neutralize(0)?;
        assert_eq!(summary(&chart), [(0, 0x01, None)]);
        Ok(())
    }

    #[test]
    fn item_index_past_char_table_is_malformed() {
        let (h, body) = build_chunk(0x0114, &[(0, 0x01, vec![2])], &[3], &[]);
        let err = parse_chunk(&h, &body, 40);
        assert!(
            matches!(err, Err(SsqError::MalformedChunk { offset: 40, .. })),
            "{err:?}"
        );
    }

    #[test]
    fn truncated_row_is_malformed() {
        let (h, mut body) = build_chunk(0x0114, &[(0, 0x03, vec![0, 0])], &[], &[]);
        body.truncate(4 + 4 + 2); // mask + one of two item bytes
        assert!(matches!(
            parse_chunk(&h, &body, 0),
            Err(SsqError::MalformedChunk { .. })
        ));
    }

    #[test]
    fn short_frez_table_is_malformed() {
        let rows = vec![(0, 0x03, vec![0, 0]), (512, 0, vec![]), (1024, 0, vec![])];
        let (h, body) = build_chunk(0x0114, &rows, &[], &[0x01]);
        assert!(matches!(
            parse_chunk(&h, &body, 0),
            Err(SsqError::MalformedChunk { .. })
        ));
    }

    #[test]
    fn extra_block_out_of_bounds_is_malformed() {
        let (h, mut body) = build_chunk(0x0114, &[(0, 0x01, vec![1])], &[3], &[]);
        body[..4].copy_from_slice(&0x1000u32.to_le_bytes());
        assert!(matches!(
            parse_chunk(&h, &body, 0),
            Err(SsqError::MalformedChunk { .. })
        ));
    }

    #[test]
    fn zero_length_sub_block_is_malformed_not_an_infinite_loop() {
        let (h, mut body) = build_chunk(0x0114, &[(0, 0x01, vec![1])], &[3], &[]);
        let rel = u32::from_le_bytes([body[0], body[1], body[2], body[3]]) as usize;
        let sub_len_at = 4 + rel + 8 + 4;
        body[sub_len_at..sub_len_at + 4].copy_from_slice(&0u32.to_le_bytes());
        assert!(matches!(
            parse_chunk(&h, &body, 0),
            Err(SsqError::MalformedChunk { .. })
        ));
    }
}
