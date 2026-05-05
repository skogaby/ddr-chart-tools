# Requirements: 20260429-ddr-mines-support

## Overview

Add tool-side support for ITG-style per-panel mine notes on the DDR side of the pipeline, via the SSQ `MINE_DATA` chunk (kind 20) defined in `docs/ssq_mine_chunk_format.md`. Before this feature, the tool's model has no concept of a per-panel mine: any `M` character in an SSC/SM input must be a full-row shock pattern, and partial-mine rows are rejected with `UnsupportedMine`. After this feature, per-panel mines flow through the pipeline end-to-end, **with per-difficulty scoping** (each chart has its own mines — Easy can have different mines than Hard):

- SSC/SM `M` characters that do **not** form a full-row shock become per-panel mines in the model.
- SSC/SM `M` characters that **do** form a full-row shock continue to be emitted as DDR shock arrows (existing behavior retained — it keeps DDR→SM5→DDR shock round-trip lossless).
- When writing SSQ, per-panel mines are serialized into **one MINE_DATA chunk per difficulty** that has mines, each keyed by the same `param2` difficulty code as the paired step chunk (per spec §2.1). Chunks are placed after all step chunks.
- When parsing SSQ, each MINE_DATA chunk is attached to the step chunk (chart) whose `param2` matches its own. Orphan mine chunks (no matching step chunk) are warned and discarded.
- When writing SSC, per-panel `Mine` notes become individual `M` characters at the appropriate panel(s) within each chart's `#NOTEDATA` section.

This feature is tool-side only. The consuming DLL mod (`NoteTypesExpansion`) is out of scope here and tracked under the sibling feature `20260427-itg-mine-support`. This feature does not add any CLI flags and does not change the set of supported `(--from-format, --to-format)` combinations.

## Glossary (additions)

- **Mine (per-panel)** — An ITG/SSC-style hazard note on a single panel. Unlike a DDR shock arrow, a mine affects only the panels it sits on. Represented in the model as a new `NoteKind::Mine` variant that carries a `PanelSet` (mirroring `NoteKind::Tap`).
- **Full-row shock row (SSC)** — An SSC row where every character is `M` and the `M`s cover every panel on one player's side (Single: all 4; Double: all 8 for both-sides, all P1 for P1-side, all P2 for P2-side). Continues to be interpreted as a `NoteKind::Shock`, not as per-panel mines.
- **MINE_DATA chunk** — The SSQ chunk with `type = 20` (`0x14`) defined in `docs/ssq_mine_chunk_format.md`. Carries `N` 8-byte entries, each `(beat_count: i32, panels: u8, flags: u8, reserved: u16)`. One chunk per difficulty-with-mines, keyed by a difficulty-code `param2`.
- **Difficulty code (in MINE_DATA context)** — The 16-bit `(slot, style)` value from `docs/ssq_format.md` §5.1 (e.g. `0x0114` = Single Basic, `0x0318` = Double Expert). Each MINE_DATA chunk's `param2` matches the `param2` of the step chunk for the same chart.
- **v1 MINE_DATA** — This feature's target version. `flags = 0`, `reserved = 0`, `param2 = <valid difficulty code>`. Forward-compatibility rules (§3.3, §6.3 of the chunk spec) are honored on the read side but never exercised on the write side.

## User Stories

### US-1: Convert per-panel SSC mines to per-difficulty SSQ MINE_DATA chunks

**As a** SM5 chart author converting my chart to DDR
**I want** per-panel `M` mines in each `#NOTEDATA` section of my SSC to land in the output SSQ as a MINE_DATA chunk keyed to that difficulty
**So that** the hook DLL mod (`NoteTypesExpansion`) plays the correct mines on each difficulty, vanilla DDR silently ignores the mine chunks, and difficulty-specific mine design (e.g. mines only on Challenge) is expressible.

**Acceptance Criteria:**
- [ ] When `--from-format SM5 --to-format DDR` is run on an SSC, the output SSQ contains **one MINE_DATA chunk per difficulty that has at least one mine note**. Each such chunk has `type = 20`, `param2 = <difficulty code>` (the same value the paired step chunk uses — `0x0114` Single Basic, `0x0318` Double Expert, etc. per `docs/ssq_format.md` §5.1), `param3 = N` (entry count for that chunk), `param4 = 0`.
- [ ] A difficulty with zero mines emits **no** MINE_DATA chunk for that difficulty. The writer does not emit `length = 12, param3 = 0` stubs.
- [ ] Each SSC `M` at panel `P` on beat `B` in a given `#NOTEDATA` section becomes exactly one MINE_DATA entry in that difficulty's chunk, with `beat_count = B × 1024` (1024 ticks per beat; 4096 ticks per measure), `panels = 1 << P`, `flags = 0`, `reserved = 0`.
- [ ] Multiple `M` characters on different panels at the same beat within the same `#NOTEDATA` section are merged into one MINE_DATA entry whose `panels` byte has multiple bits set, **unless** the set of bits forms a full-row shock pattern for the chart's style (see US-3). Entries are emitted in ascending `beat_count` order; ties break on `panels` ascending.
- [ ] All MINE_DATA chunks are placed after the last step chunk (type 3) and before the file terminator (`00 00 00 00`). The relative order among multiple MINE_DATA chunks is stable and deterministic: they are emitted in the same order as the corresponding step chunks (i.e. iteration order of `Song.charts`).
- [ ] If an `M` in the SSC sits at the same beat and same panel as a `1`, `2`, or `4` on another chart column, the mine is kept (spec §4.2 case 2). If it sits at the same beat and same panel as a `1`, `2`, or `4` in the **same** column of the **same** `#NOTEDATA` section, the mine is dropped and a `warn!` log names the difficulty, beat, and panel (spec §4.2 case 1: "arrow wins").
- [ ] A song with no mines in any difficulty produces an output SSQ that is **byte-identical** to what this tool produced before the feature landed. (No empty mine chunks, no changes to existing chunks.)
- [ ] The output SSQ continues to satisfy the existing modern-profile rules from the initial-deliverable requirements: TPS=1000 in tempo; step chunks (type 3) use 4096-ticks-per-measure; terminator is `00 00 00 00`. Adding MINE_DATA does not change the presence/shape of tempo (1), events (2), or step (3) chunks.
- [ ] Each emitted MINE_DATA chunk's `length` is exactly `12 + 8 × N`, always dword-aligned (`8N` is already mul-4, so no trailing pad is needed).
- [ ] Duplicate `(type=20, param2=X)` chunks are not emitted. The writer aggregates all mines for a single difficulty into one chunk before writing.

### US-2: Parse SSQ MINE_DATA chunks into per-difficulty per-panel Mine notes

**As a** hobbyist converting a mine-bearing DDR SSQ back to SM5
**I want** each MINE_DATA chunk to be read and its entries attached to the matching chart (by difficulty code)
**So that** DDR→SM5 conversion preserves per-difficulty mines as individual `M` characters in each `#NOTEDATA` section of the output SSC.

**Acceptance Criteria:**
- [ ] The SSQ parser recognizes `type = 20` chunks and reads them as MINE_DATA. Each chunk is attached to the step chunk (chart) whose `param2` matches the MINE_DATA chunk's `param2` (the difficulty code).
- [ ] Each 8-byte entry is decoded as `(beat_count: i32, panels: u8, flags: u8, reserved: u16)` (little-endian).
- [ ] Each valid entry becomes one `NoteKind::Mine` note on the matching chart, with `note.beat = Beat::from_measure_ticks(beat_count)` and `note.panels = PanelSet::from_bits(style, panels)` where `style` is derived from the matching step chunk's `param2` low byte (same logic already used for step-chunk difficulty decoding).
- [ ] A MINE_DATA chunk whose `param2` does **not** match any step chunk in the file is an **orphan** and is skipped with a `warn!` logging the chunk offset and the unrecognized `param2`. The run does not abort.
- [ ] A MINE_DATA chunk with `param2 == 0` is treated as orphan (no difficulty code is `0x0000`), skipped with a `warn!`. This catches legacy/malformed files that pre-date the v1 spec's difficulty-code convention.
- [ ] A MINE_DATA chunk whose `param2` decodes to an **invalid difficulty code** (outside the 10 valid values in spec §2.1) is skipped with a `warn!`.
- [ ] A MINE_DATA chunk with `param3 × 8 + 12 != length` is detected, a `warn!` logs the chunk's byte offset and the declared vs. expected length, and the **entire chunk** is skipped. The run does not abort; subsequent chunks continue to parse.
- [ ] A file with **duplicate** `(type=20, param2=X)` chunks (spec §2.2 says this is malformed) is handled by accepting the first chunk and skipping subsequent ones with a `warn!` naming the offset and difficulty code. This matches the DLL's documented "stops at first match" behavior.
- [ ] Per-entry validation (each log line names the entry's byte offset within the chunk):
  - `panels == 0x00` → skip entry, `warn!`.
  - `panels ∈ {0xFF, 0x0F, 0xF0}` → **do not skip**. Convert to a `NoteKind::Shock` at the same beat on the matching chart with the appropriate `ShockSide` (0xFF = both, 0x0F = P1Only for Double / BothSides for Single, 0xF0 = P2Only), attach it to the chart's notes, and `warn!` describing the recovery and the entry's byte offset.
  - For a Single-style chart, `panels & 0xF0 != 0` → skip entry, `warn!`.
  - `beat_count < 0` → skip entry, `warn!`.
  - `flags != 0` → skip entry, `warn!` (spec §3.3 forward-compat).
  - `reserved != 0` → skip entry, `warn!` (spec §3.3).
- [ ] Mines on the chart's `notes` vector are sorted by beat, interleaved with taps/holds/shocks, matching the model's invariant that all note kinds flow through one ordered stream per chart.
- [ ] A chart that has a step chunk but **no** matching MINE_DATA chunk is valid and common (spec §2.1: "If no MINE_DATA chunk with a matching `param2` exists, the chart has no mines on that difficulty"). The parser does not log anything for the missing-mines case.
- [ ] The parser does not reject an SSQ merely because it contains MINE_DATA chunks. Existing `DDR → SM5` and `DDR → DDR` paths continue to succeed on mine-bearing input.

### US-3: Preserve the "full-row M = shock" SSC convention

**As a** hobbyist who authored a shock arrow in SSC as a full-row `M` pattern
**I want** that row to continue to be emitted as a DDR shock arrow in the output SSQ
**So that** round-tripping a vanilla DDR chart through SM5 and back (DDR→SM5→DDR) preserves the shock-arrow semantics I had originally, without silently splitting it into four independent mines.

**Acceptance Criteria:**
- [ ] The current SSC row-classifier continues to recognize the three accepted full-row shock patterns and emit them as `NoteKind::Shock`:
  - Single: every panel is `M` (pattern `MMMM`, mask `0x0F`) → `ShockSide::BothSides`.
  - Double: every panel is `M` (pattern `MMMMMMMM`, mask `0xFF`) → `ShockSide::BothSides`.
  - Double: every P1 panel is `M`, every P2 panel is `0` (mask `0x0F`) → `ShockSide::P1Only`.
  - Double: every P2 panel is `M`, every P1 panel is `0` (mask `0xF0`) → `ShockSide::P2Only`.
- [ ] Any other SSC row containing one or more `M` characters — including rows that mix `M` with other symbols, partial-side mines on Double, and single-panel mines on Single — is emitted as one or more `NoteKind::Mine` notes, **not** as a shock. This replaces the current `SscError::UnsupportedMine` behavior. The `UnsupportedMine` error variant is removed (or, if kept for forward compat, never returned).
- [ ] On the SSQ writer side, `NoteKind::Shock` continues to be serialized as a step-chunk byte (`0xFF / 0x0F / 0xF0`), **not** as a MINE_DATA entry. Only `NoteKind::Mine` notes go into MINE_DATA.
- [ ] For SSC inputs where multiple `M`s on the same beat happen to cover every panel on a side, the SSC parser collapses them into one `Shock` as above; the writer therefore emits a step-byte shock. A mine-only author who actually wants "four independent mines hitting simultaneously on all 4 panels of Single" cannot express that intent in SSC after this feature — the row will always classify as a shock. This is a documented trade-off; see Open Question #2.
- [ ] Mixed-content rows that today would trigger `SscError::UnsupportedMine` (e.g. one `M` alongside a `1` on a different panel) now parse cleanly: the `1` becomes a tap, the `M` becomes a per-panel mine, both land on the chart's `notes` vector at the same beat.

### US-4: Write per-panel mines into SSC output

**As a** hobbyist converting a mine-bearing DDR SSQ to SSC
**I want** MINE_DATA entries to appear as individual `M` characters in the output SSC
**So that** the mines are preserved for gameplay in StepMania 5.

**Acceptance Criteria:**
- [ ] For each `NoteKind::Mine` note at beat `B` with `panels` bitmask, the SSC writer emits an `M` at every panel bit set in `panels`, on the grid row corresponding to beat `B` within its measure.
- [ ] Mines co-existing with taps/holds at the same beat but on different panels render correctly: each panel slot independently shows its own character (`1`, `2`/`3` for hold endpoints, or `M`), matching the existing multi-event-per-row grid-emission code path.
- [ ] The quantizer in `ssc/notes.rs::pick_quantize` picks the same quantize for a row that has mines as it would for the same row without mines, i.e. mine placement does not force a higher row quantize unless the mine beat itself requires it. (Mines land on beats just like taps do; they do not introduce new fractional offsets.)
- [ ] If a `Shock` and a `Mine` happen to co-exist at the same beat (possible only if the SSQ parser was given a chunk where both a step-byte shock and a MINE_DATA entry landed at the same tick), the SSC writer emits the `M` characters over top of the shock's `M` pattern when they overlap. No extra error handling is required — the grid-emission code writes each event's character into its slot, and identical overwrites are a no-op. A `warn!` fires at parse time when the overlap is detected so the user is aware (see US-2 acceptance).

### US-5: Round-trip fidelity including per-difficulty mines

**As a** maintainer
**I want** a DDR-with-mines SSQ (possibly with different mines on different difficulties) to survive DDR→SM5→DDR roundtrip with per-difficulty mines preserved
**So that** the round-trip fidelity story extends to the new note type without regressing shocks or taps, and difficulty-specific mine design is preserved.

**Acceptance Criteria:**
- [ ] An integration test exists for SM5→DDR that feeds an SSC with two `#NOTEDATA` sections (e.g. Single Easy, Single Hard) containing **different** per-panel mine layouts, and asserts the output SSQ contains two MINE_DATA chunks, each with the correct `param2` difficulty code and expected entries.
- [ ] An integration test exists for SM5→DDR where one difficulty has mines and another does not. The output SSQ contains exactly one MINE_DATA chunk (for the mine-bearing difficulty); the mine-free difficulty gets no MINE_DATA chunk.
- [ ] An integration test exists for DDR→SM5 that feeds an in-code-synthesized SSQ with tempo/events/two step chunks (different difficulty codes)/two matching MINE_DATA chunks (matching `param2`), and asserts each output SSC `#NOTEDATA` section's grid contains the correct `M` characters for its difficulty.
- [ ] A DDR→SM5 test with an **orphan** MINE_DATA chunk (param2 not matching any step chunk) confirms the chunk is skipped with a `warn!` and the rest of the file parses successfully.
- [ ] A DDR→DDR round-trip test (synthetic SSQ in, SSQ out) preserves the MINE_DATA chunks per-difficulty, subject to the writer's sort-order normalization (ascending `beat_count`, ties on `panels` ascending). Each chunk's output bytes are asserted against regenerated expected byte sequences, not against input bytes — the writer is allowed to re-order entries within a chunk.
- [ ] A shock-round-trip regression test still passes: an SSC with a full-row `MMMM` row, taken DDR→SM5→DDR→SM5, reproduces the full-row `MMMM` row on each SSC pass. The feature's new code paths do not accidentally split full-row shocks into per-panel mines.
- [ ] A "vanilla SSQ" test (an SSQ with no MINE_DATA chunk) continues to parse and write successfully, producing no MINE_DATA chunk on output. Existing `initial-deliverable` tests remain green.
- [ ] All mine-related tests use synthetic fixtures built in-code from the byte layouts in `docs/ssq_mine_chunk_format.md` and `docs/ssq_format.md`. No real Konami assets, no real community charts, no on-disk fixtures beyond what is checked in to `tests/fixtures/`. (Consistent with Learning 5 in `.spec/learnings/sdd-software-developer.md`.)

### US-6: Vanilla-compatibility guarantee

**As a** user with unmodded DDR World hardware
**I want** a mine-bearing SSQ from this tool to load and play exactly like an arrow-only version of the same chart on my hardware
**So that** I don't have to choose between authoring for modded and vanilla users.

**Acceptance Criteria:**
- [ ] The tool never emits a chunk type greater than `17` except for `20` (MINE_DATA). No other kind-20+ chunks exist in the writer.
- [ ] Every emitted MINE_DATA chunk uses `param2 = <valid difficulty code>` (one of the 10 values in `docs/ssq_format.md` §5.1), `param3 = N ≥ 1`, `param4 = 0`. No chunk has `param2 == 0xFFFF` or any other non-difficulty-code value.
- [ ] All MINE_DATA chunks sit strictly after the last step chunk. A vanilla DDR World chunk walk (which is by `(type, param2)` for steps and by `type` for tempo/events) finds every vanilla chunk before reaching any MINE_DATA, then encounters each MINE_DATA, fails to match any known `(type, param2)` pair for non-mine purposes, advances by `length`, and reaches the terminator — exactly the behavior documented in `docs/ssq_mine_chunk_format.md` §6.1.
- [ ] No existing integration test's output SSQ byte sequence regresses when compared to the feature's baseline (i.e. a chart with zero mines across all difficulties must produce the same output bytes it did before this feature). This is verified by a byte-equality assertion on one representative no-mine fixture.

### US-7: No CLI surface changes

**As a** user of the CLI
**I want** the existing `--help` and flag grammar to be unchanged by this feature
**So that** scripts and muscle memory continue to work.

**Acceptance Criteria:**
- [ ] No new flags are added. No existing flag's meaning or validation changes.
- [ ] `ddr-chart-tools --help` output is unchanged except for anything already required by `clap` help-text regeneration (e.g. version string). Spot-check by diffing `--help` output before and after the feature.
- [ ] Mines flow end-to-end whenever the source format contains them; there is no opt-in, no opt-out, no `--mines-as-shocks`, no `--strip-mines`. If a future user wants to strip mines they can omit them from the SSC before running the tool.

### US-8: Error and warning observability

**As a** user debugging a mine-related parse/write problem
**I want** every mine-related skip, drop, or recovery to produce a log line that names the file offset (for binary) or beat/panel (for text) and explains what happened
**So that** I can distinguish "chunk was malformed" from "chunk was valid but co-located with an arrow so the mine was dropped".

**Acceptance Criteria:**
- [ ] Every `warn!` path enumerated in US-2 (panels=0, panels=0xFF/0x0F/0xF0 recovered to shock, Single-chart high-nibble, negative beat_count, flags!=0, reserved!=0, orphan param2, invalid difficulty code, duplicate chunk, length mismatch) produces exactly one log line per triggering entry/chunk, at `warn!` level (visible under `--quiet`), naming the entry's byte offset (or the chunk's byte offset for chunk-level issues) and the rule that fired.
- [ ] The SM5→DDR "arrow wins" drop (US-1) produces a `warn!` log line naming the source SSC file path (already threaded into the job layer), the difficulty (style + slot), the beat, and the panel.
- [ ] No mine-related code path ever uses `println!` for diagnostics. All diagnostic output goes through the `log` facade per the project's Rust CLI standards.

## Out of Scope

Intentionally deferred. Say "no" to scope creep that tries to pull any of these in.

- **DLL-mod-side work** — the `NoteTypesExpansion` mod changes that consume MINE_DATA are tracked under the sibling feature `20260427-itg-mine-support`. This feature ships the tool-side half only.
- **Future chunk kinds (21+)** for lifts (`L`), rolls (`R`), fakes (`F`), and other SSC note types. The mine-chunk spec §5 explicitly scopes those out, and they remain silently dropped on SSC parse with a `warn!`, as they are today.
- **Non-zero `flags` bits** — the writer always emits `flags = 0`. Readers skip non-zero flags with a warn, preserving forward-compat. No code path sets any flag bit. Per the updated spec (§3.3, §6.3), `flags` — not `param2` — is the canonical extension point for future mine variants.
- **Mine sub-type discrimination** — orthogonal to the per-difficulty `param2` this feature uses. Future sub-types (cold mines, cosmetic mines, etc.) land on `flags` bits in a v2 of the chunk spec.
- **DDR_LEGACY → anything paths.** Legacy SSQs (TPS ≠ 1000, from pre-DDR-World releases) do not contain MINE_DATA chunks by definition (the kind was defined after DDR World). No modernization logic runs on MINE_DATA. If a malformed legacy SSQ somehow contains a kind-20 chunk, the parser will read it under the same rules as a World-profile SSQ — no special case.
- **Editing `docs/ssq_mine_chunk_format.md`.** It is authoritative. As of 2026-04-29 the spec is internally consistent (the stale `param2 == 0` references from the pre-per-difficulty draft were cleaned up via a handoff to the spec-authoring agent). Implementation follows §2.1 verbatim.
- **Editing `docs/ssq_format.md`** to add a back-reference to the mine-chunk spec. The mine-chunk spec already links to ssq_format.md; a reciprocal link is nice-to-have but not required for this feature. A future documentation pass may add it.
- **New CLI flags** of any kind (see US-7).
- **Mine density limits, author-time warnings on "too many mines," or per-chart validation beyond what the MINE_DATA parser does.** Out of scope.
- **Real-hardware testing.** Consistent with the initial deliverable, no automated CI runs against real hardware or real Konami assets.

## Open Questions

These are flagged for the design phase (principal engineer) to resolve where a code-structure decision is needed, or noted for future retrospection.

1. **"Four simultaneous per-panel mines on Single" is not expressible in SSC after this feature.** Because a full-row `MMMM` on Single unambiguously classifies as `ShockSide::BothSides`, there is no way to distinguish "four independent mines hitting at once" from "a shock arrow" in the SSC input grammar. This is a trade-off made to preserve DDR→SM5→DDR shock round-trip (US-3). If a user someday wants "four per-panel mines at one beat" as a distinct construct, they would need an SSC-level sentinel we don't have, or a different input format. For this feature, we live with the ambiguity and document it.

2. **Sort-order determinism on write.** The writer sorts by `beat_count` ascending with ties on `panels` ascending (spec §4.1). Within the same beat and same panel bitmask, there can only be one valid entry. The parser does not re-order incoming entries — it preserves the order they appear on disk, which means a DDR→DDR round-trip where the input was not sorted ascending will **re-sort** the output. This is fine per the spec (§4.1 "The game tolerates any order"), but tests must assert against a sorted-expected value, not against the input bytes.

3. **Chunk emission order for multiple MINE_DATA chunks.** When a song has mines on multiple difficulties, the writer emits one MINE_DATA chunk per difficulty. The relative order among those chunks is specified as "same order as `Song.charts`" (see US-1). DDR doesn't care about the order (the DLL looks them up by `(type, param2)`), so this is a deterministic-output decision, not a correctness one. Design phase should confirm the iteration source is stable.

## Dependencies

- **`docs/ssq_mine_chunk_format.md`** — authoritative v1 spec for the MINE_DATA chunk. Implementation follows it byte-for-byte.
- **`docs/ssq_format.md`** — existing SSQ spec; mine chunk sits alongside the tempo/events/step/aux chunks it defines.
- Existing crates already in `Cargo.toml` (no new dependencies expected for this feature). The design phase may reconsider.
- The `NoteKind::Mine` variant is new to `src/model/mod.rs`. The principal engineer owns the decision of whether `Mine` should carry a `PanelSet` inline (matching `Tap`) or some other shape — the user has agreed with "similar to `Tap`" in Q3 of discovery.

## Assumptions

- One MINE_DATA chunk per difficulty, per file. Each chunk's `param2` matches exactly one step chunk's `param2`. If multiple MINE_DATA chunks share the same `param2` (malformed input), the first is accepted and subsequent duplicates are warned+skipped.
- Mines do not participate in freeze/hold resolution. A `0x00` step byte never ends a mine — mines are not step-chunk data and are completely independent of the freeze-block parser state.
- Mines do not contribute to BPM calculation, tempo-chunk `tempo_data[]` synthesis, or the `finish-bracketing-guard` logic from Learning 11 (`sdd-software-developer.md`). The last-chart-beat calculation for the FINISH guard may include mine beats — that is a design-phase decision (principal engineer).
- Mine writing is additive: a chart with zero mines produces byte-identical output to a chart authored before this feature. Existing initial-deliverable tests stay green.
- The feature ships as a single user-approved unit. No phased rollout; the DDR→DDR, DDR→SM5, SM5→DDR paths all learn about mines together.

## Notes for Principal Engineer

Design decisions that need explicit attention, with pointers to the code they touch:

- **New `NoteKind::Mine` variant.** Decide whether it's `Mine { /* no fields */ }` with panel info inherited from `Note.panels` (matching `Tap`), or `Mine { panels: PanelSet }` with a payload. User's Q3 answer says "like Tap." Keep the `Note.panels` field authoritative for panel info across all note kinds; don't duplicate.
- **Removing `SscError::UnsupportedMine`.** The error variant and its call sites in `src/ssc/notes.rs::collect_mine_bits` and `decode_row` need reworking. The full-row classifier still exists (US-3); the rejection path does not. `collect_mine_bits` needs a new return signature that distinguishes "full-row shock" (emit `Shock`) from "some mines, possibly mixed with taps/holds" (emit per-panel `Mine` notes alongside any taps).
- **`docs/ssq_mine_chunk_format.md` §4.2 co-location logic** on the SSQ writer path. When the writer sees a `Mine` and an arrow on the same panel at the same beat in the **same chart**, drop the mine and warn. This logic belongs in the SSQ writer (`src/ssq/writer.rs`) or an intermediate pass, not in the SSC parser.
- **MINE_DATA chunk placement** on write: after all step chunks, before terminator. The existing `write` in `src/ssq/writer.rs` has an obvious insertion point between the `for chart in &song.charts { write_steps_chunk(chart, out)?; }` loop and the terminator write. The new writer iterates the same `song.charts` again to emit per-difficulty mine chunks in the same order.
- **Per-difficulty `param2` discriminator.** Each MINE_DATA chunk's `param2` matches the paired step chunk's `param2`. Both derive from `(chart.style, chart.difficulty)` via the existing `difficulty_code` helper in `src/ssq/writer.rs`. Reuse it.
- **Parsing**: `src/ssq/mod.rs::dispatch_chunk` currently routes `4 | 5 | 9 | 17` to the aux-dropped path and anything else to `SsqError::UnexpectedChunkType`. Add a `20 => ...` arm that dispatches to a new `src/ssq/mines.rs` module. Parsing proceeds in two phases: (a) collect all MINE_DATA chunks during the initial dispatch pass, keyed by their `param2`; (b) after all chunks are read, attach each MINE_DATA chunk's entries to the step chunk with matching `param2`. Orphan chunks (unmatched `param2`) log a warn and are discarded.
- **Model placement**: `src/model/mod.rs` is the right home for `NoteKind::Mine`. No new cross-format types needed.
- **`SsqParseResult` sidecar**: unlike events and raw tempo pairs, mines do land on the common `Song.charts[i].notes`, so they do **not** need a sidecar. The DDR→DDR path will therefore re-serialize mines from the model rather than round-trip raw bytes. This is safe because the 8-byte entry shape has no padding or hidden fields that round-tripping might lose — the writer can reconstruct byte-exact output from `(beat, panels, flags=0, reserved=0)`.
- **Writer-side deduplication**: within a single chart, when one `Mine` note lists multiple bits in `panels`, the writer emits one MINE_DATA entry with that multi-bit mask. When multiple `Mine` notes on the same chart at the same beat each list a single bit, the writer merges them into one multi-bit entry. Follow the sort rule (§4.1) — the merge is a simple group-by-beat + OR of panel masks within a chart's own mine list. **Do not merge across charts** — different difficulties get different chunks.
- **Test harness**: the existing `tests/fixtures/` layout and synthetic-byte-building helpers (e.g. `src/ssq/mod.rs` tests `build_ssq`, `tempo_body`, `events_body`) are the model. Add `mine_body(entries: &[(i32, u8)])` helper under `src/ssq/mod.rs::tests` or a new module, following the same pattern. Tests should cover multi-difficulty SSQs (e.g. step chunk `0x0114` + mine chunk `0x0114` + step chunk `0x0314` + mine chunk `0x0314`).
- **Events chunk invariants from Learning 11**: the FINISH bracketing guard still applies. Mines do not affect it, because mines are in a separate chunk that isn't routed through `synthesize_events` — but the `max_chart_beat` calculation (which feeds the "synthesize trailing tempo pair at `last_note + 2 measures`" logic) should include mine beats so a song whose last note is a mine still has its FINISH bracketed correctly.
