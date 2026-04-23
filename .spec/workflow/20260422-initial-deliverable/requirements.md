# Requirements: 20260422-initial-deliverable

## Overview

This is the baseline deliverable for `ddr-chart-tools`. It makes the tool usable end-to-end for its two primary workflows — arcade ↔ StepMania 5 roundtrip, and DDR legacy SSQ modernization — on single files and on flat folders of files. After this feature ships, a user can install the CLI, point it at real DDR or SM5 content, and get back working converted content for the target platform.

The scope is strictly conversion. Parsing, format translation, audio recoding, and writing output files — nothing else. No GUI, no network, no editor features, no thumbnails/jackets, no recursive directory scan. **All audio work happens in-process** — no external CLI tools are invoked or required at runtime. The tool is distributed as a single self-contained binary.

## Glossary (additions to product.md)

- **WAVM** — A legacy DDR audio container used by the Ultramix generation (DDR Ultramix / Ultramix 2 / 3 / 4 on Xbox). Pre-DDR-World hardware generations may ship audio as WAVM in place of XWB. This tool reads WAVM as an input format; it never writes it.
- **DDR_LEGACY** — Any pre-DDR-World authoring of DDR content. Covers both:
  - Legacy-SSQ-with-WAVM-audio (Ultramix-era), and
  - Legacy-SSQ-with-XWB-audio (older arcade generations and any legacy mix that happens to use XWB).

  The tool distinguishes the audio container by its on-disk format, not by a separate flag. A single `--from-format DDR_LEGACY` invocation handles whichever of WAVM or XWB appears alongside the SSQ.

## User Stories

### US-1: Convert a single DDR song to StepMania 5

**As a** hobbyist who wants to play DDR arcade songs in StepMania 5
**I want** to run one command on an SSQ + audio pair and get back an SSC + OGG
**So that** I can load the song into SM5 immediately.

**Acceptance Criteria:**
- [ ] `ddr-chart-tools --from-format DDR --to-format SM5 --chartfile song.ssq --audiofile song.xwb` produces `song.ssc` and `song.ogg` in the same directory as the inputs.
- [ ] The output `.ssc` contains every chart found in the input `.ssq` (Single Basic/Difficult/Expert/Beginner/Challenge; Double Basic/Difficult/Expert/Beginner/Challenge — whichever slots the source has), mapped to SM5's canonical difficulty slots.
- [ ] BPM changes, stops, and freeze notes from the SSQ are represented in the SSC with gameplay-equivalent timing.
- [ ] The output `.ogg` is decodable by StepMania 5 (valid Ogg Vorbis stream, matching channel count and sample rate to the source audio).
- [ ] For `--from-format DDR` specifically, the accepted audio input format is XWB. WAVM is accepted as input when `--from-format DDR_LEGACY` is used (see US-3).
- [ ] Running the command on a file pair that the tool cannot parse exits non-zero, writes no output, and prints an error message that names the file and the parse location (byte offset for SSQ/XWB/WAVM; line/column for text formats).
- [ ] Running the command with `--to-format SM5` never writes a `.sm` file.

### US-2: Convert a single StepMania 5 song to DDR

**As a** hobbyist who wants to inject community SM5 songs into DDR
**I want** to run one command on an SSC (or SM) + OGG pair and get back an SSQ + XWB
**So that** the song is playable on DDR World.

**Acceptance Criteria:**
- [ ] `ddr-chart-tools --from-format SM5 --to-format DDR --chartfile song.ssc --audiofile song.ogg` produces `song.ssq` and `song.xwb` in the same directory as the inputs.
- [ ] The same command works when `--chartfile` is a `.sm` file; the tool accepts SM as an input-only dialect of SM5.
- [ ] The output `.ssq` is authored to the modern DDR profile: `TPS=1000` in the tempo chunk, only chunk types 1, 2, and 3 present, no auxiliary chunks (types 4, 5, 9, 17), no `param2 == 0xFFFF` on any chunk, correct terminator `00 00 00 00`.
- [ ] Charts from the SSC/SM are mapped onto DDR's valid difficulty codes (slot × style: 0x0114, 0x0214, 0x0314, 0x0414, 0x0614, 0x0118, 0x0218, 0x0318, 0x0418, 0x0618). SM5 `dance-single` maps to Single (0x14); `dance-double` maps to Double (0x18). Any other stepstype (`dance-solo`, `dance-couple`, `pump-*`, `routine`, etc.) is rejected with an error naming the unsupported stepstype.
- [ ] SM5 difficulty names (`Beginner`, `Easy`, `Medium`, `Hard`, `Challenge`) are mapped to DDR slots (0x04, 0x01, 0x02, 0x03, 0x06 respectively). `Edit` charts are skipped with a `warn` log naming the chart; the rest of the song is still converted.
- [ ] If two input charts would map to the same DDR difficulty code, the tool fails fast on that file with a clear error (naming both charts and the target slot). It does not silently drop or overwrite one.
- [ ] The output `.xwb` is playable by DDR World (valid XACT Wave Bank v43, MS-ADPCM encoded, sample rate and channel count match the OGG source or are explicitly resampled).
- [ ] BPM changes, stops, and freeze/hold notes from the SSC/SM are represented in the SSQ with gameplay-equivalent timing, respecting the 4096-ticks-per-measure convention and `TPS=1000`.

### US-3: Modernize a legacy DDR SSQ to current DDR

**As a** hobbyist with pre-DDR-World SSQs (Ultramix, earlier arcade mixes)
**I want** to convert them to the modern SSQ profile
**So that** they load correctly in current DDR.

**Acceptance Criteria:**
- [ ] `ddr-chart-tools --from-format DDR_LEGACY --to-format DDR --chartfile song.ssq --audiofile song.{xwb|wavm}` produces a new `song.ssq` and a new `song.xwb` in the same directory.
- [ ] The tool accepts XWB or WAVM for `--audiofile` when `--from-format DDR_LEGACY`. The audio container is detected from the file's magic/header, not the file extension. Extension mismatches (e.g. a WAVM file named `.xwb`) are allowed so long as the header parses; extension is a hint only.
- [ ] The output `.ssq` is authored to the modern DDR profile (same rules as US-2: TPS=1000, chunk types 1/2/3 only, no sentinels, correct terminator).
- [ ] The output `.xwb` is authored to a format DDR World reads (XACT Wave Bank v43, MS-ADPCM). If the input audio was XWB already, the tool **may** byte-copy the input when the container matches modern DDR's expectations; if it cannot confirm the input is already in the expected shape, it decodes to PCM and re-encodes. If the input was WAVM, it always decodes (WAVM → PCM) and then encodes (PCM → XWB).
- [ ] All legacy-only SSQ chunks (types 4, 5, 9, 17) in the source are dropped and each drop is logged at `warn` level, naming the chunk type, its size, and the source file.
- [ ] Timing is preserved: tick values scaled from the source TPS (150 in practice for legacy) to `TPS=1000` so wall-clock BPM, stops, and step times are unchanged. Rounding rules are documented; any cumulative drift greater than one tick at `TPS=1000` per tempo segment is a bug.
- [ ] Freeze-block semantics are preserved in meaning (same panels, same freeze vs. shock disposition), even though the tick offsets are rescaled.

### US-4: Modernize a legacy DDR SSQ to StepMania 5

**As a** hobbyist with pre-DDR-World SSQs
**I want** to convert them directly to SSC + OGG
**So that** I can play them in SM5 without first modernizing them to current DDR.

**Acceptance Criteria:**
- [ ] `ddr-chart-tools --from-format DDR_LEGACY --to-format SM5 --chartfile song.ssq --audiofile song.{xwb|wavm}` produces `song.ssc` and `song.ogg` in the same directory.
- [ ] Accepts XWB or WAVM audio input on the same rules as US-3 (header-based detection).
- [ ] The output matches what would be produced by chaining `DDR_LEGACY → DDR → SM5`, but is done in one command. The intermediate modern SSQ and intermediate XWB are not written to disk.
- [ ] Dropped legacy-only chunks are logged identically to US-3.

### US-5: Batch conversion on a flat input folder

**As a** hobbyist with many songs
**I want** to point the tool at a folder and have it convert every eligible file pair
**So that** I don't have to script the single-file command.

**Acceptance Criteria:**
- [ ] `ddr-chart-tools --from-format {FROM} --to-format {TO} --input-folder DIR` scans the **top level only** of `DIR` for chart+audio file pairs matching the source format's expectations and converts each pair.
  - For `DDR`: SSQ + XWB.
  - For `DDR_LEGACY`: SSQ + (XWB or WAVM — audio container detected by header, not extension).
  - For `SM5`: (SSC or SM) + OGG.
- [ ] Pairing is by shared basename without extension: `foo.ssq` + `foo.xwb` is one pair; `foo.ssq` with no companion audio file is unpaired and skipped with a warning that names the missing side.
- [ ] Unpaired audio files (audio without a matching chart) are also skipped with a warning.
- [ ] When multiple audio files with the same basename exist (e.g. `foo.xwb` and `foo.wavm` both present for `foo.ssq` in DDR_LEGACY mode), the tool fails for that file with a clear error naming the ambiguity. See Open Question #9.
- [ ] Files with the wrong extensions for the declared `--from-format` are ignored silently (e.g. a stray `readme.txt` in the folder).
- [ ] A failure on one file (parse error, write error, unsupported chart stepstype, etc.) is logged at `error` level and the batch continues with the next file.
- [ ] Exit code: `0` if all files succeeded, `1` if any per-file failure occurred, `2` if the run never started due to a CLI or argument error.
- [ ] At end of run, a summary line prints total counts: attempted, succeeded, failed, skipped.
- [ ] Subdirectories under `DIR` are not scanned. Finding a subdirectory is not an error; it is ignored.
- [ ] Output files are colocated with their inputs.
- [ ] If an output file already exists, the run fails for that file with a clear error unless `--overwrite` is supplied; when `--overwrite` is supplied, existing files are silently replaced.

### US-6: CLI discoverability and safety

**As a** new user
**I want** clear `--help`, unambiguous flag grammar, and safe defaults
**So that** I can figure out what the tool does and not damage my files by accident.

**Acceptance Criteria:**
- [ ] `ddr-chart-tools --help` lists every flag with a short description and shows at least one example for each of: single DDR→SM5, single SM5→DDR, legacy→DDR (with both XWB and WAVM audio examples), batch.
- [ ] `--from-format` and `--to-format` are required.
- [ ] Exactly one of `{--chartfile + --audiofile}` and `{--input-folder}` must be supplied.
- [ ] `--chartfile` requires `--audiofile`, and vice versa.
- [ ] `--to-format DDR_LEGACY` is rejected at argument-parsing time with an error stating that legacy output is not supported.
- [ ] Unsupported `(from, to)` combinations beyond the matrix in product.md are rejected at argument-parsing time naming both formats.
- [ ] `--overwrite` (no value) enables silent overwriting of existing output files. Default is to fail when an output path exists.
- [ ] `--version` prints the tool's version.
- [ ] `-v` raises log level to debug, `-vv` raises to trace.
- [ ] `--quiet` suppresses `info`-level output but keeps `warn` and `error` visible. Legacy-data-drop warnings, skipped `Edit` charts, and per-file failures are still shown under `--quiet`. (`-q -q` to suppress warnings is deferred and not part of this deliverable.)
- [ ] The binary is named `ddr-chart-tools`.

### US-7: Round-trip fidelity (verification story)

**As a** maintainer
**I want** the tool's output to round-trip cleanly for a canonical test corpus
**So that** regressions are caught automatically.

**Acceptance Criteria:**
- [ ] An integration test exists for each supported `(from, to)` pair (DDR→SM5, SM5→DDR, DDR_LEGACY→DDR, DDR_LEGACY→SM5) using at least one checked-in fixture per pair.
- [ ] The `DDR_LEGACY→{DDR,SM5}` tests include at least one WAVM-audio fixture and one XWB-audio fixture, so both legacy audio decode paths are exercised.
- [ ] For DDR→SM5→DDR, the note timing of every chart after the round trip matches the original within the precision limits documented in tech.md. Round-trip drift must not introduce any note misplacement at gameplay resolution.
- [ ] For DDR_LEGACY→DDR, the resulting SSQ passes a "modern profile" validator: TPS=1000, only chunk types 1/2/3, no sentinel values, correct terminator.
- [ ] Tests run under `cargo test` with no environmental setup beyond a stable Rust toolchain. **No external CLI tools are required to run tests** (no `vgmstream`, no DirectX SDK, no `oggenc`).

### US-8: No external runtime dependencies

**As a** user on macOS, Linux, or Windows
**I want** the tool to work out-of-the-box after a single `cargo install` (or a prebuilt binary download)
**So that** I don't have to install and manage `vgmstream`, DirectX SDKs, FFmpeg, `oggenc`, or any other external helper.

**Acceptance Criteria:**
- [ ] All audio decoding (XWB MS-ADPCM, WAVM, OGG) happens in-process, via Rust crates or first-party code.
- [ ] All audio encoding (XWB MS-ADPCM, OGG Vorbis) happens in-process, via Rust crates or first-party code.
- [ ] The shipped binary runs on macOS, Linux, and Windows without invoking any subprocess for format work. (Spawning system processes for unrelated reasons — e.g. a future feature like opening a file explorer — is not prohibited, but no such feature is in this deliverable.)
- [ ] `cargo build --release` produces a working binary without requiring any tool other than the Rust toolchain itself.
- [ ] If a design-phase investigation concludes that in-process encoding of a specific format (most likely XWB MS-ADPCM encode) is not achievable in pure Rust within the project's timeline, that becomes an escalation to the user — not a silent switch to a shell-out. See Open Question #10.

## Out of Scope

- Thumbnails, jackets, banners, video backgrounds, preview clips, keysounds, lyrics.
- Recursive directory scanning — only the top level of `--input-folder` is considered.
- Emitting legacy-format SSQs (`--to-format DDR_LEGACY`).
- Emitting SM (we only write SSC).
- Emitting WAVM (input-only).
- GUI, interactive prompts, or TUI progress bars beyond simple line-based logging.
- Network access of any kind.
- Chart authoring, editing, difficulty balancing, or generation.
- Parallel batch processing.
- Handling DDR generations other than DDR World for output.
- Legacy audio formats other than XWB and WAVM (other pre-World formats can be added later; out of scope for this deliverable).
- Converting audio separately from charts.
- Handling files outside the top level of the input folder in batch mode, or per-file output-directory overrides.
- Configuration files, environment-variable configuration, profiles.
- Windows-specific packaging, code signing, or installer generation.
- Validating that the output actually loads on real DDR / SM5 hardware as part of CI.
- Shelling out to external CLI tools (`vgmstream`, DirectX SDK, FFmpeg, `oggenc`, etc.) at runtime or test time.

## Open Questions

All open questions from earlier revisions are now resolved. Captured here for audit trail.

1. **CLI grammar.** Resolved: flag-driven with `--from-format`/`--to-format` as primary, `clap` conditional-requirement handling for the rest. No subcommands.

2. **Output collision.** Resolved: fail with a clear error unless `--overwrite` is supplied; `--overwrite` silently replaces existing files.

3. **`--quiet` semantics.** Resolved: suppress `info` but keep `warn` and `error`. `-q -q` to suppress warnings is deferred and out of scope.

4. **DDR_LEGACY audio handling.** Resolved: accept XWB and WAVM as inputs; detect by header; never write WAVM; all audio conversion is in-process with no external CLI tools.

5. **Batch exit codes.** Resolved: `0` all success / `1` any per-file failure / `2` CLI/setup error.

6. **Basename collision warning in help.** Resolved: called out in `--help`.

7. **SM5 stepstypes.** Resolved: accept only `dance-single` (maps to DDR 4-panel Single) and `dance-double` (maps to DDR 8-panel Double); reject all others.

8. **SM5 `Edit` charts.** Resolved: skip with a warning; continue converting the rest of the song.

9. **Ambiguous audio in DDR_LEGACY batch mode.** Resolved: fail loudly for that file. When a folder contains both `foo.xwb` and `foo.wavm` alongside `foo.ssq`, the tool errors on that file, names the ambiguity, and moves on to the next.

10. **In-process XWB MS-ADPCM encoding viability.** Resolved (for requirements; concrete approach still design-phase work). Constraint: no external CLI tools. The design phase will research pure-Rust MS-ADPCM encoder crates first. If none are suitable, fallback options in priority order: (a) first-party MS-ADPCM encoder in Rust against the Microsoft spec, (b) Rust FFI to a small vendored C encoder statically linked into the binary, (c) Ghidra-assisted reverse-engineering of the August 2007 DirectX SDK's XACT authoring CLI (`xactbld` / `xwbtool` family) if public documentation proves insufficient. The user has offered to load the DirectX SDK CLI tools into Ghidra as a reference if needed.

11. **WAVM format references.** Resolved: use `~/Desktop/vgmstream` as the format reference. Key files:
    - `src/meta/raw_wavm.c` — the WAVM entry point. Headerless format. Hardcoded assumptions: 2 channels, 44100 Hz, coding = `XBOX_IMA` ADPCM, no loop. The whole file is raw XBOX-IMA samples.
    - `src/coding/xbox_ima_decoder.c` (and siblings under `src/coding/`) — XBOX-IMA ADPCM decoder implementation.
    Because WAVM is effectively "raw XBOX-IMA with fixed parameters," the Rust-side decoder needed is narrow: one ADPCM variant, no container parsing. Design phase will either pick a Rust crate that supports XBOX-IMA ADPCM or port the ~100 LOC of `xbox_ima_decoder.c` directly.

## Dependencies

- **docs/ssq_format.md** (in-repo) — authoritative SSQ spec.
- **StepManiaPaX Python codebase** (`~/Desktop/Projects/StepmaniPaX/python`) — reference for XWB parsing, MS-ADPCM decode, OGG emission.
- **StepMania 5 source** (`~/Desktop/stepmania`) — reference for SSC and SM formats (`src/NotesLoaderSSC.cpp`, `src/NotesLoaderSM.cpp`, `src/NotesWriterSSC.cpp`).
- **vgmstream** (`~/Desktop/vgmstream`, local) — reference for WAVM format and XBOX-IMA ADPCM decoding. Specifically `src/meta/raw_wavm.c` and `src/coding/xbox_ima_decoder.c`. Used as a code-level reference only; not linked or shelled-out to.
- **DirectX SDK CLI tools (August 2007 release)** — available to the user, usable as a last-resort reference for the XWB MS-ADPCM encoder via Ghidra disassembly. Not a runtime dependency.
- **A stable Rust toolchain** (installable via `rustup`).

## Assumptions

- User is on macOS or Linux primarily. Windows is best-effort.
- Input files are trustworthy DDR / SM5 content; the tool just reports parse errors cleanly when inputs are malformed.
- A "song" is one chart file + one audio file. Multi-track audio (multiple songs in one XWB) is out of scope; we assume one audio track per file.
- SSC/SM files describe one song each.
- Audio conversion can be end-to-end in-memory for typical song lengths (~15 minutes of stereo 44.1 kHz).
- "Gameplay-equivalent timing" is the correctness bar; exact bit-for-bit round-trip is not required.
- All audio encoding and decoding is done in-process — the user does not need `vgmstream`, FFmpeg, DirectX SDK, or `oggenc` installed.
- WAVM as used by DDR Ultramix (Xbox) is the specific WAVM variant supported. Other WAVM dialects (if any) are not a concern for this deliverable.

## Notes for Principal Engineer

- `docs/ssq_format.md` §6–§9 cover chunk types 4/5/9/17 — legacy-only, always dropped on modernization.
- Section 5.1 defines the exact 10 valid `(slot, style)` combinations DDR World accepts; SM5 → DDR mapping targets exactly these.
- `docs/ssq_format.md` §3 explains the per-file TPS. Legacy = 150, modern = 1000; all tick-valued fields rescale.
- StepManiaPaX's XWB parser (`stepmanipax/xwb/parser.py`), MS-ADPCM decoder (`stepmanipax/xwb/adpcm_decoder.py`), and audio converter (`stepmanipax/xwb/audio_converter.py`) are the closest decode reference for XWB.
- **WAVM decoding** — `~/Desktop/vgmstream/src/meta/raw_wavm.c` shows WAVM is a headerless format with fixed 2ch/44100Hz/XBOX-IMA assumptions. The actual decode is in `~/Desktop/vgmstream/src/coding/xbox_ima_decoder.c`. If any real DDR WAVM file deviates from the fixed assumptions, that's a signal worth flagging; the requirements do not currently cover variable-parameter WAVM.
- **No reference exists for OGG → XWB (MS-ADPCM encode).** This is the single biggest unknown. Research pure-Rust crates first; if none suffice, implement from the Microsoft spec or use Ghidra on the DirectX SDK (August 2007) XACT authoring tools the user can provide. No runtime shelling-out.
- Per US-8, no external CLI tool may be invoked at runtime or test time. If pure-Rust crates are insufficient, Rust FFI to a vendored/statically-linked C library is acceptable; shelling out is not.
