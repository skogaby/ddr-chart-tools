# Design: 20260429-ddr-mines-support

**Requirements**: [requirements.md](requirements.md)
**Parent SIM**: none (hobby project)

---

## Overview

Adds a new `MINE_DATA` chunk (kind 20) to the SSQ reader and writer, plus a new `NoteKind::Mine { .. }` variant to the common model, so per-panel ITG-style mines flow SM5↔DDR end-to-end. The `docs/ssq_mine_chunk_format.md` v1 spec is authoritative; implementation follows its byte layout, validation rules, and co-location rules verbatim. The existing full-row-`M`-as-shock classification in the SSC parser is preserved so DDR shock-arrow round-trips stay lossless.

**Per-difficulty keying**: the v1 spec (§2.1) uses `param2` as the difficulty discriminator (same `(slot, style)` encoding as step chunks). Each chart with mines gets its own MINE_DATA chunk keyed by its step chunk's `param2`. Mines flow through `Song.charts[i].notes` alongside taps/holds/shocks — no sidecar needed on `SsqParseResult` (mines have no hidden fields to round-trip, and per-difficulty scoping means no cross-chart aggregation is ever needed).

---

## Architecture Decisions

### Decision 1: Represent mines as a new `NoteKind` variant, not a sidecar

**Problem**: Where do per-panel mines live in the model? Options are (a) a new `NoteKind::Mine` variant carried in `Chart.notes`, or (b) a parallel `Chart.mines: Vec<Mine>` field, or (c) a sidecar vector on `SsqParseResult` like `events` and `raw_tempo_pairs`.

**Decision**: (a) — new `NoteKind::Mine` variant that carries panel info via the existing `Note.panels: PanelSet`, mirroring `NoteKind::Tap`.

**Rationale**:
- Mines participate in the same "sorted by beat, one event per tick" note stream taps and shocks already live in. Putting them in `Chart.notes` means the SSC writer's existing grid-emission code — which walks a chart's notes once and writes `1/2/3/M` characters per panel — picks up mines naturally. A parallel `Chart.mines` field would duplicate that walk.
- The `Song` model is the cross-format lingua franca; mines are real model-level concepts (SSC speaks them directly), not an SSQ-only sidecar like raw tempo pairs or ignored event bytes. Learning 7 says format-specific data → sidecar; cross-format data → `Song`. Mines are the latter.
- Mines have no hidden bits that need to round-trip losslessly. The 8-byte entry carries `(beat, panels, flags=0, reserved=0)`; the writer regenerates all four from the model. No sidecar needed.

**Alternatives Considered**:
- (b) Parallel `Chart.mines`: doubles grid walks in SSC writer; splits tap/mine co-location logic across two collections; does not solve any real problem that (a) leaves unsolved.
- (c) Sidecar on `SsqParseResult`: wrong layer — would force DDR→SM5 to thread a raw chunk through the SSC writer, defeating the "everything goes through the common model" rule in `tech.md`.

**Tradeoffs**: Expanding `NoteKind` means every `match` on it grows a new arm. Rust's exhaustiveness check catches these, and the existing `match` sites are few (SSC writer grid emit, SSQ writer step/mine split, runtime note filtering in `synthesize_events::last_tick`). Worth it for model uniformity.

### Decision 2: Dual-output split — shocks in step chunks, mines in `MINE_DATA`

**Problem**: After SSC parsing produces a mix of `Shock` and `Mine` notes on one chart, how does the SSQ writer know which go in the step chunk (as byte `0xFF / 0x0F / 0xF0`) and which go in the mine chunk (as 8-byte entries)?

**Decision**: Split by `NoteKind` at the top of `write_steps_chunk` / a new `write_mines_chunk`. `Shock` stays in step chunk, `Mine` goes to mine chunk. The split is done once, per chart, via a partition pass before the existing step-chunk emitter runs.

**Rationale**:
- Shocks and mines are semantically distinct post-feature; conflating them at write time would require re-inferring shock-vs-mine from panel masks — brittle and easy to get wrong.
- The existing `emit_steps_and_freezes` already classifies `NoteKind::Shock` into the three byte values. Adding a `NoteKind::Mine => skip, collect for mine chunk` arm is a one-line change.
- Mine chunks are scoped per chart (Decision 3 — `param2` = difficulty code), so each chart's mines are written in a separate chunk immediately after the step-chunk loop finishes. This matches the layout required by `docs/ssq_mine_chunk_format.md` §1 ("After all step chunks").

**Alternatives Considered**:
- **Classify at write time by panel mask**: impossible — a `Mine` with `panels = 0x0F` on Double mode is a legal half-row mine, but `0x0F` in a step byte is a P1-only shock. Ambiguous without `NoteKind`.
- **Make `Mine` carry its own `PanelSet` payload distinct from `Note.panels`**: no benefit; complicates the shared-field invariant between `Tap`/`HoldHead`/`Mine`.

**Tradeoffs**: Two passes over each chart's notes (one for step bytes, one for mine entries), but each is O(N) and charts have ≤ a few thousand notes. Negligible.

### Decision 3: One MINE_DATA chunk per difficulty, keyed by `param2` difficulty code

**Problem**: A file has multiple charts (difficulties). Mines can differ per difficulty. How is each mine entry associated with a specific chart on read, and how is each chart's mines serialized on write?

**Decision**: Follow spec §2.1 exactly — each MINE_DATA chunk's `param2` carries the same difficulty code (`0x0114`, `0x0318`, etc.) as the paired step chunk. On read, match MINE_DATA chunks to step chunks by `param2` equality. On write, emit one MINE_DATA chunk per chart that has at least one `Mine` note, using `difficulty_code(chart.style, chart.difficulty)` for `param2` (the helper already exists in `ssq/writer.rs`).

**Rationale**:
- The spec is authoritative and explicit; deviating would break the DLL mod's lookup.
- Per-difficulty scoping matches how step chunks work — every DDR reader is already comfortable with `(type=3, param2=<code>)` lookups, and mines extend the same pattern.
- Makes per-difficulty mine design (e.g. mines only on Challenge) expressible end-to-end, which was not possible under the old "broadcast to all charts" model.
- The writer can reuse `difficulty_code(style, difficulty)` — no new encoding logic.

**Alternatives Considered**:
- ~~Broadcast to every chart (the original Decision 3 before the spec update)~~: rejected because it made per-difficulty mines inexpressible — users couldn't author different mines for Easy vs Hard.
- **One file-wide MINE_DATA chunk with `param2 = 0`**: matches the pre-update spec, but no longer aligned with what the DLL mod looks for. Would produce unreadable output post-update.
- **Embed mines inside the step chunk** (e.g. reserve a step-byte value for mines): extends the step chunk's vocabulary beyond the vanilla set `{0x00, 0x0F, 0xF0, 0xFF, step-bits}`, which would confuse the vanilla-compat story. Rejected.

**Tradeoffs**: Slightly more I/O overhead on write (one chunk header per mine-bearing difficulty, ~12 bytes each). Negligible — a song with all 10 difficulties mine-bearing is ~120 bytes of chunk-header overhead on a file that's already tens of kilobytes. Handles orphan chunks (MINE_DATA with a `param2` that matches no step chunk) as warn+skip: harmless and aligned with spec §2.1.

### Decision 3b: Orphan MINE_DATA chunks and invalid `param2` values

**Problem**: What if an input SSQ has a MINE_DATA chunk whose `param2` does not match any step chunk in the file (orphan)? What if `param2 == 0` (the pre-update spec's value)? What if `param2` isn't a valid difficulty code (`0x0514`, `0xABCD`, etc.)?

**Decision**:
- **Orphan** (`param2` is a valid difficulty code but no matching step chunk exists): skip the entire chunk with `warn!("orphan mine chunk ... no matching step chunk")`.
- **`param2 == 0`**: skip with `warn!` (catches pre-update-spec files).
- **`param2` decodes to an invalid difficulty code** (slot 0x05, unknown style byte, etc.): skip with `warn!`.
- **Duplicate** (two MINE_DATA chunks with the same `param2`): accept the first, skip subsequent with `warn!` (matches DLL "stops at first match" behavior).

**Rationale**: All four are classified as malformed-but-recoverable. Skipping with a warn preserves the rest of the file's data, follows spec §9's "refuse to crash" mandate, and surfaces the issue to the user via the warn log.

**Alternatives Considered**:
- **Hard-fail on any of these**: would abort a batch run for a single malformed file; user wanted per-file error recovery (initial-deliverable requirements).
- **Treat `param2 == 0` as "apply to every chart" for backwards compat**: tempting, but no real files exist with pre-update-spec MINE_DATA (this tool is the only one that would emit them, and it hasn't shipped mine support yet). Carrying legacy behavior for a non-existent corpus isn't worth it.

**Tradeoffs**: None. Graceful degradation is the correct read posture for a hobby tool.

### Decision 4: Recover, don't reject, malformed `panels` bytes on parse

**Problem**: Spec §3.2 lists illegal `panels` values: `0x00`, `0xFF`, `0x0F`, `0xF0` (the shock encodings), and Single-mode charts with high nibble set. Requirements US-2 says recover `0xFF/0x0F/0xF0` to a `Shock` with a warn, skip everything else with a warn.

**Decision**: Implement the recovery rules from US-2 as a small validation function in the new `ssq/mines` module that returns a classified outcome: `Valid(Mine)`, `RecoveredShock(Shock)`, or `Skipped(reason)`. The parser calls it per entry, attaches `Valid`/`RecoveredShock` to the matching chart, and logs warns on `RecoveredShock`/`Skipped`.

**Rationale**:
- A single enum makes the rule set auditable — each arm corresponds to one line in US-2's acceptance list. Spread across ad-hoc `if` branches, the coverage is hard to verify.
- `RecoveredShock` reuses the existing `NoteKind::Shock` and flows through the existing step-chunk writer on DDR→DDR — no new write path for "recovered shocks." The recovery is invisible downstream.
- The spec explicitly calls this a malformed-input scenario (writers must never produce these values), so the log is WARN, not ERROR.

**Alternatives Considered**:
- **Reject the file outright on any invalid entry**: too strict; spec says "log WARN and continue." Users with a bad chart shouldn't lose the whole song.
- **Silent drop**: loses fidelity data. User wanted observability in US-8.

**Tradeoffs**: The `RecoveredShock` path means a malformed input SSQ can produce a round-trip that differs from the input (mine recovered to shock, written as a step byte), but this is a feature — the user explicitly wanted the recovery in Q5 of discovery.

**Note**: Chunk-header validation (the `param2` difficulty-code check, duplicate detection, length mismatch) is separate from per-entry validation and is handled before `parse_body` runs. See Decision 3b for the rules.

### Decision 5: Per-chart dedup and sort on SSQ write

**Problem**: When writing MINE_DATA for a chart, how are multiple model `Mine` notes on the same `(beat, style)` merged? The SSC parser emits one `Mine` per `M` character, so one per row-per-panel — the writer needs to combine bit-by-bit same-beat entries into multi-bit entries (per spec §4.1).

**Decision**: Per-chart, pre-write pass: group a chart's `Mine` notes by `beat_tick`, OR their `panels` masks together, then emit one entry per unique beat. Sort ascending by `beat` then by `panels`. **No cross-chart aggregation** — per-difficulty scoping (Decision 3) means each chart's chunk is independent.

**Rationale**:
- Matches the spec's mandatory sort (`§4.1`): ascending by `beat_count`, ties broken on `panels` ascending.
- Handles the "multi-bit entry" case without a special SSC-side pass — the SSC parser can emit one `Mine` per `M` character, one entry per bit, and the writer merges them within a chart via `panels_OR`.
- Keeps the SSC parser dumb — it doesn't need to know about the writer's merging rule or per-difficulty scoping.

**Alternatives Considered**:
- **Merge at SSC parse time**: couples the SSC parser to the SSQ output format (bad layering).
- **Emit one mine entry per panel bit (no merging)**: wastes entries, violates spec's preference for multi-bit packing (§3.2: "Multiple bits may be set in one entry — this is a single mine occupying multiple panels at the same beat").

**Tradeoffs**: Writer does slightly more work than a pure pass-through, but the work is O(mines in one chart). Negligible.

### Decision 6: `NoteKind::Mine` carries no payload; panel info lives in `Note.panels`

**Problem**: Should `Mine` be `NoteKind::Mine` (unit-like, matching `Tap`) or `NoteKind::Mine { panels: PanelSet }` (carrying its own payload)?

**Decision**: Unit-like `NoteKind::Mine`. Panel info lives in the enclosing `Note.panels`, same as `Tap`. User explicitly confirmed in Q3 of discovery.

**Rationale**:
- Consistent with `Tap`: both are "instantaneous hit/miss on these panels at this beat."
- `Note.panels` is already authoritative across all variants; duplicating it in the variant payload invites drift and confuses consumers.
- `HoldHead` carries `length` because length is unique to holds; mines have no such extra field. `Shock` carries `side` because side is distinct from a panel set (it's a semantic label, not a bit mask). Mines carry neither.

**Alternatives**: (see problem statement)

**Tradeoffs**: Future mine semantics (e.g. `flags` bit for "cosmetic mine") would need a new payload. Spec §3.3 reserves flags for v2+; this feature explicitly leaves them at 0. If v2 happens, either add a payload then or add a second variant `NoteKind::CosmeticMine`. Don't pre-abstract.

### Decision 7: Remove `SscError::UnsupportedMine`; keep full-row shock classification

**Problem**: The current SSC parser rejects any `M`-bearing row that doesn't form a full-row shock. Requirements US-3 says partial mines must pass through as per-panel mines, but full-row shocks must still classify as shocks for DDR round-trip fidelity.

**Decision**: Rewrite `collect_mine_bits` in `ssc/notes.rs` to return a 3-way classification — `FullRowShock(ShockSide)`, `PerPanelMines(u8)`, or `None` — instead of a 2-way `Option<ShockSide>` plus `Err(UnsupportedMine)`. The `UnsupportedMine` variant is removed from `SscError`.

Full-row shock classification (the existing `classify_mine_row` logic) stays verbatim. The new code path: if `mine_bits` forms a full-row shock, emit one `Shock` note; otherwise emit one `Mine` note whose `panels` is the mine bits themselves (which may coexist on the same row as a `1`/`2`-derived `Tap` note). The `non_mine_nonzero` rejection is dropped — mixed `M`+`1` rows now parse as two distinct notes at the same beat.

**Rationale**:
- Matches US-3 acceptance criteria exactly.
- Removing an error variant that no caller needs to branch on is a net simplification.
- The existing `classify_mine_row` logic (Single mask `0x0F`, Double masks `0xFF`/`0x0F`/`0xF0`) is already correct; preserves DDR→SM5→DDR shock fidelity without new code.

**Alternatives Considered**:
- **Keep `UnsupportedMine` behind a CLI flag for strict mode**: US-7 says no new flags.
- **Always emit per-panel mines, drop full-row shock special-case**: regresses shock round-trip for DDR→SM5→DDR (a `Shock` read from an SSQ step byte would write `MMMM` in SSC, parse back as 4 mines, write back to SSQ as 4 `MINE_DATA` entries — losing the shock classification entirely).

**Tradeoffs**: "Four independent mines at one beat covering all 4 Single panels" becomes unexpressible in SSC (it always classifies as a shock). Documented in requirements Open Question #2; acceptable.

### Decision 8: Writer invariant — `Mine` notes can never carry shock-mask panels

**Problem**: What happens if someone hand-builds a `Song` where a `Mine` note's `panels` is `0x0F`, `0xFF`, or `0xF0`? Spec §3.2 says the writer must never emit those values in MINE_DATA.

**Decision**: Treat it as a writer-side invariant violation — assert, don't silently recover. The SSC parser never produces such a `Mine` (full-row shocks are intercepted upstream in Decision 7). The DDR parser only produces them via the `RecoveredShock` path (Decision 4), which returns `Shock`, not `Mine`. Any `Mine` that arrives at the writer with one of those masks is a programmer bug, not a data bug.

**Rationale**:
- Keeps the writer simple — no re-classification logic at emit time.
- Loud failure (debug assertion or error) surfaces the bug immediately in tests; silent recovery hides it.
- The invariant is enforceable via a unit test: construct a `Mine` with `panels = 0x0F`, write, assert `SsqError::Write` fires.

**Alternatives Considered**:
- **Silently demote to a step-byte shock**: moves the Decision-7 classification into the writer; duplicates logic that belongs in the SSC parser.
- **Silently split into per-panel mines (e.g. `0x0F` → four 0x01/0x02/0x04/0x08 entries)**: violates the spec's intent (those panel masks are reserved because they'd confuse the DLL's shock classifier at render time; hand-crafted splits don't help).

**Tradeoffs**: None — this is the correct strict-write posture.

---

## Component Design

### New/Modified Components

| Component | Layer/Location | Responsibility | Replaces/Extends |
|-----------|----------------|----------------|------------------|
| `NoteKind::Mine` | `src/model/mod.rs` | New variant, unit-like, paired with `Note.panels` | Extends `NoteKind` |
| `src/ssq/mines.rs` | new | Parse + write one MINE_DATA chunk per difficulty; chunk-header validation (param2 difficulty code, duplicate/orphan detection); per-entry validation/classification | New module |
| `SsqError::MineChunkLengthMismatch` (or similar) | `src/ssq/mod.rs` | Typed error for `param3 × 8 + 12 != length` | New variant |
| `ssq/mod.rs::dispatch_chunk` | `src/ssq/mod.rs` | New `20 => ...` arm collecting MINE_DATA chunks into `PartialSong.pending_mine_chunks` keyed by `param2` | Extends existing dispatch |
| `ssq/mod.rs::PartialSong` | `src/ssq/mod.rs` | New `pending_mine_chunks: HashMap<u16, Vec<Note>>` (or `Vec<(u16, Vec<Note>)>` for order-preservation), drained into charts by matching `param2` at finalize | Extends `PartialSong` |
| `ssq/writer.rs::write` | `src/ssq/writer.rs` | New per-chart post-step-chunk call: for each chart, if it has any `Mine` notes, call `mines::write_chunk(chart, out)` with the chart's difficulty code as `param2` | Extends existing writer |
| `ssq/writer.rs::emit_steps_and_freezes` | `src/ssq/writer.rs` | New `NoteKind::Mine => continue` arm (mines don't go in step chunk) | Extends existing emitter |
| `ssc/notes.rs::collect_mine_bits` | `src/ssc/notes.rs` | Replaced with 3-way classifier returning `MineRowKind::FullRowShock` / `PerPanelMines` / `None` | Rewritten |
| `ssc/notes.rs::decode_row` | `src/ssc/notes.rs` | New `'M'` branch emits `NoteKind::Mine` when not classified as shock | Modified |
| `ssc/notes.rs::place_note_events` | `src/ssc/notes.rs` | New `NoteKind::Mine => emit 'M' per active panel` arm | Modified |
| `SscError::UnsupportedMine` | `src/ssc/mod.rs` | Removed | Deleted |
| `src/job/mod.rs::synthesize_events::last_tick` | `src/job/mod.rs` | Verify `Mine` notes flow through the existing `_ => n.beat.as_rational()` fallthrough (no logic change expected; add a focused regression test) | Verified, no logic change |

### Component Interactions

```
            ┌─ SSC parser ──────────────────────────────┐
            │                                            │
SSC bytes ──┤  decode_row (per #NOTEDATA section)        │
            │    ├─ collect_mine_bits (3-way classifier) │
            │    │    ├─ FullRowShock → Note{Shock}      │
            │    │    ├─ PerPanelMines → Note{Mine}      │
            │    │    └─ None (no M) → Note{Tap/Hold}    │
            │    └─ mixed rows: Note{Tap} + Note{Mine}   │
            │       (both at same beat)                  │
            └────────────────────────────────────────────┘
                                │
                                ▼
                    Song.charts[i].notes
                    (sorted by beat, mixed kinds;
                     per-difficulty scoped)
                                │
                                ▼
┌──────────── SSQ writer ────────────────────────────────┐
│                                                         │
│  write(song, events, raw_tempo_pairs, out)              │
│    ├─ write_tempo_chunk                                 │
│    ├─ write_events_chunk                                │
│    ├─ for chart in charts:                              │
│    │     write_steps_chunk                              │
│    │       └─ emit_steps_and_freezes                    │
│    │            ├─ Tap → bits in step byte              │
│    │            ├─ HoldHead → bits + later 0x00 + entry │
│    │            ├─ Shock → byte 0xFF/0x0F/0xF0          │
│    │            └─ Mine → skip (handled below)          │
│    ├─ for chart in charts:   ◄── NEW                    │
│    │     if chart has Mine notes:                       │
│    │        mines::write_chunk(chart, out)              │
│    │          ├─ param2 = difficulty_code(style, diff)  │
│    │          ├─ collect this chart's Mine notes only   │
│    │          ├─ group by beat, OR panels within chart  │
│    │          ├─ sort ascending beat, tie on panels     │
│    │          ├─ assert panels ∉ {0x0F, 0xFF, 0xF0}     │
│    │          └─ emit kind-20 chunk                     │
│    └─ write terminator                                  │
└─────────────────────────────────────────────────────────┘

┌──────────── SSQ parser ────────────────────────────────┐
│                                                         │
│  dispatch_chunk                                         │
│    ├─ 1 → tempo                                         │
│    ├─ 2 → events                                        │
│    ├─ 3 → steps → PartialSong.charts.push              │
│    ├─ 4|5|9|17 → aux dropped                           │
│    ├─ 20 → mines::parse_chunk   ◄── NEW                │
│    │       ├─ validate chunk header:                    │
│    │       │    ├─ param3·8+12 != length → warn+skip    │
│    │       │    ├─ param2 == 0 → warn+skip (orphan)     │
│    │       │    ├─ param2 not valid diff code→warn+skip │
│    │       │    └─ (duplicate check happens at finalize)│
│    │       ├─ per-entry classify:                       │
│    │       │    ├─ Valid(Mine) → notes for this chunk  │
│    │       │    ├─ RecoveredShock(Shock) → notes       │
│    │       │    └─ Skipped → warn only                  │
│    │       └─ PartialSong.pending_mine_chunks.push      │
│    │           ((param2, notes_for_this_chunk))         │
│    └─ other → SsqError::UnexpectedChunkType             │
│                                                         │
│  finalize                                               │
│    ├─ detect duplicates in pending_mine_chunks:         │
│    │     for each (param2, notes), if a chart exists    │
│    │     with the same difficulty code AND that chart   │
│    │     has already received a chunk's notes, warn+skip│
│    ├─ attach each chunk's notes to the matching chart   │
│    │     (by chart.style/difficulty → diff code ==      │
│    │     chunk's param2)                                │
│    ├─ orphan chunks (no matching chart) → warn+drop     │
│    └─ re-sort each affected chart's notes by beat       │
└─────────────────────────────────────────────────────────┘
```

### Removed Components

| Component | Reason |
|-----------|--------|
| `SscError::UnsupportedMine` | Partial / mixed mine rows are now valid input (Decision 7); no code path returns this error anymore |
| Rejection branch in `decode_row` for `non_mine_nonzero` mixed rows | Same reason — mixed `M`+`1` rows are valid |

---

## Integration Points

**External Services**: N/A — this is a CLI tool with no network I/O.

**Data Storage**: File I/O only. No DB.

**Configuration**: No new config; no new CLI flags (US-7).

**Cross-module dependencies added**:
- `ssc/notes.rs` → `model::NoteKind::Mine` (new variant reference).
- `ssq/mines.rs` → `model::{Beat, Chart, NoteKind, PanelSet, ShockSide, Style, Note}` (re-uses existing types); imports `difficulty_code` from `ssq/writer.rs` (or lift it to a shared helper).
- `ssq/mod.rs` → `ssq::mines` (new module in the `ssq/` tree).
- `ssq/writer.rs` → `ssq::mines::write_chunk` for per-chart chunk emission.

---

## Public Contracts (Signatures Only — NO Implementations)

```rust
// src/model/mod.rs — extend the existing NoteKind enum:
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum NoteKind {
    Tap,
    HoldHead { length: Beat },
    Shock { side: ShockSide },
    Mine,  // NEW: per-panel mine; panels come from Note.panels
}

// src/ssq/mines.rs — new module public API:

/// One decoded mine-chunk outcome per 8-byte entry. Consumed by the
/// SSQ parser to decide how to attach the entry to the chart list.
enum EntryOutcome {
    Mine(Note),               // well-formed per-panel mine (possibly multi-bit panels)
    RecoveredShock(Note),     // panels was 0xFF/0x0F/0xF0 — warn + convert
    Skipped,                  // any other illegal shape — warn + drop
}

/// Parse one kind-20 chunk into a list of notes and report its
/// associated `param2` difficulty code to the caller.
/// Returns `Some((param2, notes))` if the chunk's header passes
/// validation (param2 is a known difficulty code, length matches
/// param3·8+12); returns `None` otherwise, after having logged a
/// `warn!` describing the skip reason.
/// `chart_style_for_validation` is derived from the chunk's own
/// `param2` low byte, not from any separately-tracked chart — the
/// parser doesn't need to know which chart this chunk pairs with
/// to classify entries.
pub fn parse_chunk(header: &ChunkHeader, body: &[u8], chunk_offset: usize)
    -> Option<(u16, Vec<Note>)>;

/// Write one MINE_DATA chunk for `chart` into `out`. Called once per
/// chart by the caller (the SSQ writer's outer loop). Emits nothing
/// if the chart has no `Mine` notes. Uses `difficulty_code(chart.
/// style, chart.difficulty)` for `param2`.
pub fn write_chunk(chart: &Chart, out: &mut impl Write) -> Result<(), SsqError>;

// src/ssc/notes.rs — new internal classifier:
enum MineRowKind {
    FullRowShock(ShockSide),   // emit a Shock note, drop further M processing
    PerPanelMines(u8),          // emit one Mine note carrying this panel mask
    NoMines,                    // row contains no 'M'
}

fn classify_mine_row(row: &str, style: Style) -> MineRowKind;
```

---

## Changes to Existing Code

### `src/model/mod.rs`
- **Change**: Add `Mine` arm to `NoteKind`. Update the module's unit tests to cover the new variant (ordering, equality).
- **Reason**: Core representation of the new note type.
- **Impact**: Every existing `match` on `NoteKind` grows an arm. Compiler catches all sites.

### `src/ssq/mod.rs`
- **Change**: Add `20 => mines::parse_chunk(...)` arm to `dispatch_chunk`. Add `pending_mine_chunks: Vec<(u16, Vec<Note>)>` to `PartialSong` — each entry is `(param2_difficulty_code, parsed_notes)`. In `finalize`, after building `charts`: walk `pending_mine_chunks` in insertion order, and for each `(param2, notes)`:
  - If no chart has a step chunk matching `param2` → orphan; log `warn!` and discard.
  - If a chart matches AND that chart has not yet received a mine chunk's notes → attach notes (cloning each `Note` so the chart owns them), apply `Note.panels` masking via `PanelSet::from_bits(chart.style, ...)` to strip invalid high-nibble bits on Single charts, then re-sort the chart's notes by beat.
  - If a chart matches but already has notes from a previous chunk → duplicate `(type=20, param2=X)`; log `warn!` and discard (matches spec §2.2 "stops at first match").
- **Reason**: Routes the new chunk; implements Decisions 3 (per-difficulty keying) and 3b (orphan/duplicate handling).
- **Impact**: DDR→anything paths now surface mines on the model, correctly scoped per difficulty. DDR→DDR round-trip exercises the new write path.

### `src/ssq/writer.rs`
- **Change**: (1) Add `NoteKind::Mine => continue` arm in `emit_steps_and_freezes`. (2) After the step-chunk loop and before the terminator write, iterate `song.charts` again and, for each chart that has any `Mine` notes, call `mines::write_chunk(chart, out)`. The mines module computes `param2 = difficulty_code(chart.style, chart.difficulty)` internally (reusing the existing helper). (3) Minor: ensure `max_chart_beat` includes Mine notes (the existing `_ => Some(n.beat)` match arm already does this, but verify with a test).
- **Reason**: Implements Decisions 2 (dual-output split), 3 (per-difficulty chunk emission), and 5 (per-chart dedup/sort).
- **Impact**: Output SSQ byte order is unchanged for mine-free charts (the second iteration emits nothing when no chart has mines). Mine-bearing output gains one new chunk per mine-bearing difficulty, all clustered after the last step chunk and before the terminator.

### `src/ssc/notes.rs`
- **Change**: Replace `collect_mine_bits` with a 3-way classifier returning `MineRowKind`. Update `decode_row` to act on each arm. Remove the `unreachable!` M-branch in the panel-scan loop; instead, if the row classified as `PerPanelMines(mask)`, push one `Note { kind: Mine, panels: PanelSet::from_bits(style, mask) }` and still scan the rest of the row for taps/holds on non-mine panels. Update `place_note_events` to add a `NoteKind::Mine => emit 'M' per active panel` arm.
- **Reason**: Implements Decision 7 (drop UnsupportedMine) and closes the loop on US-3 and US-4.
- **Impact**: SSC inputs that previously errored now parse. SSC outputs emit `M` characters for `Mine` notes at correct rows/panels.

### `src/ssc/mod.rs`
- **Change**: Remove `SscError::UnsupportedMine` variant.
- **Reason**: No caller remains.
- **Impact**: Any external code that matched on this variant would break — but this is an internal error enum only used inside the crate, and the crate's `Error` type wraps it via `thiserror`. Safe.

### `src/job/mod.rs`
- **Change**: Verify `synthesize_events`'s `last_tick` calculation naturally picks up `Mine` notes. The existing filter falls through `_ => Some(n.beat.as_rational())` which covers `Mine`. Add a focused test that confirms this — a chart whose only "last" note is a mine must still get a properly-bracketed FINISH.
- **Reason**: Learning 11 invariant — FINISH must be bracketed by TIMING entries including beats introduced by mines.
- **Impact**: No production code change expected; this is an audit to prevent a regression in the FINISH-bracketing guard.

### `src/ssq/steps.rs`
- **Change**: None. The step-chunk parser never sees a kind-20 chunk.
- **Reason**: Mines are a separate chunk; step-chunk bytes are still the classic `{0x00, 0x0F, 0xF0, 0xFF, bits}` set.
- **Impact**: N/A.

---

## Deployment Sequence

This is a hobby tool distributed as a `cargo build --release` binary; there is no pipeline to sequence. Implementation order within the feature:

1. Model change (`NoteKind::Mine`) and all resulting compile errors fixed (model tests, SSC grid emit, SSQ step emit). **Compile green, tests not yet covering mines.**
2. New `src/ssq/mines.rs` module — `parse_chunk` + `write_chunk` + unit tests against synthetic bytes (covering per-difficulty `param2`, orphan chunks, duplicates).
3. SSQ parser integration (`dispatch_chunk` arm, `PartialSong.pending_mine_chunks` field, `finalize` attach-by-difficulty logic).
4. SSQ writer integration (per-chart `mines::write_chunk` call after the step-chunk loop; Mine arm in `emit_steps_and_freezes`).
5. SSC parser rework (`classify_mine_row` → 3-way, `decode_row` rewrite, remove `UnsupportedMine`).
6. SSC writer Mine arm in `place_note_events`.
7. Integration tests: SM5→DDR with per-difficulty mines, DDR→SM5 with per-difficulty mines, orphan-chunk skip, DDR→DDR round-trip with per-difficulty mines, shock-regression, no-mine byte-identical baseline.
8. `synthesize_events` audit test (Mine at last-tick bracket check).

**Rollback**: This is a single feature in one working copy. The user will iterate on checkpoint commits (per Learning 2). If the feature must be abandoned, revert the commits.

---

## Risks and Mitigations

| Risk | Impact | Likelihood | Mitigation |
|------|--------|------------|------------|
| FINISH/END bracketing (Learning 11) breaks when a chart's last note is a Mine | H | L | Mine beats are included in `max_chart_beat` via the existing fallthrough match arm. Add a focused regression test in `job/mod.rs` tests to prove this. |
| `NoteKind::Mine` match exhaustiveness cascade introduces bugs in obscure places | M | M | Rust compiler catches every missing arm. Code review step: grep the crate for `match.*NoteKind` and ensure all sites compile; no `#[allow(non_exhaustive)]` suppressions. |
| SSC parser change accidentally breaks existing full-row shock round-trip | H | L | The `classify_mine_row` Single/Double full-row detection logic is preserved verbatim. A shock-regression integration test (US-5 acceptance) catches any break. |
| MINE_DATA chunk placement ordering accidentally violates vanilla-compat assumption (e.g. written before step chunks, or between two step chunks) | H | L | US-6 acceptance has a vanilla-chunk-walk assertion; tests verify every MINE_DATA chunk's byte offset is after the last step chunk's byte offset + length. |
| Wrong `param2` difficulty code on write (e.g. off-by-one in `difficulty_code`) silently scopes mines to the wrong chart | H | L | Writer reuses the existing `difficulty_code` helper already exercised by step-chunk writing. Test: round-trip an SSQ with distinct mine patterns on two difficulties and assert each decoded chunk's `param2` matches its chart. |
| Orphan or duplicate MINE_DATA chunk in input crashes the parser | M | L | `parse_chunk` returns `Option<(u16, Vec<Note>)>`; `None` triggers skip-with-warn. `finalize` walks in order and warn+drops duplicates. Unit test: synthetic SSQ with orphan+duplicate combinations exercises every path. |
| Writer's invariant assertion (Decision 8) fires unexpectedly on well-formed input and crashes a batch run | M | L | Choose `SsqError::Write` (returned, surfaces as a per-file batch error) rather than `debug_assert!` (aborts the process). The batch runner's per-file error-recovery already logs and continues. |
| Mine-chunk tests built from synthetic bytes drift away from the spec if the spec is updated | L | M | The spec file and test fixtures live in-repo and are reviewed together; tests reference spec section numbers in comments per the Rust CLI standards guide. |
| A v1 reader of this tool's output (the DLL mod) expects a valid difficulty code in `param2` and trips on an invalid one | H | L | Writer always derives `param2` from the chart's existing style/difficulty via `difficulty_code` (which can only produce the 10 valid codes — it panics on invalid inputs, which is fine because all charts in the model have valid style/difficulty by construction). |

---

## Open Questions

1. **Is the per-chart clone of notes (for mine attachment in `finalize`) acceptable memory-wise?** For a chart with 10,000 mines, that's ~80 KB per chart. Well within acceptable memory for a CLI tool. Not a scope concern.

2. **Should the writer emit an empty MINE_DATA chunk (`length=12, param3=0`) for any difficulty?** No — the writer always synthesizes from the model's current state. A difficulty with zero mines emits no chunk at all. Covered by US-1 acceptance.

3. **Should `mines::write_chunk` live on `ssq::mines` or become a method on `Chart`?** Module-level function is simpler; `Chart` doesn't know about SSQ serialization today, and adding that coupling would bleed SSQ concerns into the model. Keep it module-level. Tasks phase can revisit if the signature accretes more arguments.

4. **Spec cleanup required** — ~~`docs/ssq_mine_chunk_format.md` §8.3 byte-stream example and §9 validation checklist still contain stale `param2 == 0` references (leftover from the pre-per-difficulty draft). They contradict the authoritative §2.1. This feature follows §2.1; the user should clean up the stale bits in §8.3 and §9 before the feature ships. Out of scope for this feature's implementation tasks, but worth a doc-commit.~~ **Resolved** 2026-04-29: the user cleaned up §8.1, §8.3, §9, and §10 of the spec (via a handoff to the spec-authoring agent). The authoritative `param2`-is-the-difficulty-code convention is now consistent throughout the document. This feature's implementation follows it as-is.
