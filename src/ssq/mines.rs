//! MINE_DATA chunk parser and writer (spec `docs/ssq_mine_chunk_format.md`).
//!
//! This module owns everything specific to SSQ chunk kind 20
//! (`MINE_DATA`): the 12-byte header validation, the 8-byte per-entry
//! layout, the per-entry classification rules (valid mine, recovered
//! shock, skipped), and the per-chart chunk emission.
//!
//! It deliberately does **not** own:
//!
//! - Dispatcher routing (the `20 =>` arm in [`crate::ssq::dispatch_chunk`]).
//! - Cross-chunk attachment (matching a parsed chunk's `param2` to a
//!   step chunk's chart at finalize time).
//! - The common-model [`NoteKind::Mine`] variant itself.
//!
//! Those concerns live in `ssq/mod.rs` and `model/mod.rs` respectively.
//! This module is purely "decode one chunk's bytes" and "encode one
//! chart's mines".
//!
//! # Parse direction
//!
//! [`parse_chunk`] takes the 12-byte [`ChunkHeader`] plus the chunk
//! body and validates the header up front:
//!
//! - `param3 × 8 + 12 == length` (body size matches declared entry count)
//! - `param2 != 0` (rejects pre-update-spec "all-charts" chunks)
//! - `param2` decodes to one of the 10 valid `(slot, style)` codes
//!   from `ssq_format.md §5.1`
//!
//! A header-level failure returns `None` and logs a `warn!`. On
//! success, each 8-byte entry runs through [`classify_entry`] and
//! produces one of three outcomes per spec §3.2 and §4:
//!
//! | Entry shape | Outcome |
//! |---|---|
//! | `panels` is a valid single- or multi-panel mask | `Mine`       |
//! | `panels` is one of the shock masks `0xFF/0x0F/0xF0` | `RecoveredShock` |
//! | anything else illegal | `Skipped`     |
//!
//! Recovered shocks and valid mines both flow into the returned
//! `Vec<Note>`; skipped entries do not, but each emits a `warn!`
//! naming the entry's byte offset and the rule that fired.
//!
//! # Write direction
//!
//! [`write_chunk`] takes a `&Chart`, collects the chart's
//! `NoteKind::Mine` notes, groups them by beat-tick and ORs their
//! panel masks within each beat (spec §4.1, design Decision 5),
//! sorts ascending by `(beat, panels)`, and emits one
//! `type=20, param2=difficulty_code(…), param3=N, length=12+8N`
//! chunk. A chart with zero mines writes zero bytes (design
//! Decision 3b — don't emit empty chunks).
//!
//! The writer refuses to serialize a `Mine` note whose panels equal
//! one of the shock masks (design Decision 8): this would poison the
//! DLL's runtime classifier. The result is a typed
//! [`SsqError::InvalidMinePanels`], returned — not a panic — so the
//! batch runner's per-file error recovery catches it.

use std::collections::BTreeMap;
use std::io::Write;

use crate::model::{Beat, Chart, Note, NoteKind, PanelSet, ShockSide, Style};
use crate::util::io::LeReader;

use super::chunk::ChunkHeader;
use super::writer::difficulty_code;
use super::SsqError;

/// Size of one MINE_DATA entry in bytes (spec §3.1).
const ENTRY_SIZE: usize = 8;

/// MINE_DATA chunk kind (spec §1).
const MINE_CHUNK_TYPE: u16 = 20;

/// Per-entry classification outcome (design Decision 4).
///
/// - `Mine`: well-formed per-panel mine, possibly multi-bit.
/// - `RecoveredShock`: `panels` was `0xFF`/`0x0F`/`0xF0` (a vanilla
///   shock-arrow encoding bled into the mine chunk). Converted to a
///   `NoteKind::Shock` note so the step-chunk writer handles it on
///   DDR → DDR; a `warn!` names the recovery.
/// - `Skipped`: any other illegal shape (zero panels, negative beat,
///   non-zero `flags`/`reserved`, or Single-mode high-nibble bits).
///   Emits a `warn!` and drops the entry.
enum EntryOutcome {
    Mine(Note),
    RecoveredShock(Note),
    Skipped,
}

/// Parse one MINE_DATA chunk into its `param2` difficulty code and a
/// list of notes (mines and recovered shocks).
///
/// Returns `None` when the chunk header fails validation
/// (length mismatch, `param2 == 0`, or `param2` is not one of the 10
/// valid difficulty codes from `ssq_format.md §5.1`). The failure is
/// logged at `warn!` level — the caller (the SSQ dispatcher) should
/// treat `None` as "drop this chunk and continue parsing the file".
///
/// On success, every entry is classified per spec §3.2; valid mines
/// and recovered shocks appear in the returned `Vec<Note>` in file
/// order. Skipped entries do not — each logs a `warn!` naming its
/// byte offset within the chunk and the rule that fired.
///
/// `chunk_offset` is the chunk's absolute byte offset in the file,
/// used purely for warn/error reporting.
pub fn parse_chunk(
    header: &ChunkHeader,
    body: &[u8],
    chunk_offset: usize,
) -> Option<(u16, Vec<Note>)> {
    debug_assert_eq!(
        header.ty, MINE_CHUNK_TYPE,
        "caller dispatched wrong chunk type"
    );

    // Header validation: length must equal 12 + 8·N.
    let expected_length = ChunkHeader::HEADER_SIZE + (ENTRY_SIZE as u32) * u32::from(header.param3);
    if header.length != expected_length {
        log::warn!(
            "{}",
            SsqError::MineChunkLengthMismatch {
                offset: chunk_offset,
                declared: header.length,
                param3: header.param3,
                expected: expected_length,
            }
        );
        return None;
    }

    // Header validation: param2 is the difficulty code of the paired
    // step chunk. `0` is the pre-update-spec "all charts" value — no
    // real files exist with that value, so treat it as orphan/bogus.
    if header.param2 == 0 {
        log::warn!(
            "orphan mine chunk at byte {chunk_offset}: param2=0 (pre-update-spec / not paired to any chart); skipping"
        );
        return None;
    }

    // Header validation: param2 must decode to one of the 10 valid
    // `(slot, style)` codes from `ssq_format.md §5.1`.
    let style = match valid_difficulty_style(header.param2) {
        Some(s) => s,
        None => {
            log::warn!(
                "mine chunk at byte {chunk_offset}: param2=0x{:04X} is not a valid difficulty code; skipping entire chunk",
                header.param2
            );
            return None;
        }
    };

    // Per-entry classification.
    let mut notes: Vec<Note> = Vec::with_capacity(usize::from(header.param3));
    for entry_idx in 0..usize::from(header.param3) {
        let entry_start = entry_idx * ENTRY_SIZE;
        let entry_offset = chunk_offset + (ChunkHeader::HEADER_SIZE as usize) + entry_start;
        let entry_bytes = &body[entry_start..entry_start + ENTRY_SIZE];
        match classify_entry(entry_bytes, style, entry_offset) {
            EntryOutcome::Mine(note) | EntryOutcome::RecoveredShock(note) => notes.push(note),
            EntryOutcome::Skipped => {}
        }
    }

    Some((header.param2, notes))
}

/// Write one MINE_DATA chunk for `chart`. Emits nothing (no chunk
/// header, no body) if the chart has no `NoteKind::Mine` notes.
///
/// Per design Decisions 3 and 5:
/// - `param2` is derived via [`difficulty_code`] from the chart's
///   style and difficulty — the same helper the step-chunk writer
///   uses, so each mine chunk is guaranteed to pair with its step
///   chunk by `param2` equality.
/// - Notes at the same beat-tick are grouped and their panel masks
///   ORed together, producing one output entry per unique
///   `beat_count`. The [`BTreeMap`] keying provides ascending-beat
///   order; ties on `beat_count` are broken on `panels` ascending
///   per spec §4.1.
/// - The writer refuses `Mine` notes whose panels equal one of the
///   vanilla shock masks (`0x0F`, `0xFF`, `0xF0`) — design
///   Decision 8. This is an invariant that the SSC parser (Task 4)
///   and the SSQ parser's shock recovery (Decision 4) both uphold;
///   a violation here is a programmer bug, surfaced as a typed
///   [`SsqError::InvalidMinePanels`] rather than a panic.
pub fn write_chunk(chart: &Chart, out: &mut impl Write) -> Result<(), SsqError> {
    // Group this chart's Mine notes by beat-tick, ORing panel masks
    // together per unique beat. BTreeMap preserves ascending key
    // order so spec §4.1's sort-ascending-by-beat invariant holds by
    // construction.
    let mut by_beat: BTreeMap<i32, u8> = BTreeMap::new();
    for note in &chart.notes {
        if !matches!(note.kind, NoteKind::Mine) {
            continue;
        }
        let panels = note.panels.bits();
        reject_shock_mask(panels)?;
        let tick = beat_to_measure_ticks_i32(note.beat)?;
        if tick < 0 {
            return Err(SsqError::Write(format!(
                "mine note at tick {tick}: spec §4.5 requires non-negative beat_count"
            )));
        }
        *by_beat.entry(tick).or_insert(0) |= panels;
    }

    if by_beat.is_empty() {
        return Ok(());
    }

    // BTreeMap iteration order is already ascending by key; merging
    // via OR on same-beat entries means no two entries share a
    // beat-tick, so the spec §4.1 tie-break on panels is vacuous.
    // The invariant check runs again on the merged result (defensive
    // — an OR of two valid masks can, in theory, produce a shock
    // mask: e.g. 0x0F = 0x01 | 0x02 | 0x04 | 0x08).
    let entries: Vec<(i32, u8)> = by_beat
        .into_iter()
        .map(|(tick, panels)| {
            reject_shock_mask(panels)?;
            Ok((tick, panels))
        })
        .collect::<Result<Vec<_>, SsqError>>()?;

    let n = entries.len();
    let body_len = n * ENTRY_SIZE;
    let chunk_length = ChunkHeader::HEADER_SIZE + body_len as u32;
    let param2 = difficulty_code(chart.style, chart.difficulty);

    // Header: length (u32), type (u16), param2 (u16), param3 (u16), param4 (u16).
    out.write_all(&chunk_length.to_le_bytes()).map_err(io_err)?;
    out.write_all(&MINE_CHUNK_TYPE.to_le_bytes())
        .map_err(io_err)?;
    out.write_all(&param2.to_le_bytes()).map_err(io_err)?;
    out.write_all(&(n as u16).to_le_bytes()).map_err(io_err)?;
    out.write_all(&0u16.to_le_bytes()).map_err(io_err)?; // param4

    // Body: N × (i32 beat_count, u8 panels, u8 flags=0, u16 reserved=0).
    for (tick, panels) in &entries {
        out.write_all(&(*tick as u32).to_le_bytes())
            .map_err(io_err)?;
        out.write_all(&[*panels]).map_err(io_err)?;
        out.write_all(&[0u8]).map_err(io_err)?; // flags
        out.write_all(&0u16.to_le_bytes()).map_err(io_err)?; // reserved
    }

    Ok(())
}

/// Classify one 8-byte entry per spec §3.2 and §4.5.
///
/// The check order matters: shock-mask checks run **before** the
/// Single-mode high-nibble check so that `panels == 0x0F` on a Single
/// chunk is recovered to a `BothSides` shock (per US-2) rather than
/// being rejected for having high-nibble bits set.
///
/// Entries are always 8 bytes — [`parse_chunk`] slices them out of
/// the body before calling this.
fn classify_entry(entry: &[u8], style: Style, entry_offset: usize) -> EntryOutcome {
    debug_assert_eq!(entry.len(), ENTRY_SIZE);

    let mut reader = LeReader::new(entry);
    let beat_count = match reader.read_u32() {
        Ok(v) => v as i32,
        Err(_) => {
            // Unreachable given the debug_assert, but don't panic.
            log::warn!("mine entry at byte {entry_offset}: malformed beat_count field; skipping");
            return EntryOutcome::Skipped;
        }
    };
    let panels = entry[4];
    let flags = entry[5];
    let reserved = u16::from_le_bytes([entry[6], entry[7]]);

    // Spec §4.5: negative beat_count is illegal in v1.
    if beat_count < 0 {
        log::warn!(
            "mine entry at byte {entry_offset}: beat_count {beat_count} is negative (spec §4.5); skipping"
        );
        return EntryOutcome::Skipped;
    }

    // Spec §3.3: flags reserved for v1, must be 0.
    if flags != 0 {
        log::warn!(
            "mine entry at byte {entry_offset}: flags=0x{flags:02X} is non-zero (spec §3.3 v1); skipping"
        );
        return EntryOutcome::Skipped;
    }

    // Spec §3.3: reserved field must be 0.
    if reserved != 0 {
        log::warn!(
            "mine entry at byte {entry_offset}: reserved=0x{reserved:04X} is non-zero (spec §3.3); skipping"
        );
        return EntryOutcome::Skipped;
    }

    // Spec §3.2: panels == 0x00 is a no-op mine.
    if panels == 0x00 {
        log::warn!(
            "mine entry at byte {entry_offset}: panels=0x00 is a no-op (spec §3.2); skipping"
        );
        return EntryOutcome::Skipped;
    }

    // Spec §3.2: shock-mask values (0xFF/0x0F/0xF0) bled into a mine
    // chunk. US-2 says recover these to shocks rather than reject the
    // whole file. Checked before the Single high-nibble rule so that
    // 0x0F on Single recovers to a BothSides shock.
    match panels {
        0xFF => {
            let note = recovered_shock_note(beat_count, style, ShockSide::BothSides, entry_offset);
            log::warn!(
                "mine entry at byte {entry_offset}: panels=0xFF is a shock-mask (spec §3.2); recovering as BothSides shock"
            );
            return match note {
                Some(n) => EntryOutcome::RecoveredShock(n),
                None => EntryOutcome::Skipped,
            };
        }
        0x0F => {
            // On Single the whole pad is P1-labeled bits, so the
            // classification is "BothSides" (covers the entire Single
            // pad). On Double it means only the P1 side.
            let side = match style {
                Style::Single => ShockSide::BothSides,
                Style::Double => ShockSide::P1Only,
            };
            let note = recovered_shock_note(beat_count, style, side, entry_offset);
            log::warn!(
                "mine entry at byte {entry_offset}: panels=0x0F is a shock-mask (spec §3.2); recovering as {side:?} shock"
            );
            return match note {
                Some(n) => EntryOutcome::RecoveredShock(n),
                None => EntryOutcome::Skipped,
            };
        }
        0xF0 => {
            // P2-only shock is only meaningful on Double charts. On
            // Single, a P2-side mask indicates the author confused
            // modes — fall through to the Single-high-nibble check
            // below, which will skip it with a more specific warning.
            if let Style::Double = style {
                let note = recovered_shock_note(beat_count, style, ShockSide::P2Only, entry_offset);
                log::warn!(
                    "mine entry at byte {entry_offset}: panels=0xF0 is a shock-mask (spec §3.2); recovering as P2Only shock"
                );
                return match note {
                    Some(n) => EntryOutcome::RecoveredShock(n),
                    None => EntryOutcome::Skipped,
                };
            }
        }
        _ => {}
    }

    // Spec §3.2: Single-mode chunks forbid high-nibble bits. Catches
    // cases like 0x11 (P1 Left + P2 Left on a Single chart).
    if matches!(style, Style::Single) && (panels & 0xF0) != 0 {
        log::warn!(
            "mine entry at byte {entry_offset}: Single-mode chunk with panels=0x{panels:02X} has high-nibble bits set (spec §3.2); skipping"
        );
        return EntryOutcome::Skipped;
    }

    // Valid single- or multi-panel mine.
    let beat = match Beat::from_measure_ticks(i64::from(beat_count)) {
        Ok(b) => b,
        Err(e) => {
            log::warn!(
                "mine entry at byte {entry_offset}: cannot convert beat_count {beat_count} to Beat ({e}); skipping"
            );
            return EntryOutcome::Skipped;
        }
    };
    EntryOutcome::Mine(Note {
        beat,
        kind: NoteKind::Mine,
        panels: PanelSet::from_bits(style, panels),
    })
}

/// Build a recovered `Shock` note. Returns `None` only if the
/// beat-to-tick conversion fails (unreachable in practice given
/// `beat_count >= 0` and spec §3.4's tick space).
fn recovered_shock_note(
    beat_count: i32,
    style: Style,
    side: ShockSide,
    entry_offset: usize,
) -> Option<Note> {
    let beat = match Beat::from_measure_ticks(i64::from(beat_count)) {
        Ok(b) => b,
        Err(e) => {
            log::warn!(
                "mine entry at byte {entry_offset}: recovered-shock beat conversion failed ({e}); skipping"
            );
            return None;
        }
    };
    let panels = match side {
        ShockSide::BothSides => PanelSet::from_bits(style, 0xFF),
        ShockSide::P1Only => PanelSet::from_bits(style, 0x0F),
        ShockSide::P2Only => PanelSet::from_bits(style, 0xF0),
    };
    Some(Note {
        beat,
        kind: NoteKind::Shock { side },
        panels,
    })
}

/// Returns the chart style implied by a MINE_DATA chunk's `param2`
/// difficulty code, or `None` if the code is not one of the 10 valid
/// `(slot, style)` combinations from `ssq_format.md §5.1`.
///
/// This is a mines-specific validator: the parser does not need the
/// `Difficulty` discriminator (that comes from the chart it attaches
/// to at finalize time), only the `Style` — which determines whether
/// high-nibble panel bits are legal on subsequent entries.
fn valid_difficulty_style(code: u16) -> Option<Style> {
    let style = match code & 0x00FF {
        0x14 => Style::Single,
        0x18 => Style::Double,
        _ => return None,
    };
    match (code & 0xFF00) >> 8 {
        0x01 | 0x02 | 0x03 | 0x04 | 0x06 => Some(style),
        _ => None,
    }
}

/// Reject panel masks that collide with the step-chunk shock
/// encodings (spec §3.2, design Decision 8).
fn reject_shock_mask(panels: u8) -> Result<(), SsqError> {
    match panels {
        0x0F | 0xFF | 0xF0 => Err(SsqError::InvalidMinePanels { panels }),
        _ => Ok(()),
    }
}

/// Convert a [`Beat`] to an integer i32 measure-tick count. Matches
/// the rounding behavior of `ssq/writer.rs::beat_to_measure_ticks`.
fn beat_to_measure_ticks_i32(beat: Beat) -> Result<i32, SsqError> {
    let r = beat.as_rational();
    let num = (r.num() as i128).checked_mul(1024).ok_or_else(|| {
        SsqError::Write("overflow converting mine beat to measure ticks".to_string())
    })?;
    let den = r.den() as i128;
    let half = if num >= 0 { den / 2 } else { -(den / 2) };
    let rounded = (num + half) / den;
    i32::try_from(rounded)
        .map_err(|_| SsqError::Write("mine measure-tick out of i32 range".to_string()))
}

fn io_err(err: std::io::Error) -> SsqError {
    SsqError::Write(err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Difficulty;

    // ---------- fixture builders (synthetic bytes per Learning 5) ----------

    /// Build a MINE_DATA chunk header with the given `param2` and
    /// `param3`. `length` is computed from `param3`.
    fn mine_header(param2: u16, param3: u16) -> ChunkHeader {
        ChunkHeader {
            length: ChunkHeader::HEADER_SIZE + (ENTRY_SIZE as u32) * u32::from(param3),
            ty: MINE_CHUNK_TYPE,
            param2,
            param3,
            param4: 0,
        }
    }

    /// Serialize one 8-byte mine entry.
    fn entry_bytes(beat_count: i32, panels: u8, flags: u8, reserved: u16) -> [u8; 8] {
        let mut out = [0u8; 8];
        out[0..4].copy_from_slice(&(beat_count as u32).to_le_bytes());
        out[4] = panels;
        out[5] = flags;
        out[6..8].copy_from_slice(&reserved.to_le_bytes());
        out
    }

    /// Build a chunk body from a list of well-formed entries.
    fn body_from_entries(entries: &[(i32, u8, u8, u16)]) -> Vec<u8> {
        let mut body = Vec::with_capacity(entries.len() * ENTRY_SIZE);
        for (beat, panels, flags, reserved) in entries {
            body.extend_from_slice(&entry_bytes(*beat, *panels, *flags, *reserved));
        }
        body
    }

    /// Build a chart with the given style/difficulty and a list of
    /// `NoteKind::Mine` notes at the specified `(beat_tick, panels)`.
    fn mine_chart(style: Style, difficulty: Difficulty, mines: &[(i32, u8)]) -> Chart {
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

    // ---------- parse_chunk: header validation (spec §9) ----------

    #[test]
    fn parse_chunk_valid_single_basic_header_parses() {
        // param2 = 0x0114 (Single Basic), one valid mine entry.
        let header = mine_header(0x0114, 1);
        let body = body_from_entries(&[(2048, 0x04, 0, 0)]);
        let (param2, notes) = parse_chunk(&header, &body, 100).unwrap();
        assert_eq!(param2, 0x0114);
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].kind, NoteKind::Mine);
        assert_eq!(notes[0].panels.bits(), 0x04);
        assert_eq!(notes[0].beat, Beat::from_measure_ticks(2048).unwrap());
    }

    #[test]
    fn parse_chunk_param2_zero_is_orphan_and_skipped() {
        // Pre-update-spec "all charts" value — skip with warn.
        let header = mine_header(0, 1);
        let body = body_from_entries(&[(0, 0x01, 0, 0)]);
        assert!(parse_chunk(&header, &body, 100).is_none());
    }

    #[test]
    fn parse_chunk_invalid_slot_0x05_is_skipped() {
        // 0x0514: style=0x14 (Single), slot=0x05 (not a valid slot).
        let header = mine_header(0x0514, 1);
        let body = body_from_entries(&[(0, 0x01, 0, 0)]);
        assert!(parse_chunk(&header, &body, 200).is_none());
    }

    #[test]
    fn parse_chunk_invalid_style_byte_is_skipped() {
        // 0x01AB: slot=0x01 (valid), style=0xAB (garbage).
        let header = mine_header(0x01AB, 1);
        let body = body_from_entries(&[(0, 0x01, 0, 0)]);
        assert!(parse_chunk(&header, &body, 300).is_none());
    }

    #[test]
    fn parse_chunk_length_mismatch_is_skipped() {
        // Declare param3=2 but give the header a length that says 1 entry.
        let mut header = mine_header(0x0114, 2);
        header.length = ChunkHeader::HEADER_SIZE + ENTRY_SIZE as u32; // room for 1 entry
        let body = body_from_entries(&[(0, 0x01, 0, 0), (1024, 0x02, 0, 0)]);
        assert!(parse_chunk(&header, &body, 400).is_none());
    }

    // ---------- parse_chunk: per-entry classification (spec §3.2, §4.5) ----------

    #[test]
    fn parse_chunk_entry_panels_zero_is_skipped() {
        let header = mine_header(0x0114, 1);
        let body = body_from_entries(&[(0, 0x00, 0, 0)]);
        let (_, notes) = parse_chunk(&header, &body, 500).unwrap();
        assert!(notes.is_empty(), "0x00 panels should be skipped");
    }

    #[test]
    fn parse_chunk_entry_shock_mask_0xff_on_single_recovers_bothsides() {
        let header = mine_header(0x0114, 1);
        let body = body_from_entries(&[(2048, 0xFF, 0, 0)]);
        let (_, notes) = parse_chunk(&header, &body, 600).unwrap();
        assert_eq!(notes.len(), 1);
        match &notes[0].kind {
            NoteKind::Shock {
                side: ShockSide::BothSides,
            } => {}
            other => panic!("expected BothSides Shock, got {other:?}"),
        }
    }

    #[test]
    fn parse_chunk_entry_shock_mask_0xff_on_double_recovers_bothsides() {
        let header = mine_header(0x0118, 1);
        let body = body_from_entries(&[(2048, 0xFF, 0, 0)]);
        let (_, notes) = parse_chunk(&header, &body, 700).unwrap();
        assert_eq!(notes.len(), 1);
        match &notes[0].kind {
            NoteKind::Shock {
                side: ShockSide::BothSides,
            } => {}
            other => panic!("expected BothSides Shock, got {other:?}"),
        }
    }

    #[test]
    fn parse_chunk_entry_shock_mask_0x0f_on_single_recovers_bothsides() {
        // On Single, 0x0F covers the whole pad → BothSides (per task spec).
        let header = mine_header(0x0114, 1);
        let body = body_from_entries(&[(2048, 0x0F, 0, 0)]);
        let (_, notes) = parse_chunk(&header, &body, 800).unwrap();
        assert_eq!(notes.len(), 1);
        match &notes[0].kind {
            NoteKind::Shock {
                side: ShockSide::BothSides,
            } => {}
            other => panic!("expected BothSides Shock on Single, got {other:?}"),
        }
    }

    #[test]
    fn parse_chunk_entry_shock_mask_0x0f_on_double_recovers_p1only() {
        let header = mine_header(0x0118, 1);
        let body = body_from_entries(&[(2048, 0x0F, 0, 0)]);
        let (_, notes) = parse_chunk(&header, &body, 900).unwrap();
        assert_eq!(notes.len(), 1);
        match &notes[0].kind {
            NoteKind::Shock {
                side: ShockSide::P1Only,
            } => {}
            other => panic!("expected P1Only Shock on Double, got {other:?}"),
        }
    }

    #[test]
    fn parse_chunk_entry_shock_mask_0xf0_on_double_recovers_p2only() {
        let header = mine_header(0x0118, 1);
        let body = body_from_entries(&[(2048, 0xF0, 0, 0)]);
        let (_, notes) = parse_chunk(&header, &body, 1000).unwrap();
        assert_eq!(notes.len(), 1);
        match &notes[0].kind {
            NoteKind::Shock {
                side: ShockSide::P2Only,
            } => {}
            other => panic!("expected P2Only Shock on Double, got {other:?}"),
        }
    }

    #[test]
    fn parse_chunk_entry_single_with_high_nibble_bit_is_skipped() {
        // 0x11 on Single: P1 Left (0x01) plus a stray high-nibble bit.
        let header = mine_header(0x0114, 1);
        let body = body_from_entries(&[(0, 0x11, 0, 0)]);
        let (_, notes) = parse_chunk(&header, &body, 1100).unwrap();
        assert!(notes.is_empty(), "Single with high-nibble bit must skip");
    }

    #[test]
    fn parse_chunk_entry_negative_beat_count_is_skipped() {
        let header = mine_header(0x0114, 1);
        let body = body_from_entries(&[(-1, 0x01, 0, 0)]);
        let (_, notes) = parse_chunk(&header, &body, 1200).unwrap();
        assert!(notes.is_empty(), "negative beat_count must skip");
    }

    #[test]
    fn parse_chunk_entry_nonzero_flags_is_skipped() {
        let header = mine_header(0x0114, 1);
        let body = body_from_entries(&[(0, 0x01, 0x01, 0)]);
        let (_, notes) = parse_chunk(&header, &body, 1300).unwrap();
        assert!(notes.is_empty(), "non-zero flags must skip in v1");
    }

    #[test]
    fn parse_chunk_entry_nonzero_reserved_is_skipped() {
        let header = mine_header(0x0114, 1);
        let body = body_from_entries(&[(0, 0x01, 0, 0x0001)]);
        let (_, notes) = parse_chunk(&header, &body, 1400).unwrap();
        assert!(notes.is_empty(), "non-zero reserved must skip in v1");
    }

    #[test]
    fn parse_chunk_multi_bit_panels_on_single_produces_mine() {
        // 0x09 = L + R on Single (bit 0 + bit 3). Valid multi-panel mine.
        let header = mine_header(0x0114, 1);
        let body = body_from_entries(&[(4096, 0x09, 0, 0)]);
        let (_, notes) = parse_chunk(&header, &body, 1500).unwrap();
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].kind, NoteKind::Mine);
        assert_eq!(notes[0].panels.bits(), 0x09);
    }

    // ---------- write_chunk: round-trip shape ----------

    #[test]
    fn write_chunk_empty_chart_emits_nothing() {
        let chart = mine_chart(Style::Single, Difficulty::Basic, &[]);
        let mut out = Vec::new();
        write_chunk(&chart, &mut out).unwrap();
        assert!(out.is_empty(), "a chart with no mines must emit no bytes");
    }

    #[test]
    fn write_chunk_single_mine_emits_20_byte_chunk() {
        // One mine on Single Basic at beat 2048 (= 2 beats) on P1 Down.
        let chart = mine_chart(Style::Single, Difficulty::Basic, &[(2048, 0x02)]);
        let mut out = Vec::new();
        write_chunk(&chart, &mut out).unwrap();

        // 12 header + 8 entry = 20 bytes.
        assert_eq!(out.len(), 20);
        // length u32
        assert_eq!(u32::from_le_bytes(out[0..4].try_into().unwrap()), 20);
        // type u16
        assert_eq!(u16::from_le_bytes(out[4..6].try_into().unwrap()), 20);
        // param2 u16 = difficulty_code(Single, Basic) = 0x0114
        assert_eq!(u16::from_le_bytes(out[6..8].try_into().unwrap()), 0x0114);
        // param3 u16 = entry count
        assert_eq!(u16::from_le_bytes(out[8..10].try_into().unwrap()), 1);
        // param4 u16 = 0
        assert_eq!(u16::from_le_bytes(out[10..12].try_into().unwrap()), 0);
        // entry: beat_count (i32), panels (u8), flags (u8), reserved (u16)
        assert_eq!(i32::from_le_bytes(out[12..16].try_into().unwrap()), 2048);
        assert_eq!(out[16], 0x02);
        assert_eq!(out[17], 0); // flags
        assert_eq!(u16::from_le_bytes(out[18..20].try_into().unwrap()), 0);
    }

    #[test]
    fn write_chunk_same_beat_different_panels_or_merges() {
        // Two mines at the same beat on adjacent panels → one entry
        // with the ORed mask (spec §4.1, design Decision 5).
        let chart = mine_chart(
            Style::Single,
            Difficulty::Basic,
            &[(1024, 0x01), (1024, 0x02)],
        );
        let mut out = Vec::new();
        write_chunk(&chart, &mut out).unwrap();

        // Expect one entry only.
        assert_eq!(u16::from_le_bytes(out[8..10].try_into().unwrap()), 1);
        assert_eq!(out.len(), 20);
        assert_eq!(out[16], 0x03, "ORed panels 0x01 | 0x02");
    }

    #[test]
    fn write_chunk_reverse_beat_order_emits_sorted_ascending() {
        // Authored in descending order; writer must emit ascending
        // per spec §4.1.
        let chart = mine_chart(
            Style::Single,
            Difficulty::Basic,
            &[(4096, 0x04), (1024, 0x02)],
        );
        let mut out = Vec::new();
        write_chunk(&chart, &mut out).unwrap();

        // Two entries: 12 header + 16 body = 28 bytes.
        assert_eq!(out.len(), 28);
        assert_eq!(u16::from_le_bytes(out[8..10].try_into().unwrap()), 2);
        // First entry's beat_count should be 1024.
        assert_eq!(i32::from_le_bytes(out[12..16].try_into().unwrap()), 1024);
        // Second entry's beat_count should be 4096.
        assert_eq!(i32::from_le_bytes(out[20..24].try_into().unwrap()), 4096);
    }

    #[test]
    fn write_chunk_duplicate_same_beat_same_panel_dedups() {
        // Two identical mines at the same beat — OR is idempotent,
        // one output entry.
        let chart = mine_chart(
            Style::Single,
            Difficulty::Basic,
            &[(1024, 0x04), (1024, 0x04)],
        );
        let mut out = Vec::new();
        write_chunk(&chart, &mut out).unwrap();
        assert_eq!(u16::from_le_bytes(out[8..10].try_into().unwrap()), 1);
        assert_eq!(out[16], 0x04);
    }

    #[test]
    fn write_chunk_double_expert_uses_0x0318_param2() {
        let chart = mine_chart(Style::Double, Difficulty::Expert, &[(0, 0x11)]);
        let mut out = Vec::new();
        write_chunk(&chart, &mut out).unwrap();
        assert_eq!(u16::from_le_bytes(out[6..8].try_into().unwrap()), 0x0318);
    }

    // ---------- write_chunk: invariant violations (design Decision 8) ----------

    #[test]
    fn write_chunk_rejects_0x0f_panels() {
        // Construct a Mine whose panels == 0x0F — a programmer bug
        // that the SSC parser (Task 4) and the shock-recovery path
        // both prevent. The writer must refuse, not panic.
        let chart = Chart {
            style: Style::Single,
            difficulty: Difficulty::Basic,
            notes: vec![Note {
                beat: Beat::from_measure_ticks(1024).unwrap(),
                kind: NoteKind::Mine,
                panels: PanelSet::from_bits(Style::Single, 0x0F),
            }],
        };
        let mut out = Vec::new();
        let err = write_chunk(&chart, &mut out).unwrap_err();
        assert!(matches!(err, SsqError::InvalidMinePanels { panels: 0x0F }));
    }

    #[test]
    fn write_chunk_rejects_0xff_panels_on_double() {
        let chart = Chart {
            style: Style::Double,
            difficulty: Difficulty::Expert,
            notes: vec![Note {
                beat: Beat::from_measure_ticks(0).unwrap(),
                kind: NoteKind::Mine,
                panels: PanelSet::from_bits(Style::Double, 0xFF),
            }],
        };
        let mut out = Vec::new();
        let err = write_chunk(&chart, &mut out).unwrap_err();
        assert!(matches!(err, SsqError::InvalidMinePanels { panels: 0xFF }));
    }

    #[test]
    fn write_chunk_rejects_0xf0_panels_on_double() {
        let chart = Chart {
            style: Style::Double,
            difficulty: Difficulty::Expert,
            notes: vec![Note {
                beat: Beat::from_measure_ticks(0).unwrap(),
                kind: NoteKind::Mine,
                panels: PanelSet::from_bits(Style::Double, 0xF0),
            }],
        };
        let mut out = Vec::new();
        let err = write_chunk(&chart, &mut out).unwrap_err();
        assert!(matches!(err, SsqError::InvalidMinePanels { panels: 0xF0 }));
    }

    // ---------- round-trip: write → parse ----------

    #[test]
    fn round_trip_mixed_single_and_multi_panel_mines_preserves_notes() {
        // Build a chart with mines on varied beats + panel shapes.
        // Write → then parse back → expect the entries to match
        // (modulo the writer's sort-ascending-by-beat normalization).
        let chart = mine_chart(
            Style::Single,
            Difficulty::Challenge,
            &[
                (0, 0x01),    // single mine, beat 0, L
                (1024, 0x09), // multi-bit, beat 1, L+R
                (3072, 0x04), // single mine, beat 3, U
                (2048, 0x02), // out-of-order insertion → writer normalizes
            ],
        );
        let mut bytes = Vec::new();
        write_chunk(&chart, &mut bytes).unwrap();

        // Re-parse the bytes we just wrote.
        // Strip the 12-byte header to get the body.
        let header = ChunkHeader {
            length: u32::from_le_bytes(bytes[0..4].try_into().unwrap()),
            ty: u16::from_le_bytes(bytes[4..6].try_into().unwrap()),
            param2: u16::from_le_bytes(bytes[6..8].try_into().unwrap()),
            param3: u16::from_le_bytes(bytes[8..10].try_into().unwrap()),
            param4: u16::from_le_bytes(bytes[10..12].try_into().unwrap()),
        };
        let body = &bytes[12..];
        let (param2, notes) = parse_chunk(&header, body, 0).unwrap();

        // param2 should be Single Challenge = 0x0614.
        assert_eq!(param2, 0x0614);
        // All four input mines preserved; re-parsed in ascending-beat order.
        assert_eq!(notes.len(), 4);
        let ticks: Vec<i32> = notes
            .iter()
            .map(|n| {
                let r = n.beat.as_rational();
                ((r.num() * 1024) / r.den() as i64) as i32
            })
            .collect();
        assert_eq!(ticks, vec![0, 1024, 2048, 3072]);
        // Panel masks match the sorted input.
        let masks: Vec<u8> = notes.iter().map(|n| n.panels.bits()).collect();
        assert_eq!(masks, vec![0x01, 0x09, 0x02, 0x04]);
        for n in &notes {
            assert_eq!(n.kind, NoteKind::Mine);
        }
    }
}
