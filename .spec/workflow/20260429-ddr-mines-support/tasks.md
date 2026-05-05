# Tasks: 20260429-ddr-mines-support

Tasks are sized to be independently reviewable and buildable. Per project Learning 1, the sdd-software-developer chains tasks within a single session (approval check-ins between tasks, no hand-back to EM). Per project Learning 2, no git operations or CR flow are involved — the user handles checkpoint commits externally.

Per project Learning 5, all tests use synthetic in-code fixtures built from `docs/ssq_format.md` and `docs/ssq_mine_chunk_format.md` byte layouts. No real Konami/community assets.

## Workspace Info
**Primary Package**: ddr-chart-tools
**All Packages**: ddr-chart-tools (single-crate workspace)

---

## Task 1: Add `NoteKind::Mine` variant to the common model
**Package(s)**: ddr-chart-tools
**Goal**: `NoteKind::Mine` exists as a unit-like variant of `NoteKind`, every existing `match NoteKind { ... }` site compiles with an explicit arm, and existing tests stay green.
**Scope**:
- Extend `NoteKind` in `src/model/mod.rs` with a unit-like `Mine` variant (no payload; panel info rides on `Note.panels`, matching `Tap` — per design Decision 6).
- Add `Mine =>` arms to every existing `match` on `NoteKind` to satisfy exhaustiveness. No user-visible behavior change yet (no format emits or consumes `Mine` in this task):
  - SSQ writer `emit_steps_and_freezes` → skip (mines don't go in step chunks; step-chunk loop ignores them).
  - SSC writer `place_note_events` → skip (mines don't emit a character yet).
  - Any classifier/filter in `src/job/` or elsewhere → fall through the existing `_ => Some(n.beat.as_rational())` style default so `Mine` notes participate in beat-based bookkeeping (FINISH bracketing audit in Task 5 validates this is correct).
- Keep the SSC parser's `UnsupportedMine` behavior unchanged for now — this task does not unlock SSC mine input.
- Update `src/model/` unit tests to cover the new variant's `Debug`, `Clone`, `PartialEq`, `Eq`, `Hash` derives.

**Tests**:
- Existing `cargo test` suite stays green (zero regressions).
- New unit test in `src/model/mod.rs` asserting `NoteKind::Mine` participates in equality and hashing as a distinct variant.
- `cargo build` and `cargo clippy --all-targets -- -D warnings` succeed.

**Dependencies**: None.

- [x] 1.1 Extend `NoteKind` enum in `src/model/mod.rs` with `Mine` variant.
- [x] 1.2 Grep for `match .*NoteKind` across `src/` and add `NoteKind::Mine =>` arms to every site. Placeholder behavior (skip/no-op) is acceptable — document the intent in a short comment per site.
- [x] 1.3 Add unit test(s) in `src/model/` for equality/hashing/ordering of the new variant.
- [x] 1.4 Verify `cargo build`, `cargo test`, `cargo clippy --all-targets -- -D warnings` all pass.

---

## Task 2: Implement `src/ssq/mines.rs` module (parse + write + classify)
**Package(s)**: ddr-chart-tools
**Goal**: A self-contained `src/ssq/mines.rs` module that can parse one MINE_DATA chunk into `(param2: u16, notes: Vec<Note>)` and write one MINE_DATA chunk from a `&Chart`, with comprehensive unit tests covering every validation rule from design Decisions 3b, 4, 5, and 8. Not yet wired into the SSQ dispatcher or writer.

**Scope**:
- Create `src/ssq/mines.rs` per Learning 10 (`touch` + `git add` empty first, then write content). Expose:
  - `pub fn parse_chunk(header: &ChunkHeader, body: &[u8], chunk_offset: usize) -> Option<(u16, Vec<Note>)>` — validates header (`param2` is a valid difficulty code via the existing decoding helper; `param3 × 8 + 12 == length`), returns `None` with `warn!` on header-level failures per design §Decision 3b. Per-entry classification via internal `EntryOutcome` enum (design Decision 4): `Mine(Note)`, `RecoveredShock(Note)`, `Skipped` — each with a distinct `warn!` message naming the entry's byte offset and the rule that fired.
  - `pub fn write_chunk(chart: &Chart, out: &mut impl Write) -> Result<(), SsqError>` — collects the chart's `NoteKind::Mine` notes, groups by `beat_tick`, ORs `panels` masks within each beat (design Decision 5), sorts ascending by `(beat, panels)` per spec §4.1, asserts no resulting mask equals `0xFF`/`0x0F`/`0xF0` via a typed error (design Decision 8; `SsqError::Write` variant — do NOT use `debug_assert!`, per design risk mitigation), and emits `type=20`, `param2=difficulty_code(chart.style, chart.difficulty)`, `param3=N`, `param4=0`, `length=12+8N`. Emits nothing (no chunk) if the chart has no `Mine` notes.
- Add `SsqError::MineChunkLengthMismatch { offset, declared, expected }` (or similar) to `src/ssq/mod.rs` for the header-validation path. Add any new write-side error variant required for Decision 8 invariant violations.
- Reuse the existing `difficulty_code(style, difficulty)` helper. If it currently lives inside `writer.rs` as a non-`pub` function, lift it to `pub(super)` or move to a new shared `ssq/difficulty.rs` — design leaves the placement to tasks phase; pick the smaller change.
- Register the new module from `src/ssq/mod.rs` (`mod mines;`) so it compiles, but do NOT yet add the `20 => mines::parse_chunk(...)` arm to the dispatcher — that's Task 3.

**Tests** (all in `src/ssq/mines.rs` unit-test module, using synthetic bytes built inline per Learning 5):
- **`parse_chunk` — header validation**:
  - Valid `param2` = `0x0114` (Single Basic), valid entries → returns `Some((0x0114, notes))` with the expected note list.
  - `param2 == 0` → returns `None`, warns "orphan / pre-update-spec".
  - `param2` = `0x0514` (invalid slot 5) → returns `None`, warns invalid difficulty code.
  - `param2` = `0x01AB` (invalid style byte) → returns `None`, warns invalid difficulty code.
  - `param3 * 8 + 12 != length` → returns `None`, warns length mismatch with declared vs expected.
- **`parse_chunk` — per-entry classification**:
  - `panels = 0x00` → Skipped with warn.
  - `panels = 0xFF` on Single → RecoveredShock (`ShockSide::BothSides`), warns recovery.
  - `panels = 0x0F` on Double → RecoveredShock (`ShockSide::P1Only`), warns recovery.
  - `panels = 0xF0` on Double → RecoveredShock (`ShockSide::P2Only`), warns recovery.
  - `panels = 0x0F` on Single → RecoveredShock (`ShockSide::BothSides`), warns recovery. (Spec §3.2 says `0x0F` is a shock encoding regardless of mode; on Single the recovery still maps to `BothSides`.)
  - Single-mode chart with `panels & 0xF0 != 0` (e.g. `0x11`) → Skipped with warn.
  - `beat_count < 0` → Skipped with warn.
  - `flags != 0` → Skipped with warn.
  - `reserved != 0` → Skipped with warn.
  - Valid multi-bit panels (e.g. `0x09` = L+R on Single) → Mine with multi-bit `PanelSet`.
- **`write_chunk` — round-trip shape**:
  - Chart with zero `Mine` notes → emits zero bytes.
  - Chart with one single-panel `Mine` → emits expected 20-byte chunk (12 header + 8 entry) with correct `param2` for the chart's difficulty.
  - Chart with two `Mine` notes on same beat, different panels → one entry with ORed `panels` mask.
  - Chart with `Mine` notes in reverse beat order → output entries sorted ascending by beat.
  - Chart with two `Mine` notes at same beat same panel → one entry (dedup via OR idempotence).
- **`write_chunk` — invariant violation**:
  - Hand-crafted chart with a `Mine` note whose `panels` mask equals `0x0F` → `write_chunk` returns `Err(SsqError::Write(...))` (not a panic).
- **Round-trip**:
  - Build a chart with a mix of single- and multi-panel mines on varied beats → `write_chunk` produces bytes → `parse_chunk` on those bytes → attach notes to a fresh chart with the same style/difficulty → note list matches (after sort normalization per spec §4.1).

**Dependencies**: Task 1 complete (needs `NoteKind::Mine`).

- [x] 2.1 `touch src/ssq/mines.rs && git add src/ssq/mines.rs`, then register the module in `src/ssq/mod.rs`.
- [x] 2.2 Define `EntryOutcome`, `parse_chunk`, and the `SsqError::MineChunkLengthMismatch` (or chosen name) variant. Reuse or lift `difficulty_code` as needed.
- [x] 2.3 Define `write_chunk` including pre-write group-by-beat + OR-merge + sort + invariant assertion.
- [x] 2.4 Write the unit tests enumerated above. Each test's name references the rule or spec section it exercises.
- [x] 2.5 Verify `cargo build`, `cargo test --package ddr-chart-tools -- ssq::mines`, `cargo clippy --all-targets -- -D warnings` all pass.

---

## Task 3: Wire mines into the SSQ parser + writer (DDR↔DDR end-to-end)
**Package(s)**: ddr-chart-tools
**Goal**: DDR→DDR round-trip preserves per-difficulty mines byte-for-byte (modulo the writer's sort-order normalization). A no-mine SSQ produces byte-identical output to the pre-feature baseline. SSC→DDR and DDR→SSC paths still don't emit/consume mines yet — Tasks 4 and 5 will unlock those.

**Scope**:
- **Parser side (`src/ssq/mod.rs`)**:
  - Add `20 => mines::parse_chunk(header, body, chunk_offset)` arm to `dispatch_chunk`.
  - Extend `PartialSong` with `pending_mine_chunks: Vec<(u16, Vec<Note>)>` (order-preserving — needed to detect "second chunk with same param2" as duplicate).
  - When `parse_chunk` returns `Some((param2, notes))`, push into `pending_mine_chunks`.
  - In `finalize`, after charts are built but before returning:
    - Iterate `pending_mine_chunks` in insertion order. For each `(param2, notes)`:
      - Find the chart whose step chunk's `param2` equals this value.
      - If no match → log `warn!("orphan mine chunk param2=0x{:04X}, no matching step chunk")` and discard.
      - If match found AND that chart has not already received mine notes in this pass → merge notes into chart's `notes` vector (clone or move), apply `PanelSet::from_bits(chart.style, panels)` to sanitize Single-mode high-nibble bits (defensive — `parse_chunk` should have already rejected these per Task 2, but the mask is idempotent), then re-sort the chart's notes by beat.
      - If match found but chart already has mine notes (duplicate `(type=20, param2=X)`) → log `warn!("duplicate mine chunk param2=0x{:04X}, keeping first")` and discard.
- **Writer side (`src/ssq/writer.rs`)**:
  - Add `NoteKind::Mine => continue` arm to `emit_steps_and_freezes` (mines don't go in the step chunk).
  - After the existing `for chart in charts { write_steps_chunk(chart, out)?; }` loop and before the terminator write, add:
    ```rust
    for chart in &song.charts {
        mines::write_chunk(chart, out)?;  // emits nothing if chart has no mines
    }
    ```
  - Verify `max_chart_beat` in the tempo-chunk synthesis path (Learning 11 — FINISH bracketing) continues to include `Mine` note beats via the existing fallthrough match arm. Do not change the code unless a test demonstrates a regression; an explicit audit is Task 5's scope.
- **Integration tests** (`tests/` or in-module integration tests):
  - **Vanilla-baseline byte equality**: pick one existing no-mine fixture from `tests/sm_to_ddr/` or `tests/ddr_to_sm/` — run it through DDR→DDR and assert the output bytes equal a pre-feature expected byte sequence (or the input bytes for a true round-trip case). Guards US-6 acceptance.
  - **DDR→DDR with per-difficulty mines** (synthetic SSQ built in-code per Learning 5): two charts with different mine patterns, write → read → write, assert final bytes match the first write's bytes (entries sorted, `param2` matches each chart's difficulty).
  - **Orphan mine chunk on parse**: synthetic SSQ with a kind-20 chunk whose `param2` does not match any step chunk — parser logs `warn!` and produces a valid `Song` with no mine notes.
  - **Duplicate mine chunk on parse**: synthetic SSQ with two kind-20 chunks sharing the same `param2` — parser keeps first, warns on second; resulting chart has only first chunk's mines.

**Tests**:
- All existing `cargo test` green.
- New integration tests listed above pass.
- `cargo clippy --all-targets -- -D warnings` passes.

**Dependencies**: Tasks 1 and 2 complete.

- [x] 3.1 Add `20 =>` arm to `dispatch_chunk` in `src/ssq/mod.rs`; extend `PartialSong` with `pending_mine_chunks`.
- [x] 3.2 Implement the `finalize` attachment/orphan/duplicate logic.
- [x] 3.3 Add `NoteKind::Mine` skip arm + per-chart `mines::write_chunk` loop to `src/ssq/writer.rs`.
- [x] 3.4 Add the four integration tests listed above.
- [x] 3.5 Verify `cargo build`, `cargo test`, `cargo clippy --all-targets -- -D warnings` all pass.

---

## Task 4: Rework the SSC parser for per-panel mines (SM5→DDR end-to-end)
**Package(s)**: ddr-chart-tools
**Goal**: SSC/SM input with partial `M` rows parses cleanly into `NoteKind::Mine` notes. Full-row `M` rows continue to classify as `NoteKind::Shock` (US-3 preserved). SM5→DDR produces SSQ with a proper MINE_DATA chunk per difficulty. Existing shock-arrow round-trip tests still pass.

**Scope**:
- **SSC parser (`src/ssc/notes.rs`)**:
  - Add a `MineRowKind` enum: `FullRowShock(ShockSide)`, `PerPanelMines(u8)`, `NoMines`.
  - Replace `collect_mine_bits` (and whatever it returns today) with a `classify_mine_row(row: &str, style: Style) -> MineRowKind` function. The full-row classification logic from the current code (Single mask `0x0F`, Double masks `0xFF` / `0x0F` / `0xF0`) is preserved verbatim — only the return shape and the mixed-row rejection are new.
  - Update `decode_row` (or the current equivalent) so that:
    - `FullRowShock(side)` → emit one `Note { kind: Shock { side }, panels: PanelSet::full_row(style, side) }` (or current equivalent shock construction), and treat all `M`s as consumed.
    - `PerPanelMines(mask)` → emit one `Note { kind: Mine, panels: PanelSet::from_bits(style, mask) }`, AND continue scanning the row for `1`/`2`/`3`/`4` characters on non-mine panels so mixed rows produce multiple notes at the same beat.
    - `NoMines` → existing tap/hold-only scan path.
  - Drop the `non_mine_nonzero` rejection branch that currently returns `SscError::UnsupportedMine`.
- **SSC error enum (`src/ssc/mod.rs`)**:
  - Remove `SscError::UnsupportedMine`. Update any internal callers (none should remain after the parser rework).
- **Unit tests (in `src/ssc/notes.rs` test module)**:
  - `MMMM` on Single → `FullRowShock(BothSides)`.
  - `MMMMMMMM` on Double → `FullRowShock(BothSides)`.
  - `MMMM0000` on Double → `FullRowShock(P1Only)`.
  - `0000MMMM` on Double → `FullRowShock(P2Only)`.
  - `0M00` on Single → `PerPanelMines(0x02)` (Down-only mine).
  - `M10M` on Single → `PerPanelMines` with mask `0x09` + a separate `Tap` on Down (the `1`). Both notes at same beat, different panels.
  - `MM00` on Single → `PerPanelMines(0x03)` (partial, not a full row).
  - `M0000000` on Double → `PerPanelMines(0x01)` (P1 Left mine only).
  - No `M` characters → `NoMines`, existing logic applies.
- **Integration tests**:
  - Build an in-memory `Song` from a synthetic SSC string (per Learning 5) with per-panel mines across two difficulties → run SM5→DDR → assert the output SSQ contains two MINE_DATA chunks with the expected `param2` values and expected entries.
  - **Shock regression**: existing DDR→SM5→DDR shock round-trip test continues to pass. Build a synthetic SSQ with a step-byte shock, convert DDR→SM5, confirm SSC has the full-row `M` pattern, convert SM5→DDR, confirm step-byte shock is preserved (no MINE_DATA chunk emitted).
  - **Mixed-row parse**: SSC with a row `M1M1` — two `Tap`s on Down/Right, two `Mine`s on Left/Up, all at the same beat, roundtrip correctly.

**Tests**:
- All existing `cargo test` green (including the shock regression).
- New tests above pass.
- `cargo clippy --all-targets -- -D warnings` passes.

**Dependencies**: Tasks 1, 2, 3 complete (needs `NoteKind::Mine` and the MINE_DATA write path).

- [x] 4.1 Define `MineRowKind` and rewrite `classify_mine_row` in `src/ssc/notes.rs`.
- [x] 4.2 Update `decode_row` to consume the 3-way classification and emit Mine/Shock/Tap notes as appropriate.
- [x] 4.3 Remove `SscError::UnsupportedMine` and any dead callers.
- [x] 4.4 Add the unit tests enumerated above.
- [x] 4.5 Add the integration tests (SM5→DDR with mines; shock regression; mixed-row parse).
- [x] 4.6 Verify `cargo build`, `cargo test`, `cargo clippy --all-targets -- -D warnings` all pass.

---

## Task 5: SSC writer Mine arm + observability + final audits (DDR→SM5 and full round-trip)
**Package(s)**: ddr-chart-tools
**Goal**: DDR→SM5 emits `M` characters for MINE_DATA entries at correct panels and rows. DDR→SM5→DDR round-trip preserves per-panel mines. FINISH-bracketing invariant (Learning 11) is audited against mine-only last-beat cases. Every `warn!` path from requirements US-8 is exercised by at least one test. `--help` output is unchanged per US-7.

**Scope**:
- **SSC writer (`src/ssc/notes.rs` / `src/ssc/mod.rs`)**:
  - Add a `NoteKind::Mine =>` arm to `place_note_events` (or current equivalent) that emits `'M'` for each panel bit set in the note's `panels` onto the row at the note's beat. Co-exists with `Tap`/`HoldHead` emission on the same row at different panels — reuse the existing per-panel slot assignment logic.
  - Verify the quantizer (`ssc/notes.rs::pick_quantize` or equivalent) picks the same quantize for a mine-bearing row as for the same row without mines. Mines land on tap-aligned beats, so this should be a no-op — but add one targeted unit test that asserts it.
- **FINISH-bracketing audit (`src/job/mod.rs`)**:
  - Add a focused regression test: build an `events` + `charts` input where a chart's **last note is a `NoteKind::Mine`** and no later tap/hold exists. Run through `synthesize_events`. Assert the emitted tempo-pair / events sequence brackets FINISH correctly — the synthesized trailing tempo pair sits at `last_mine_beat + 2 measures`, FINISH at `last_mine_beat + 1 measure`, END at the last tempo-pair tick. Per Learning 11, this must not regress.
- **Integration tests**:
  - **DDR→SM5 with per-difficulty mines**: synthetic SSQ with two MINE_DATA chunks (two difficulties) → convert DDR→SM5 → assert the output SSC has `M` characters at the expected panels/rows in each `#NOTEDATA` block.
  - **DDR→SM5→DDR full round-trip**: synthetic SSQ with mines on multiple panels across multiple beats → DDR→SM5 → SM5→DDR → assert final SSQ's MINE_DATA chunks match the first write's chunks entry-for-entry (after sort normalization).
  - **SM5→DDR→SM5 full round-trip**: synthetic SSC with per-panel mines → SM5→DDR → DDR→SM5 → assert the SSC character grid is preserved row-by-row for mine positions.
- **Observability audit**:
  - For each `warn!` call site added across Tasks 2–5 (length mismatch, orphan param2, duplicate chunk, Single-mode high-nibble, negative beat, invalid flags, invalid reserved, recovered shock, arrow-wins drop), add a test that exercises the path and confirms a warn line is emitted (use `testing_logger` crate or a log-capture harness; if no such crate is already in `Cargo.toml`, this audit can be done by reading the code and confirming each `warn!` is reachable via a test input — don't add a new crate for this).
- **Final acceptance checks**:
  - `cargo run -- --help` output diff before/after this feature branch: only expected diff is any clap-internal help-text regeneration (e.g. version string). No new flag lines. Captures US-7.
  - Every acceptance criterion in `requirements.md` is mapped to a test (document the mapping in this task's completion notes, e.g. in the tasks.md acceptance section or in a PR-style summary at task close).

**Tests**:
- All existing `cargo test` green.
- New tests from this task pass.
- `cargo clippy --all-targets -- -D warnings` passes.
- `cargo fmt` produces no changes.

**Dependencies**: Tasks 1–4 complete.

- [x] 5.1 Add `NoteKind::Mine` arm to `place_note_events` in `src/ssc/notes.rs`; targeted unit test for quantize behavior on mine rows.
- [x] 5.2 Add FINISH-bracketing regression test in `src/job/mod.rs` tests for the "last note is a mine" case.
- [x] 5.3 Add integration tests: DDR→SM5 with per-difficulty mines; DDR→SM5→DDR round-trip; SM5→DDR→SM5 round-trip.
- [x] 5.4 Observability audit: confirm each `warn!` site has test coverage (add tests where missing; no new crate dependency).
- [x] 5.5 Capture `--help` output, confirm no user-visible flag changes (US-7).
- [x] 5.6 Map requirements acceptance criteria → tests in the task's completion notes; verify every US-1 through US-8 criterion is covered.
- [x] 5.7 Final verification: `cargo build`, `cargo test`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check` all pass.

---

## QA Section
**Status**: Approved
**Test Results**: 303/303 pass (synthetic tests across Tasks 1–5). Manual real-file round-trip on `Xuxa/fiwo.sm` (98 mine-bearing rows) preserves 94 per-panel mines + 4 full-row shocks through SM5→DDR→SM5.
**Feedback**: Covered in per-task checklists.

## Acceptance Section
**PM**: n/a (hobby project, user is sole arbiter)
**Status**: Approved
**Notes**: All 5 tasks approved in-session. Feature closes in a ready-for-in-game-verification state. User handles checkpoint commit + squash externally (Learning 2).
