# Technical Context

## Technology Stack

- **Language**: Rust (2021 edition, stable toolchain — pin to latest stable at project bootstrap time)
- **Crate type**: Binary (`[[bin]]`). Not a library. Cargo.lock is committed.
- **Target platforms**: macOS and Linux primarily (developer's environment is macOS). Windows support is best-effort — avoid platform-specific code, but don't spend time testing on Windows.
- **Distribution**: `cargo install` from source, or a prebuilt binary. No package-manager integration yet.

## Crate Dependencies

| Crate | Version | Purpose |
|-------|---------|---------|
| `clap` (derive) | 4 | CLI arg parsing with derive macros |
| `anyhow` | 1 | Error context in CLI/orchestration layer |
| `thiserror` | 2 | Typed errors in format parser/writer layers |
| `log` | 0.4 | Logging facade |
| `env_logger` | 0.11 | Log subscriber (verbosity from `-v` flags) |
| `lewton` | 0.10 | OGG Vorbis decode (pure Rust) |
| `vorbis_rs` | 0.5 | OGG Vorbis encode (static libvorbis via `cc`) |
| `rustfft` | 6 | FFT for the auto-sync onset envelope (`sync/`). Pure Rust, runtime-selected SIMD, no C code; adds 7 small pure-Rust crates |

Dev-only: `tempfile = "3"` for filesystem tests.

All audio codecs (MS-ADPCM, XBOX-IMA, OGG Vorbis) and the auto-sync signal analysis are in-process. No external CLI tools at runtime.

## Cross-Compilation

Windows cross-compile from macOS uses `x86_64-pc-windows-gnu` target with `mingw-w64`. Config in `.cargo/config.toml`.

## Architecture Patterns

### Layered pipeline

The tool is a pure data-in / data-out pipeline. No state, no network, no concurrency needed for the initial deliverable. The canonical layering:

```
CLI args  →  Job planner  →  Per-job converter  →  Format parser/writer  →  Disk I/O
```

- **CLI layer** (`cli/`): parses args, validates the `(from, to)` combination against the rules in product.md, resolves input paths into a list of jobs (single or batch).
- **Job layer** (`job/` or `convert/`): takes a single conversion job and orchestrates the parser → in-memory model → writer flow. Handles per-file error recovery in batch mode.
- **Format layer** (one module per format, e.g. `ssq/`, `ssc/`, `sm/`, `xwb/`, `ogg/`): each module owns parse + write for its format. Public types are the parsed in-memory representation. Internal submodules handle binary/text I/O details.
- **Model layer** (`model/`): format-independent types the converters translate through (e.g. `Song`, `Chart`, `Note`, `TempoChange`, `Freeze`). Every conversion is `source format → model → target format`, not point-to-point.

### Never point-to-point conversion

Always go through the common model. A direct `ssq_to_ssc` function creates O(n²) conversion paths as formats are added and hides semantic gaps. The model layer forces us to name every concept that crosses a format boundary.

### Errors

- Use `thiserror` for parser/writer errors. Each format module defines its own error enum.
- Use `anyhow` at the CLI and job-orchestration layers to wrap typed errors with context.
- Parse errors include byte offsets / line numbers where possible. "Parse failed" with no location is not acceptable.
- In batch mode, a failure on one file is logged and the run continues. In single mode, a failure is fatal.

### Logging

- Default log level: `info` (per-file outcomes, summary counts).
- `-v` raises to `debug` (per-chunk parsing, field values).
- `-vv` raises to `trace` (byte-level reads).
- Dropped-data warnings from legacy modernization are at `warn` level, always visible.

## Integration Points

This tool has no external integrations. Everything is local file I/O. No network, no IPC, no subprocess calls (unless the design phase picks a shell-out strategy for OGG encoding — flagged explicitly if so).

## Common Technical Gotchas

- **SSQ endianness and alignment**: little-endian throughout, all chunks dword-aligned, but the freeze-info block inside step chunks is 2-byte aligned. Don't assume everything is 4-byte aligned.
- **SSQ TPS is per-file**: the tempo chunk's `param2` is the tick rate. Don't hardcode 1000. Observed values are 1000, 150, and 75; any positive `u16` is legal.
- **Legacy `time_offset[0]` is not always 0**: it encodes an origin-shift between the chart timeline and the audio-sync timeline. The parser accepts any value; modernization normalizes to 0 and rescales seconds-ticks accordingly.
- **SSQ chunk lookup has two sentinels**: `length == 0` and `param2 == 0xFFFF` both terminate chunk scans. Writers must not emit either value spuriously.
- **SM vs SSC parsing**: both use MSD-style `#TAG:VALUE;` syntax but SSC has per-chart timing sections that SM lacks. Don't assume an SM parser handles SSC or vice versa.
- **XWB ADPCM ≠ standard IMA ADPCM**: Microsoft's XACT format uses a specific variant. Read the audio stream format from the wave bank entry header, don't assume.
- **WAVM is headerless**: the format has no magic bytes or metadata block; channels and sample rate are fixed by convention (2ch, 44.1 kHz). Detection is by extension and file-length modulo block size.
- **Note-type mapping across formats** (shocks, mines, freezes, rolls): each format encodes these differently. Mapping lives in `model/` with explicit fail-loud behavior when a source note type has no target representation.
- **Floating-point BPM round-trips**: SSQ stores tempo as fixed-point tied to TPS; SSC stores it as decimal strings. Going DDR → SM5 → DDR can drift BPMs. Use consistent rounding and document the precision expected.
- **TPS rescale drifts sub-millisecond**: modernizing TPS=75 to TPS=1000 (ratio 40/3) and TPS=150 to TPS=1000 (ratio 20/3) requires rounding non-multiples of 3. The rounded drift is well under human perception but not byte-exact.
- **Per-target sync bias is real**: the same modernized chart can be ~50 ms out of sync across different engines (Ultramix on DDR World is a known +53 ms offender). Expose the correction as an additive bias, not a hidden constant.
- **SM `#OFFSET` and DDR `tempo_data[0]` have opposite signs**: both say where beat 0 sits in the audio, but StepMania puts beat 0 at music time `−#OFFSET` while DDR puts it at `+tempo_data[0] / TPS`. The model uses the DDR convention (`Song::audio_sync_offset_seconds`); the SSC parser/writer are the only places that negate. Storing `#OFFSET` verbatim produces a `2 × |#OFFSET|` desync — hundreds of ms on ordinary simfiles — while DDR→SM5 looks fine because modern `tempo_data[0]` is bounded to ±22 ms.
- **The XWB entry header must be derived from the PCM, not assumed**: `ddr_wave_format(rate)` takes the decoded buffer's rate. An earlier version hardcoded 44.1 kHz, so a 48 kHz OGG played ~9% slow with progressive drift. The accepted set is closed (`DDR_SAMPLE_RATES` = 44.1 k / 48 k, 2 ch); anything else is refused rather than resampled or remixed silently, as `se_bank` already does.
- **Tempo synthesis must not accumulate exact rationals**: `#BPMS` values like `249.999985` (= 49999997/200000) put large primes into each segment's `Δbeats × 60000 / bpm` term; the running sum's denominator is the lcm of all of them and exceeds `u64` after a handful of such changes ("cumulative add: arithmetic overflow"). `ssq::writer` accumulates in `i128` fixed point (10⁻⁶ ms units) instead; positions and BPMs stay exact `Rational`s in the model.
- **SSQ has no BPM field, only slopes**: a segment's tempo is derived from the *next* tempo pair. The last `#BPMS` entry therefore needs a pair after it or it does not exist in the file — the symptom was charts running at the penultimate tempo (Mukade at 1280 BPM) all the way to the end. A BPM change and a stop at the same beat must also share one anchor, or a zero-length stop is encoded between them.
- **One SSQ step byte can mix taps and freezes**: the freeze block names *which* of a row's panels are held, so `1002` (tap Left, hold Right) is one step byte with a one-bit freeze entry. The model expresses this as two `Note`s at the same beat; the SSQ writer ORs same-tick rows and freeze-ends, the SSQ parser splits a row when a freeze-end covers only some of its panels, and the SSC parser never lets a hold head share a `Note` with a tap. Promoting a whole row on one freeze bit is the "hallucinated freeze arrows" bug.
- **SSQ cannot store a triplet exactly, so SSC output snaps to StepMania's row grid**: positions are whole measure ticks (1024/beat) and 1/3 beat is 341.33 ticks, so every 12th/24th note in an SSQ is already rounded (`341`/`682` from truncating authoring tools, `341`/`683` from this tool's round-to-nearest writer). Parsed exactly that is `341/1024` — on no `#NOTES` grid at all — and demanding an exact quantize fails the whole chart (`UnrepresentableBeat` on real Expert/Challenge charts). `ssc::notes` therefore rounds each beat to the nearest 1/48 (StepMania's own `ROWS_PER_BEAT`) *before* picking a measure's quantize: moves of ≤ 1 tick are tick rounding (`debug`), anything larger is genuinely off-grid input (one `warn` per chart). The model stays exact; only the writer rounds, the same way `ssq::writer` rounds beats to ticks going the other way.
- **The song code is the cue name is the filename**: XACT resolves cues by byte-exact `strcmp` against the song ID the game loaded the files by. Case matters; a 4-character truncation broke 5-character IDs. See business rule 11.
- **DDR World's chart clock is wall-clock, not sample-derived** (RE of `gamemdx_20260825.dll` `GamePlayActor::onUpdate` @ `18005cc70`): `musicCount = avsTickMs − musicStartTickMs + smallOffsets`. It never reads the audio position or the wave's sample rate, and `xactengine2_10.dll` (`FUN_0041d54e`) reads the rate from each entry's format field and resamples into a fixed 44.1 kHz DirectSound mix. So a 48 kHz bank stays in sync; the only cost is the engine's runtime SRC quality. Don't re-introduce a resampler on the assumption the game needs 44.1 kHz.

- **DDR World rounds every tempo anchor to whole milliseconds** (RE of `step::SsqReader`'s prepare routine, identical in the `gamemdx` 2025-08-05, 2026-08-25 and 2026-09-15 builds): `anchor_ms = floor(f32(int32(tempo_data × 1000)) / f32(tps) + 0.5f)`, and each TIMING note's `musicCount` (int32 ms) comes from these anchors. TPS 1000 anchors are therefore played exactly as written; for TPS 150/75 sources exact `tempo_data / tps` timing is up to ~0.5 ms per anchor away from what the game plays.
- **Auto-sync's ±60 ms cap exists because of half-beat aliasing**: DDR's catalogue is mostly 150–200 BPM with strong off-beat onsets and eighth-note-dense charts, so a chart off by half a beat (150–200 ms) looks as in sync as a correct one to any onset method. With a ±200 ms search 2.5% of stock songs locked onto the half-beat alias. Inside ±60 ms no such alias fits below ~214 BPM.
- **`sync::TARGET_OFFSET_MS` is specific to the onset detector**: it absorbs the detector's latency relative to a perceived attack. Any change to window, hop, band, emphasis, lag or attribution in `sync/` invalidates it — re-run `tests/auto_sync_calibration.rs` (see its header) and update the constant.
- **The community offset CSV's sign**: its value `c` is the amount to *add to `#OFFSET`* (equivalently, subtract from every `tempo_data`), because the modpack's generator negates the community tool's "move the chart this much later" value. An in-sync chart measures `T − c`, so `T = median(measured + c)`. Getting this backwards fits 15× worse and is easy to detect.

## Build, Test, Run

These commands are for developers working on the tool. End-user instructions go in README.md.

```bash
cargo build              # debug build
cargo build --release    # optimized binary for distribution
cargo run -- <args>      # run the CLI directly
cargo test               # run unit + integration tests
cargo clippy -- -D warnings  # lint (required to pass before merging)
cargo fmt                # format
```

## What Belongs in Steering vs. Design vs. Tasks

- **Steering** (this file): long-lived conventions, architectural principles, domain-level gotchas.
- **Design docs** (per-feature `design.md`): specific architectural decisions, crate picks with rationale, module layout for a feature.
- **Tasks** (per-feature `tasks.md`): implementation steps, exact code, tests to write.

If a decision is specific to one feature, it belongs in that feature's design doc, not here.
