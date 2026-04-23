# Tasks: 20260422-initial-deliverable

Each task is a shippable CR: builds and tests independently, ~200 source lines max, 2–3 new files max.

## Workspace Info
**Primary Package**: ddr-chart-tools
**All Packages**: ddr-chart-tools

---

## Task 1: Project scaffold
**Package(s)**: ddr-chart-tools
**Goal**: Empty crate builds; `ddr-chart-tools --help` runs and exits 0; top-level `Error` enum exists.
**Scope**:
- `Cargo.toml` with `clap`, `anyhow`, `thiserror`, `log` deps
- `src/main.rs` — parse args (stub), setup logging, translate errors to exit codes
- `src/lib.rs` — re-exports for integration tests
- `src/error.rs` — top-level `Error` enum with placeholder variants
**Tests**: `cargo build --release` succeeds; `cargo test` runs with zero tests.
**Dependencies**: none

- [x] 1.1 Write Cargo.toml with pinned dep versions per design Decision 10/9
- [x] 1.2 Stub main.rs with clap + env_logger setup
- [x] 1.3 Create lib.rs and error.rs

---

## Task 2: Utilities (logging + byte I/O)
**Package(s)**: ddr-chart-tools
**Goal**: Shared helpers for verbose-count-to-log-level mapping and little-endian byte reading with offset tracking.
**Scope**:
- `src/util/logging.rs` — `init(verbosity: u8)` → configures `env_logger`
- `src/util/io.rs` — `LeReader` wrapper over `&[u8]` with `read_u16`, `read_u32`, `read_bytes`, offset-in-error reporting
**Tests**: Unit tests for LeReader EOF behavior and log-level mapping.
**Dependencies**: Task 1

- [x] 2.1 logging.rs with `-v`/`-vv` → Info/Debug mapping
- [x] 2.2 io.rs LeReader with offset tracking
- [x] 2.3 Unit tests

---

## Task 3: Common model types
**Package(s)**: ddr-chart-tools
**Goal**: Format-independent `Song`, `Chart`, `Note`, `AudioBuffer`, `PreviewSlice`, and a rational-arithmetic `TickScale` for lossless TPS rescaling.
**Scope**:
- `src/model/mod.rs` — Song/Chart/Note/AudioBuffer/TempoSegment/Stop structs; Style/Difficulty/NoteKind/ShockSide enums
- `src/model/tick.rs` — `TickScale` using integer rational math (no floats) per Risk mitigation
- `src/model/preview.rs` — PreviewSlice struct + default-preview constructor
**Tests**: TickScale round-trip 150↔1000 with no drift; Note ordering; PanelSet bitmask ops.
**Dependencies**: Task 1

- [x] 3.1 model/mod.rs types
- [x] 3.2 tick.rs with rational arithmetic
- [x] 3.3 preview.rs + unit tests

---

## Task 4: SSQ parser framework + tempo chunk
**Package(s)**: ddr-chart-tools
**Goal**: Parse any SSQ (modern or legacy) chunk headers and type-1 tempo chunks (BPM segments, stops, TPS detection).
**Scope**:
- `src/ssq/mod.rs` — `parse(bytes) -> Result<Song, SsqError>` stub wiring chunks to a partial Song; `SsqError` enum
- `src/ssq/chunk.rs` — chunk header I/O (type, length, param)
- `src/ssq/tempo.rs` — type-1 chunk → TempoSegment + Stop + detected TPS (150 vs 1000)
**Tests**: Parse tempo chunks from both a modern DDR World SSQ and a legacy SSQ fixture; TPS detection correct; stops extracted.
**Dependencies**: Tasks 2, 3

- [x] 4.1 SsqError + mod.rs parse dispatcher
- [x] 4.2 chunk.rs framing
- [x] 4.3 tempo.rs type-1 parsing + unit tests

---

## Task 5: SSQ events + aux chunks
**Package(s)**: ddr-chart-tools
**Goal**: Parse type-2 (song markers) and type-4/5/9/17 (legacy auxiliary) chunks.
**Scope**:
- `src/ssq/events.rs` — type-2 parsing into Song fields
- `src/ssq/aux.rs` — types 4/5/9/17 parsed into opaque blobs; flagged as legacy-only so modernize drops them
**Tests**: Unit tests on fixture chunks; aux chunks round-trip as opaque bytes.
**Dependencies**: Task 4

- [x] 5.1 events.rs type-2 parser
- [x] 5.2 aux.rs opaque-blob parser
- [x] 5.3 Unit tests

---

## Task 6: SSQ steps chunk
**Package(s)**: ddr-chart-tools
**Goal**: Parse type-3 chunks (one chart each) into model `Chart` with taps, freezes, and shocks.
**Scope**:
- `src/ssq/steps.rs` — type-3 parser: notes, freeze headers + lengths, shock-arrow encoding (both-sides vs P1/P2)
**Tests**: Parse a known DDR World SSQ; verify note counts per chart match StepManiaPaX's output on the same file.
**Dependencies**: Task 5

- [x] 6.1 Note/freeze/shock decoding
- [x] 6.2 Panel bitmask assembly (Single vs Double)
- [x] 6.3 Unit tests against fixture SSQs

---

## Task 7: SSQ writer (modern profile) + legacy modernize
**Package(s)**: ddr-chart-tools
**Goal**: Write modern-profile SSQs (TPS=1000, chunks 1/2/3 only); transform legacy-parsed Songs into modern profile via tick rescaling + aux-chunk drops (logged at `warn`).
**Scope**:
- Extend `src/ssq/mod.rs` with `write(&Song, &mut Write) -> Result<(), SsqError>` — cannot emit legacy by construction
- `src/ssq_legacy/modernize.rs` — `modernize(&mut Song)` rescales ticks 150→1000 using TickScale, drops aux, warn-logs each drop
**Tests**: DDR_LEGACY fixture → modernize → write → re-parse round-trips; modernized output has only types 1/2/3.
**Dependencies**: Task 6

- [x] 7.1 SSQ writer (modern-only)
- [x] 7.2 ssq_legacy/modernize.rs transform
- [x] 7.3 Round-trip integration test

---

## Task 8: SSC parser (MSD + notes)
**Package(s)**: ddr-chart-tools
**Goal**: Parse SSC files into model `Song` including `#NOTEDATA` blocks.
**Scope**:
- `src/ssc/mod.rs` — `parse(text) -> Result<Song, SscError>` + SscError enum
- `src/ssc/msd.rs` — shared MSD tokenizer (reused by sm/ in Task 10)
- `src/ssc/notes.rs` — `#NOTEDATA` parse: stepstype, difficulty, note grid, holds, rolls→reject-as-unsupported
**Tests**: Parse a StepManiaPaX-generated SSC; verify Song matches source fixture's expected model.
**Dependencies**: Tasks 2, 3

- [x] 8.1 msd.rs tokenizer
- [x] 8.2 ssc/mod.rs parser
- [x] 8.3 notes.rs #NOTEDATA parser + unit tests

---

## Task 9: SSC writer
**Package(s)**: ddr-chart-tools
**Goal**: Write SSC files (never SM) with `#NOTEDATA` blocks for each chart.
**Scope**:
- Extend `src/ssc/mod.rs` with `write(&Song, &mut Write) -> Result<(), SscError>`
- Extend `src/ssc/notes.rs` with `#NOTEDATA` serializer (note grid, freeze→hold, shock handling per US-2)
**Tests**: DDR → parse → write SSC → re-parse in StepMania (offline sanity); unit round-trip on fixtures.
**Dependencies**: Task 8

- [x] 9.1 ssc writer
- [x] 9.2 notes writer
- [x] 9.3 Round-trip tests

---

## Task 10: SM parser (read-only)
**Package(s)**: ddr-chart-tools
**Goal**: Parse legacy SM files into model `Song` (no writer — SSC is the only SM5 output format).
**Scope**:
- `src/sm/mod.rs` — reuses ssc::msd tokenizer
- `src/sm/notes.rs` — `#NOTES` block parser (5-section `:`-separated format)
**Tests**: Parse a known SM fixture; verify Chart count + note counts.
**Dependencies**: Task 8 (reuses msd.rs)

- [x] 10.1 sm/mod.rs
- [x] 10.2 sm/notes.rs + unit tests

---

## Task 11: WAVM decoder (XBOX-IMA)
**Package(s)**: ddr-chart-tools
**Goal**: Decode headerless WAVM (fixed 2ch/44100Hz XBOX-IMA) into PCM.
**Scope**:
- `src/wavm/mod.rs` — `parse(bytes) -> Result<AudioBuffer, WavmError>`; no writer
- `src/wavm/xbox_ima.rs` — XBOX-IMA decoder ported from `~/Desktop/vgmstream/src/coding/ima_decoder.c` (~100 LOC)
**Tests**: Decode a real legacy WAVM sample; verify PCM length matches expected duration × 44100 × 2.
**Dependencies**: Tasks 2, 3

- [x] 11.1 xbox_ima.rs decoder port
- [x] 11.2 wavm/mod.rs wrapper + unit tests

---

## Task 12: XWB container (parse + write)
**Package(s)**: ddr-chart-tools
**Goal**: Parse and write XWB v43 containers (WBND header, 5-segment layout, 2-entry structure); exposes raw MS-ADPCM block bytes on read, accepts raw block bytes on write.
**Scope**:
- `src/xwb/mod.rs` — stub `parse` and `write` entry points with ADPCM hook points
- `src/xwb/container.rs` — WBND header, segment table, entry metadata, WAVEBANKMINIWAVEFORMAT bit-packing
**Tests**: Round-trip every `~/Desktop/DDR WORLD/.../dance/*.xwb` — parse-then-write must be byte-identical (catches bit-packing mistakes per Risk table).
**Dependencies**: Tasks 2, 3

- [x] 12.1 WBND header + segment table
- [x] 12.2 Entry metadata + bitfield pack/unpack
- [x] 12.3 Byte-identical round-trip test over all 12 DDR World XWBs

---

## Task 13: MS-ADPCM decoder
**Package(s)**: ddr-chart-tools
**Goal**: Decode MS-ADPCM blocks (128 samples/block, 2 channels) → PCM. Completes the XWB→PCM pipeline.
**Scope**:
- `src/xwb/adpcm/mod.rs` — public decode entry; error type
- `src/xwb/adpcm/decode.rs` — per-block decoder (7 standard predictor coefficients, adaptive step)
**Tests**: Decode fizz.xwb, verify duration; decode → hound-reencode → decode self-consistency.
**Dependencies**: Task 12

- [x] 13.1 adpcm/mod.rs
- [x] 13.2 decode.rs per-block
- [x] 13.3 Wire into xwb::parse; unit tests

---

## Task 14: MS-ADPCM encoder ⚠ highest risk
**Package(s)**: ddr-chart-tools
**Goal**: Encode PCM → MS-ADPCM blocks DDR World accepts. Pure-Rust first attempt per Decision 5; vendored C fallback is a separate follow-up if this fails.
**Scope**:
- `src/xwb/adpcm/encode.rs` — per-block encoder: predictor selection, 4-bit delta quantization with adaptive step
**Tests**:
  - Self-round-trip: encode PCM → decode via Task 13 → SNR ≥ 55 dB against original
  - Encode a DDR World sample's PCM, wrap in XWB (Task 12), open in vgmstream as reference sanity check
  - Manual: load one produced XWB in a real DDR World build before merge
**Dependencies**: Task 13

- [x] 14.1 Predictor coefficient selection
- [x] 14.2 Block encoder with adaptive step
- [x] 14.3 Self round-trip + vgmstream sanity test
- [x] 14.4 If DDR rejects output: open follow-up task for vendored C fallback (Decision 5b)

---

## Task 15: XSB template writer
**Package(s)**: ddr-chart-tools
**Goal**: Generate XSBs by patching a static template with the 4-char song code per Decision 7.
**Scope**:
- `src/xsb/mod.rs` — `write(name, out)` loads template, zeroes 0x08-0x0f timestamp, patches 0x4a/0x8a/0x13a/0x13f name fields
- `src/xsb/template.bin` — extracted from `fizz.xsb` with name regions zeroed (dev task documented inline)
**Tests**: `write("fizz", ...)` produces bytes byte-identical to real `fizz.xsb` (after zeroing the known timestamp region on the reference).
**Dependencies**: Task 1

- [x] 15.1 Extract template.bin from fizz.xsb (dev script)
- [x] 15.2 xsb/mod.rs name-patch writer
- [x] 15.3 Byte-identical fizz test

---

## Task 16: OGG Vorbis decode + encode
**Package(s)**: ddr-chart-tools
**Goal**: OGG Vorbis I/O for SM5 audio — `lewton` decode, `vorbis_rs` (static libvorbis) encode per Decision 9.
**Scope**:
- `src/ogg/mod.rs` — entry points + OggError
- `src/ogg/decode.rs` — lewton wrapper → AudioBuffer
- `src/ogg/encode.rs` — vorbis_rs wrapper ← AudioBuffer (quality setting chosen to match StepManiPaX default)
**Tests**: Decode a known OGG, verify duration; encode + self-decode round-trip under Vorbis-quantization SNR threshold.
**Dependencies**: Task 3

- [x] 16.1 ogg/mod.rs + decode.rs
- [x] 16.2 encode.rs with vorbis_rs
- [x] 16.3 Round-trip tests

---

## Task 17: CLI + batch pairing
**Package(s)**: ddr-chart-tools
**Goal**: Parse argv into validated `Cli`; reject `--to-format DDR_LEGACY`; enforce `--chartfile`/`--audiofile` coupling and `{files} xor {input-folder}`; build `Vec<Job>`. Batch mode pairs basenames across formats.
**Scope**:
- `src/cli/mod.rs` — Cli struct (clap derive), `validate()`, `into_jobs()`
- `src/cli/job.rs` — Job struct + Format enum
- `src/util/pair.rs` — basename pairing for batch mode per US-5; ambiguity → per-file error
**Tests**: Table-driven tests on valid + invalid argv; pair resolver tests covering ambiguity (two audio formats for same basename) and missing-partner cases.
**Dependencies**: Task 3

- [x] 17.1 cli/mod.rs + job.rs with clap derive
- [x] 17.2 validate() + into_jobs()
- [x] 17.3 util/pair.rs + unit tests

---

## Task 18: Job orchestrator (run_one)
**Package(s)**: ddr-chart-tools
**Goal**: Execute one `Job` end-to-end: read inputs, dispatch to right parser/writer per `(from, to)` pair, handle DDR_LEGACY→DDR byte-copy passthrough with compliance check per Decision 3.
**Scope**:
- `src/job/mod.rs` — `run_one(&Job) -> Result<(), Error>`; dispatch table; passthrough compliance check (XWB header match + XSB present + template match)
**Tests**: Each of the 4 supported directions runs end-to-end on fixture files; passthrough triggers only for compliant legacy inputs.
**Dependencies**: Tasks 7, 9, 10, 11, 12, 13, 14, 15, 16, 17

- [x] 18.1 Dispatch scaffolding
- [x] 18.2 Passthrough compliance check
- [x] 18.3 Per-direction integration tests on fixtures

---

## Task 19: Batch runner
**Package(s)**: ddr-chart-tools
**Goal**: Run `Vec<Job>` with per-file error recovery; emit summary (attempted/succeeded/failed/skipped); surface non-zero exit if any failed.
**Scope**:
- `src/job/batch.rs` — `run_batch(&[Job]) -> BatchSummary`; continues past per-file errors (logged at `error`); exit-code mapping in main.rs
**Tests**: Batch with mixed success/failure fixtures; summary counts correct; process exits non-zero on any failure.
**Dependencies**: Task 18

- [x] 19.1 run_batch loop + summary struct
- [x] 19.2 main.rs exit-code wiring
- [x] 19.3 Tests on mixed-outcome batches

---

## QA Section
**Status**: Pending
**Test Results**:
**Feedback**:

## Acceptance Section
**PM**: pending
**Status**: Pending
**Notes**:
