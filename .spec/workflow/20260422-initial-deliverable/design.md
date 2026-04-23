# Design: 20260422-initial-deliverable

**Requirements**: [requirements.md](requirements.md)
**Parent SIM**: none (solo hobby project)

---

## Overview

A single Rust binary crate that converts DDR arcade content (SSQ+XWB+XSB) and StepMania 5 content (SSC/SM+OGG) in the four supported directions, on single files and flat folders. Everything is in-process: no subprocess invocation at runtime or test time. All parsing, format translation, audio decoding, and audio encoding happens in pure Rust (with one vendored C dependency permitted if MS-ADPCM encoding proves impractical in pure Rust — see Decision 5).

This design took an up-front reverse-engineering pass on `XactBld.exe` and inspected a real DDR World XWB/XSB pair to remove the biggest unknown in the requirements (Open Question #10: how to produce DDR-compatible audio without shelling out). That pass produced concrete byte-level targets instead of "we'll figure it out in implementation."

---

## Reverse-engineering findings (de-risk summary)

Captured here because they underpin Decisions 5, 6, 7, and 8.

**From `XactBld.exe` decompilation (Ghidra) + ssqparse's known-working XAP template + diffing all 12 XSBs and inspecting a real DDR World XWB/XSB pair (`fizz.*`):**

| Question | Finding | Source |
|---|---|---|
| XWB version | v43 ("Content Version = 43"); magic `WBND`; 5-segment layout | `fizz.xwb` header bytes 0x00-0x33 |
| Codec | MS-ADPCM (`PC Format Tag = 2` = `WAVE_FORMAT_ADPCM`) | `XactBld.exe FUN_01044bbc` + ssqparse XAP |
| Samples per block | 128 (hardcoded default in XactBld ADPCM ctor; also explicit in ssqparse XAP) | `XactBld.exe FUN_01044bbc` sets `*((int)this + 0x18) = 0x80` |
| Audio format | 2 channels, 44100 Hz, 16-bit source | ssqparse XAP `Cache` block + `fizz.xwb` entry metadata |
| XSB required? | **Yes** — DDR World loads both XWB and XSB. | `~/Desktop/DDR WORLD/.../dance/*.xsb` present for every song except two edge cases (`souv`, `haph`) |
| XWB entries per file | 2 — main song (`<name>`) and short preview (`<name>_s`) | ssqparse XAP + `fizz.xwb` entry names |
| Seek tables | Flag `SEEKTABLES` is set in header but segment length is 0 (ADPCM doesn't need them) | `fizz.xwb` segment[2].length = 0 |
| Wave data alignment | 2048 (0x800) byte boundary inside the XWB | `fizz.xwb` segment[4].offset = 0x800 |
| XSB variable regions | 5 regions, fully enumerated. 4 fixed-width 4-byte name fields at 0x4a / 0x8a / 0x13a / 0x13f plus an 8-byte zeroable timestamp at 0x08. Mystery u16/u32 sentinel fields scattered in 0x10e-0x12d are copied verbatim from `fizz.xsb` (since fizz works in DDR, its pattern is known-good). | Diff across `fizz.xsb` / `somd.xsb` / `vill.xsb` (3 same-structure samples) |
| XSB internal names vs filesystem names | Decoupled — `homs2.xsb` stores "horms2" in its cue-name field (not "homs2"), implying DDR requests cues by index (0/1), not by name string. Internal name fields are cosmetic. | Direct byte inspection of `homs2.xsb` at offset 0x4a |

**What this means architecturally**: every byte of the audio pipeline is accounted for. No runtime unknowns; no "we'll see when we get there." The MS-ADPCM encoder implementation is the one place risk remains — Decision 5 lays out the explicit fallback chain for that.

---

## Architecture Decisions

### Decision 1: Common-model pipeline, not point-to-point conversion

**Problem**: Four supported `(from, to)` pairs today, plus room to grow. A direct `ssq_to_ssc` / `ssc_to_ssq` / `legacy_ssq_to_ssc` / `legacy_ssq_to_ssq` matrix is O(n²) and hides the semantic gaps between formats.

**Decision**: Every conversion goes `source format → common model → target format`. The model layer owns the format-independent representation of a song (charts, tempo changes, stops, freezes, shocks, audio samples).

**Rationale**:
- Forces every concept that crosses a format boundary to be named once.
- Adding a new format later means writing one parser and one writer, not 2n point-to-point conversions.
- Matches how StepMania itself is structured (`Song`, `Steps`, `NoteData`) and how ssqparse works internally.

**Alternatives considered**:
- Point-to-point: rejected, see above.
- Format-to-format via an intermediate textual IR: rejected, adds serialization overhead for no win on a 4-format tool.

**Tradeoffs**: A tiny bit of "represent this in the model even though both endpoints understand it natively" ceremony (e.g. SSC `#STOPS:` and SSQ tempo-chunk zero-delta stops both express stops; the model has a single `Stop` type). Worth it for the structural clarity.

### Decision 2: DDR_LEGACY as an input-only parser, reusing the modern SSQ writer

**Problem**: Legacy SSQs (TPS=150, chunk types 4/5/9/17 allowed) and modern SSQs (TPS=1000, types 1/2/3 only) share 90% of the format. Splitting them into two completely separate modules would duplicate parsing for chunk types 1/2/3.

**Decision**: One `ssq/` module owns parsing for both legacy and modern SSQs (types 1, 2, 3, and auxiliary 4/5/9/17). A separate `ssq_legacy/` module owns the **modernization transform** (legacy-parsed SSQ → modern-profile SSQ), which is a pure model-to-model rescale-and-filter operation. Only the `ssq/` module writes SSQs, and it always writes the modern profile (TPS=1000, chunks 1/2/3 only). US-requirement that `--to-format DDR_LEGACY` is rejected at CLI time is enforced in the CLI layer; the writer physically cannot emit a legacy SSQ.

**Rationale**:
- Single parser for the common structure.
- Modernization (tick rescaling, dropping auxiliary chunks) is a distinct logical step worth calling out as its own module.
- Writer can never accidentally produce a legacy-format file — not a matter of discipline, a matter of code topology.

**Alternatives considered**:
- Two parallel modules `ssq_modern/` and `ssq_legacy/`: rejected, duplicates chunk-type 1/2/3 parsing.
- Modernization inside the writer: rejected, conflates "convert tick rate" with "emit bytes."

**Tradeoffs**: The `ssq/` parser accepts files that the `ssq/` writer cannot produce (legacy files with TPS=150 and auxiliary chunks). This is fine — readers are always more permissive than writers, and the type system enforces that auxiliary chunks don't reach the writer (they're dropped during modernization and never appear in the model).

### Decision 3: Audio is part of the model, not a separate pipeline

**Problem**: Every conversion moves audio alongside charts. We could make audio a parallel pipeline with its own plumbing, or fold it into the same parse-model-write flow.

**Decision**: The common-model `Song` owns its audio as a PCM buffer (samples + sample rate + channel count). Parsing an `(SSQ, XWB, XSB)` triple decodes the XWB into PCM and attaches it to the model. Writing an `(SSQ, XWB, XSB)` triple encodes the model's PCM to MS-ADPCM and wraps it in an XWB+XSB pair. Same flow as charts.

**Byte-copy passthrough for `DDR_LEGACY → DDR`**: When the source format is `DDR_LEGACY`, the source audio is XWB (not WAVM), AND a matching XSB exists alongside it, AND both pass a compliance check (XWB is v43 with MS-ADPCM 128-samples-per-block 2ch/44100Hz 2-entry layout; XSB byte-matches the known template except in the identified name-field regions), byte-copy both input files to the output paths unchanged. Any other `DDR_LEGACY → DDR` case (WAVM input, missing XSB, or non-compliant XWB/XSB) decodes and re-encodes.

**Rationale**:
- One pipeline. The CLI/job layer doesn't need separate branches for chart vs audio handling.
- Single clear point for the audio-sync-offset (SSQ tempo_data[0]) to live: on the model, not duplicated across parsers.
- Passthrough is an obvious win for legacy content that's already modern-compliant (no quality loss from re-encoding, no risk of our encoder diverging from XactBld's output).
- Passthrough is strictly safe: the compliance check verifies the source files are already what we would generate, so byte-copy produces bit-identical output to the re-encode path.

**Alternatives considered**:
- Always re-encode: rejected, wastes work and risks quality loss when source is already compliant.
- Unconditional byte-copy when source is XWB: rejected, fails if the source XWB uses a different layout DDR doesn't accept.
- Separate audio pipeline: rejected, O(n²) conversion paths for audio too.

**Tradeoffs**: The compliance check adds a read+parse of the XWB header before deciding to copy. Negligible cost. Passthrough is narrow (legacy→modern DDR only, XWB+XSB present, all checks pass) by design — elsewhere the decode/re-encode path is always used for uniformity.

### Decision 4: `clap` derive with a validated `Cli` struct and a `Job` enum downstream

**Problem**: The CLI has interlocking conditional requirements (`--chartfile` requires `--audiofile`, exactly one of `{files}` or `{input-folder}`, `--to-format DDR_LEGACY` forbidden, etc.). Spreading these across the enum risks inconsistency.

**Decision**:
- `Cli` struct in `src/cli/mod.rs` uses `clap` derive with `ArgGroup` and `required_if_eq` for shape-level constraints.
- A single `Cli::validate()` method enforces semantic rules (forbidden format combos, `DDR_LEGACY` output rejection).
- `Cli::into_jobs()` turns the parsed/validated CLI into a `Vec<Job>` where `Job` is `{source_paths, target_paths, from_format, to_format}`. Downstream code sees a `Job`, not a `Cli`.

**Rationale**: Keeps clap concerns at the edge; everything below it sees a clean job list. Tests hit `into_jobs` directly without constructing `Cli` from argv. Matches the `rust-cli-standards.md` steering guidance.

**Alternatives considered**:
- Subcommands (`ddr-chart-tools ddr-to-sm5 ...`): rejected by Open Question #1 in requirements (user wants flag-driven).
- Validation scattered through enum impls: rejected, harder to audit for completeness.

**Tradeoffs**: Clap's derive API doesn't express every constraint we need; a few rules live in the `validate()` method rather than being visible at the type level. Acceptable.

### Decision 5: MS-ADPCM encoding — pure-Rust first-party, vendored C as fallback

**Problem**: No off-the-shelf pure-Rust MS-ADPCM encoder is known to exist as a maintained crate at time of writing. (Pure-Rust decoders exist — `hound` can read them, and ffmpeg-rs can, but encoding is rarer.) US-8 forbids runtime shell-out.

**Decision**: Implement a first-party pure-Rust MS-ADPCM encoder in `src/xwb/adpcm/encode.rs`, against the public Microsoft WAVEFORMATEX spec, using the confirmed XactBld parameters (128 samples per block, 2 channels, 44100 Hz). If implementation reveals a blocker (DDR rejects our encoder's output, or the algorithm proves beyond pure-Rust reach in project timeline), fall back to **(b) vendored C encoder, statically linked** via `cc` + `bindgen`. Do **not** invoke external binaries.

**Rationale**:
- MS-ADPCM encode is well-documented: per-block, choose one of 7 standard predictor coefficient sets, quantize 4-bit deltas with an adaptive step table. Reference implementations exist in ffmpeg/libsndfile/sox, from which the algorithm can be re-derived in Rust.
- XactBld's output is not bit-exact-reproducible without full RE of its predictor-search heuristic, but DDR World doesn't require bit-exactness — it just needs a standards-compliant MS-ADPCM stream wrapped in XWB v43. XactBld's output is one valid such stream; ours will be another.
- Verification approach: encode test PCM, decode with a Rust MS-ADPCM decoder (`hound` or first-party), compare to the original PCM under MS-ADPCM's known quantization error (~60 dB SNR). Then wrap in XWB and open in vgmstream as an offline sanity check during development (vgmstream is a reference, not a runtime dep).
- If pure-Rust hits a wall: ffmpeg's `adpcmenc.c` MS-ADPCM encoder is ~400 lines of portable C, liberally licensed (LGPL — acceptable for static link in a GPL/MIT dual-licensed hobby project, or we pick a more permissive reference). Vendor it, statically link, ship as part of the Rust binary. Still satisfies US-8.

**Alternatives considered**:
- (c) Ghidra-assisted bit-exact port of XactBld: rejected as up-front work; kept as a last-resort reference oracle. The RE pass already extracted the parameters we need; going deeper is unnecessary.
- `ffmpeg-next` crate: rejected, too heavy (pulls in all of libavcodec).

**Tradeoffs**: Most of the encoder risk is now in implementation, not design. The pure-Rust path could be 200 LOC (straightforward) or 500 LOC (if predictor search gets subtle); the C fallback adds build complexity but removes the unknown.

### Decision 6: XWB container — first-party Rust writer, byte-level verified against a DDR sample

**Problem**: XWB v43 is a documented container, but the exact packing of `WAVEBANKMINIWAVEFORMAT` (a 32-bit bitfield encoding codec/channels/rate/blockalign) is order-sensitive and the public XACT headers show it differently across sources.

**Decision**: Write a first-party XWB v43 parser and writer in `src/xwb/container/`. Verification strategy: the DDR World sample files in `~/Desktop/DDR WORLD/MDX-003_20260324/contents/data/sound/win/dance/*.xwb` are the ground truth. For each sample, our parser's output must round-trip (parse → write → byte-identical). This catches any bit-packing mistake immediately.

**Rationale**:
- XWB is maybe 150 lines of structure definition plus 100 lines of write logic. Not worth a dependency.
- Round-tripping real DDR XWBs is a strong correctness signal: anything our writer gets wrong, the comparison test will catch.
- The 2-entry-per-XWB convention (main + `_s` preview) is explicit in the design; we encode both.

**Alternatives considered**:
- Depend on a crate: no maintained Rust XWB writer exists.
- Copy from StepManiaPaX's Python parser: reference only (read-side), not a source of code.

**Tradeoffs**: Writing a container format from scratch always has a few byte-level gotchas. The round-trip-against-real-files test takes those gotchas from "surprise production bug" to "first-day-of-implementation known-issue list."

### Decision 7: XSB generation — static template with song-name substitution

**Problem**: Sound Bank (XSB) is a XACT structure describing cues, sounds, tracks, variations. DDR's XSBs are remarkably uniform — mostly ~326 bytes, cue structure identical across songs, only a few regions differ.

**Design-phase RE of DDR World's XSB corpus** (12 samples in `data/sound/win/dance/*.xsb`): restricting to same-structure files (4-char song code, "main-first" name-table ordering, 326 bytes — `fizz.xsb`, `somd.xsb`, `vill.xsb`), only these regions vary between songs:

| Offset | Length | What it is | Handling |
|---|---|---|---|
| 0x08-0x0f | 8 bytes | `Last Modified` timestamp or content hash | Write zeros (the ssqparse XAP template specifies 0 here, and DDR appears not to validate) |
| 0x4a-0x4d | 4 bytes | Cue name (fixed-width, null-padded ASCII) | Patch with the 4-char song code |
| 0x8a-0x8d | 4 bytes | Wave bank name (fixed-width, null-padded ASCII) | Patch with the 4-char song code |
| 0x13a-0x13d | 4 bytes | Trailing name-table entry 1 (main name) | Patch with the 4-char song code |
| 0x13f-0x142 | 4 bytes | Trailing name-table entry 2 (preview name prefix before `_s\0`) | Patch with the 4-char song code |
| 0x10e, 0x122, 0x126-0x129, 0x12c-0x12d | scattered u16/u32s | XACT-internal sentinel/variation fields that vary arbitrarily between songs (both `0x0000`, `0x0001`, and `0xFFFF` appear across the corpus) | Copy `fizz.xsb`'s values verbatim — since `fizz.xsb` works in DDR, its pattern is known-good |

All other bytes are byte-identical across the 4-char / main-first / 326-byte group.

**Evidence that internal name fields are cosmetic**: `homs2.xsb` (a DDR-shipped file) stores "horms2" — not "homs2" — in its cue-name field (0x4a region). The filesystem name and the internal cue name do not have to match, which strongly implies DDR's game engine requests cues by index (0 for main, 1 for preview) rather than by name string. The name field is essentially metadata for debugging/XACT tools.

**Decision**: Ship a single binary XSB template (extracted from `fizz.xsb`, with the song-name regions zeroed). At generation time, load the template into a buffer, zero out the 8-byte timestamp region, patch the 4 fixed-width 4-byte name-field offsets with the song's 4-char code, write to disk.

**Song-code constraint for `SM5 → DDR` conversion**: The internal name fields are fixed-width 4 bytes. For the initial deliverable, the song code must be exactly 4 ASCII characters. If the output filename's basename is longer than 4 characters, the first 4 ASCII-alphanumeric characters are used (e.g. `mycustomsong.ssc` → internal code `myco`). The output files are still named `mycustomsong.xwb` / `mycustomsong.xsb` on disk — only the internal XSB/XWB fields use the 4-char code. If the basename has fewer than 4 alphanumeric characters, padding with `_` is used. This matches how ssqparse (the known-working reference) handles names.

**Variable-length name support is out of scope** for this deliverable. DDR itself ships songs with 5-char codes (`bknh2`, `homs2`) using subtly different XSB layouts, but the initial release doesn't need to generate those. Adding full variable-length support would require writing a real XSB compiler; defer to a later feature if needed.

**Rationale**:
- The template approach survives the RE pass cleanly: only 5 small variable regions, all trivially handled.
- Matches what ssqparse does (via XactBld + XAP template); we're cutting out the XactBld middleware and producing the bytes directly.
- Byte-level verifiable: output for song code "fizz" must be byte-identical to `fizz.xsb` after we extract the template correctly. This is the first test to write.
- Constraint on 4-char codes is acceptable — DDR's own filesystem convention overwhelmingly uses 4-char codes, and it's easy to explain in CLI help.

**Alternatives considered**:
- Write a real XSB compiler: rejected, massive overkill for one supported structural shape.
- Invoke XactBld at runtime: rejected by US-8.
- Support variable-length names via offset patching: rejected for scope — the XSB has internal pointers referencing the name table that would need to shift when name length changes. Complex enough to defer.
- Accept any-length codes by truncating silently: rejected — if the user's filename is `song.ssc`, silently writing "song" into the XSB without telling them breaks the principle of least surprise. Either the CLI validates up front, or we hash-derive a 4-char code from the full basename (deterministic, lossy but explicit).

**Tradeoffs**: If DDR ever starts using a new XSB layout (different cue structure, different field sizes), the template breaks. Acceptable — the 12 DDR World samples in hand today are all compatible. Regenerating the template from a newer sample is a small manual task.

### Decision 8: WAVM decoding — pure-Rust, direct port of vgmstream's XBOX-IMA decoder

**Problem**: WAVM is a headerless format with fixed parameters (2 channels, 44100 Hz, XBOX-IMA ADPCM). No Rust crate handles it.

**Decision**: Implement WAVM as a thin wrapper around an XBOX-IMA ADPCM decoder. The decoder itself is ported from `~/Desktop/vgmstream/src/coding/ima_decoder.c` (specifically the `xbox_ima_decode_*` functions) — roughly 100 LOC of Rust. Live in `src/wavm/` with the decoder in `src/wavm/xbox_ima.rs`.

**Rationale**:
- Headerless format means "no container parsing needed," just "treat every byte as interleaved ADPCM."
- XBOX-IMA is a well-defined variant of IMA ADPCM. Direct port of 100 lines of C is straightforward.
- Fixed parameters mean the decoder has no configurability surface.

**Alternatives considered**:
- Generic IMA ADPCM crate: XBOX-IMA differs from standard IMA in block layout and sample interleaving; generic decoders don't produce correct output.
- FFI to vgmstream: rejected by US-8's spirit (we'd be linking a full audio library for 100 lines of codec).

**Tradeoffs**: If a real DDR WAVM file deviates from vgmstream's fixed-parameter assumption (e.g. mono, different rate), we fail with a clear error and add variable-parameter support later. Acceptable per the Open Questions resolution (#11) in requirements.

### Decision 9: OGG Vorbis I/O — `lewton` for decode, `vorbis_rs` (static libvorbis) for encode

**Problem**: We need in-process OGG Vorbis decode (for SM5 → DDR input) and encode (for DDR → SM5 output). US-8 forbids subprocess invocation at runtime.

**Decision**: Use `lewton` (pure-Rust Vorbis decoder) for decode. For encode, use `vorbis_rs` (Rust bindings to libvorbis) which compiles libvorbis from source via `cc` and statically links it into the final binary. No external libraries at runtime; `cargo install --path .` still succeeds with only the Rust toolchain (plus a C compiler, which Cargo already requires on all target platforms).

**Why not pure-Rust encode**: No production-quality pure-Rust Vorbis encoder exists at time of writing. `lewton` is decode-only. `symphonia` doesn't do Vorbis encode. Writing one from scratch is a multi-week signal-processing project involving MDCT transforms, floor/residue coding, and psychoacoustic modeling; the resulting quality would almost certainly be worse than libvorbis, which has 20+ years of tuning.

**Why "static libvorbis via cc" ≠ shipping a DLL**: Build-time, `cc` compiles libvorbis source files and `rustc` static-links them into the output binary. Runtime, the user sees one `ddr-chart-tools` executable with no external library dependencies — identical distribution shape to a pure-Rust binary. The C code is an implementation detail of the build.

**Precedent**: StepManiPaX (the referenced project for DDR→SM5 conversion logic) uses Python's `soundfile` for OGG encode, which itself wraps libsndfile, which wraps libvorbis. StepManiPaX's OGG encoding is NOT pure Python — the same two C libraries are in its runtime chain, just hidden behind Python bindings. Static-linking libvorbis in Rust is architecturally cleaner than StepManiPaX's approach because the dependency is compiled into the binary rather than loaded dynamically at runtime.

**Rationale**:
- `lewton` handles all our decode needs with zero C code.
- `vorbis_rs` is the standard path for Vorbis encode in Rust; maintained, used in production by other tools.
- Build complexity is confined to one dependency; does not spread through the codebase.
- US-8's requirement is "no external CLI tool invocation at runtime" — static C linkage satisfies this, same as Open Question #10's fallback (b) in requirements explicitly allows.

**Alternatives considered**:
- `symphonia`: decode yes, encode no.
- First-party pure-Rust Vorbis encoder: rejected, see above.
- Shell out to `oggenc`: rejected by US-8.
- Use a different output format for SM5 (e.g. MP3, FLAC): rejected, SM5 standard is OGG Vorbis.

**Tradeoffs**: One C build-dep for encode only. Decoders remain fully pure-Rust. If `vorbis_rs` static linkage fails on Windows (plausible — libvorbis and Windows toolchains are historically finicky), Windows becomes best-effort per the steering files' existing stance; macOS/Linux stay first-class.

**Fallback if `vorbis_rs` itself proves unworkable**: direct FFI to libvorbis via `cc` + `bindgen` with a minimal hand-written wrapper. Same runtime shape, more code to maintain. Not expected to be needed.

### Decision 10: `log` + `env_logger`, not `tracing`

**Problem**: We need warn-level logging for dropped legacy chunks, info-level for per-file outcomes, debug/trace for development.

**Decision**: Use `log` + `env_logger`. Set up the subscriber once in `main.rs` from a verbosity count (`-v`, `-vv`).

**Rationale**: We don't need structured context propagation or async-aware spans. Plain messages are sufficient. Per the steering file, "Pick `log` vs `tracing` based on whether we need structured fields (`tracing`) or plain messages (`log`) suffice" — plain messages suffice.

**Alternatives considered**: `tracing` — rejected, overkill for a single-threaded synchronous CLI.

**Tradeoffs**: If we ever add parallel batch processing (explicitly out of scope for this deliverable), per-file context would be harder to correlate without `tracing`. Defer that problem.

---

## Component Design

### Module layout

```
src/
├── main.rs                          thin — parse CLI, setup logging, exit-code translation
├── lib.rs                           re-exports for integration tests
├── cli/
│   ├── mod.rs                       Cli struct (clap derive), validate(), into_jobs()
│   └── job.rs                       Job enum + JobPlan for batch
├── job/
│   ├── mod.rs                       run_one(job) — orchestrates parse → model → write
│   └── batch.rs                     batch runner with per-file error recovery + summary
├── model/
│   ├── mod.rs                       Song, Chart, Note, Freeze, Shock, TempoSegment, Stop
│   ├── tick.rs                      TickScale (rescales between TPS values)
│   └── preview.rs                   PreviewSlice (sample-start/sample-length for _s clip)
├── ssq/
│   ├── mod.rs                       parse + write for SSQ (modern + legacy)
│   ├── chunk.rs                     Chunk enum, chunk header I/O
│   ├── tempo.rs                     type-1 chunk (tempo + TPS + stops)
│   ├── events.rs                    type-2 chunk (song markers)
│   ├── steps.rs                     type-3 chunk (one chart's notes + freezes)
│   └── aux.rs                       types 4/5/9/17 — parsed into opaque blobs, dropped on write
├── ssq_legacy/
│   └── modernize.rs                 rescale TPS 150 → 1000; drop aux chunks with warn
├── ssc/
│   ├── mod.rs                       SSC parse + write
│   ├── msd.rs                       shared MSD tokenizer (used by sm/ too)
│   └── notes.rs                     #NOTES / #NOTEDATA parse + write
├── sm/
│   ├── mod.rs                       SM parse (read-only; no writer)
│   └── notes.rs                     SM #NOTES parse
├── xwb/
│   ├── mod.rs                       XWB parse + write
│   ├── container.rs                 WBND header, segment table, entry metadata, names
│   └── adpcm/
│       ├── mod.rs                   MS-ADPCM encode + decode entry points
│       ├── encode.rs                per-block encoder (128 samples/block, 2 channels)
│       └── decode.rs                per-block decoder (for round-trip verification)
├── xsb/
│   ├── mod.rs                       write(name) — patch song name into static template
│   └── template.bin                 extracted DDR World sample XSB, name bytes zeroed
├── wavm/
│   ├── mod.rs                       decode(bytes) → PCM (fixed 2ch/44.1kHz/XBOX-IMA)
│   └── xbox_ima.rs                  XBOX-IMA ADPCM decoder (port of vgmstream)
├── ogg/
│   ├── mod.rs                       decode + encode entry points
│   ├── decode.rs                    wraps lewton
│   └── encode.rs                    wraps vorbis_rs (or direct libvorbis FFI)
├── util/
│   ├── io.rs                        BufReader helpers, LE byte readers with offset tracking
│   ├── pair.rs                      batch-mode basename pairing logic
│   └── logging.rs                   env_logger setup from -v count
└── error.rs                         top-level Error enum (one variant per module error)
```

### Per-module errors

Each module declares its own error type via `thiserror`:

```rust
// src/ssq/mod.rs
#[derive(Debug, Error)]
pub enum SsqError {
    #[error("unexpected end of file at byte {offset}")]
    UnexpectedEof { offset: u64 },
    #[error("unknown chunk type {ty} at byte {offset}")]
    UnknownChunk { ty: u16, offset: u64 },
    #[error("I/O error")]
    Io(#[from] std::io::Error),
    // ...
}

// src/error.rs
#[derive(Debug, Error)]
pub enum Error {
    #[error("SSQ error")] Ssq(#[from] SsqError),
    #[error("SSC error")] Ssc(#[from] SscError),
    #[error("XWB error")] Xwb(#[from] XwbError),
    // ...
}
```

The CLI layer wraps with `anyhow::Context` for user-facing messages.

### Component interactions

```
                      ┌─────────┐
     argv ─────────▶  │   cli   │ ──── validated ───▶  Vec<Job>
                      └─────────┘
                                                            │
                                                            ▼
                                                       ┌─────────┐
                                                       │   job   │  (per-job orchestrator)
                                                       └─────────┘
                                                       │    │     │
                                          parse        │    │     │       write
                                      ┌──────┬─────────┘    │     └────────┬─────┬────────┐
                                      │      │              ▼              │     │        │
                                      ▼      ▼        ┌──────────┐         ▼     ▼        ▼
                                   ssq/   ssc/  sm/   │  model   │      ssq/   ssc/     xwb+xsb
                                      │      │        └──────────┘         │     │        │
                                      │      │   (ssq_legacy/modernize     │     │        │
                                      │      │    is a model→model         │     │        │
                                      │      │    transform here if        │     │        │
                                      │      │    from=DDR_LEGACY)         │     │        │
                                      ▼      ▼                             ▼     ▼        ▼
                                   (charts)  │                          (charts) │     (audio)
                                      ▲      ▼                                   ▼
                                 ┌────┴───┐ (charts)                          (charts)
                                 │  xwb   │──decode──▶ PCM ─────model.audio────▶ xwb (encode)
                                 │  (adpcm│                                      xsb (template)
                                 │  wavm  │──decode──▶ PCM                       │
                                 │  ogg   │──decode──▶ PCM                       │
                                 └────────┘                                      │
                                                                                  ▼
                                                                               ogg (encode)
```

### Layer responsibilities

| Layer | Responsibility | Does NOT do |
|-------|---------------|-------------|
| `cli/` | Parse argv, validate semantic rules, emit `Vec<Job>` | File I/O, format parsing |
| `job/` | Orchestrate one or many jobs; per-file error recovery in batch; summary | CLI concerns, format internals |
| `model/` | Format-independent types + tick-rescale transform | Any I/O, any format-specific encoding |
| `ssq/` | Parse any SSQ (modern or legacy); write only modern SSQ | XSB, audio |
| `ssq_legacy/modernize` | Transform model from legacy TPS → modern TPS; drop aux chunks with `warn!` | Parsing, writing |
| `ssc/`, `sm/` | Text-format parse + (SSC-only) write | Audio |
| `xwb/` | XWB container parse + write; MS-ADPCM encode/decode | XSB, OGG |
| `xsb/` | Template-based XSB write | XWB, audio codec |
| `wavm/` | Headerless WAVM parse (XBOX-IMA decode) | Container, other codecs |
| `ogg/` | OGG Vorbis encode/decode | XWB, ADPCM |
| `util/` | Pure helpers (byte readers, path pairing, logging setup) | Anything domain-specific |

---

## Integration Points

**External services**: None. Offline CLI, no network, no IPC, no subprocesses.

**Data storage**: Local filesystem only. Outputs written colocated with inputs.

**Runtime dependencies on external tools**: None (per US-8). Build-time requires the Rust toolchain plus a C compiler (for the `cc`-built libvorbis and the MS-ADPCM C fallback if it's needed); `cargo install` from a source checkout must succeed with no additional setup.

**Reference codebases (development-time only, not runtime)**:
- `~/Desktop/Projects/StepmaniPaX/python/stepmanipax/xwb/` — XWB parser + MS-ADPCM decoder reference (Python)
- `~/Desktop/stepmania/src/NotesLoaderSSC.cpp`, `NotesLoaderSM.cpp`, `NotesWriterSSC.cpp` — SSC/SM format reference (C++)
- `~/Desktop/vgmstream/src/meta/raw_wavm.c`, `src/coding/ima_decoder.c` — WAVM + XBOX-IMA reference (C)
- `~/Desktop/ssqparse/ssqparse_fix/src/SMToSSQ.java` — known-working XAP template (Java)
- `~/Desktop/DDR WORLD/MDX-003_20260324/contents/data/sound/win/dance/*.{xwb,xsb}` — byte-level ground truth for XWB/XSB output
- `XactBld.exe` (August 2007 DirectX SDK, loaded in Ghidra) — last-resort oracle for encoder questions

**Crate dependencies (new)**:

| Crate | Version | Purpose | Justification |
|-------|---------|---------|---------------|
| `clap` | `4` (derive) | CLI parsing | Industry standard |
| `anyhow` | `1` | CLI/job error context | Steering file recommends |
| `thiserror` | `1` | Per-module typed errors | Steering file recommends |
| `log` | `0.4` | Diagnostic logging facade | Steering file recommends |
| `env_logger` | `0.11` | Log subscriber in main | Simplest subscriber for our verbosity model |
| `lewton` | `0.10` | OGG Vorbis decode | Pure-Rust, maintained |
| `vorbis_rs` | `0.5` | OGG Vorbis encode (static libvorbis) | Only viable encode option without shell-out |
| `ogg` | `0.9` | OGG container framing for lewton interop | Needed by lewton upstream anyway |

No other deps at design time. Implementation may add small helpers (e.g. `memchr` if text scanning becomes hot), but nothing architectural.

---
## Public Contracts (signatures only)

### Model

```rust
// src/model/mod.rs

/// Format-independent representation of one song.
pub struct Song {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub bpm_segments: Vec<TempoSegment>,  // BPM changes
    pub stops: Vec<Stop>,                 // pause events
    pub charts: Vec<Chart>,               // one per difficulty/style
    pub audio: AudioBuffer,               // decoded PCM
    pub audio_sync_offset_seconds: f64,   // from SSQ tempo_data[0] / TPS, or SSC #OFFSET
    pub preview: PreviewSlice,            // for generating _s clip / #SAMPLESTART
}

pub struct Chart {
    pub style: Style,            // Single (4-panel) or Double (8-panel)
    pub difficulty: Difficulty,  // Beginner, Basic, Difficult, Expert, Challenge
    pub notes: Vec<Note>,        // sorted by beat
}

pub struct Note {
    pub beat: Beat,              // 48ths-of-a-measure or rational; TBD in implementation
    pub kind: NoteKind,          // Tap, HoldHead { length }, Shock { side }, ...
    pub panels: PanelSet,        // bitmask, 4 bits for Single, 8 for Double
}

pub enum NoteKind { Tap, HoldHead { length: Beat }, Shock { side: ShockSide } }
pub enum ShockSide { BothSides, P1Only, P2Only }

pub struct AudioBuffer {
    pub samples: Vec<i16>,       // interleaved
    pub sample_rate: u32,        // 44100 for DDR content
    pub channels: u16,           // 2 for DDR content
}

pub struct PreviewSlice { pub start_seconds: f32, pub length_seconds: f32 }
```

### Format modules

```rust
// src/ssq/mod.rs
pub fn parse(bytes: &[u8]) -> Result<Song, SsqError>;              // handles both modern & legacy
pub fn write(song: &Song, out: &mut impl Write) -> Result<(), SsqError>;  // always modern profile

// src/ssq_legacy/modernize.rs
pub fn modernize(song: &mut Song) -> Result<(), ModernizeError>;  // rescales ticks, logs drops

// src/ssc/mod.rs
pub fn parse(text: &str) -> Result<Song, SscError>;
pub fn write(song: &Song, out: &mut impl Write) -> Result<(), SscError>;

// src/sm/mod.rs
pub fn parse(text: &str) -> Result<Song, SmError>;                // no write

// src/xwb/mod.rs
pub fn parse(bytes: &[u8]) -> Result<AudioBuffer, XwbError>;       // decodes MS-ADPCM → PCM
pub fn write(name: &str, audio: &AudioBuffer, preview: &PreviewSlice, out: &mut impl Write) -> Result<(), XwbError>;

// src/xsb/mod.rs
pub fn write(name: &str, out: &mut impl Write) -> Result<(), XsbError>;  // template-based

// src/wavm/mod.rs
pub fn parse(bytes: &[u8]) -> Result<AudioBuffer, WavmError>;      // XBOX-IMA decode only, no write

// src/ogg/mod.rs
pub fn parse(bytes: &[u8]) -> Result<AudioBuffer, OggError>;
pub fn write(audio: &AudioBuffer, out: &mut impl Write) -> Result<(), OggError>;
```

### Job/CLI layer

```rust
// src/cli/mod.rs
pub struct Cli { /* clap derive */ }
impl Cli {
    pub fn validate(&self) -> Result<(), CliError>;          // semantic checks
    pub fn into_jobs(self) -> Result<Vec<Job>, CliError>;    // produces job list
}

// src/cli/job.rs
pub struct Job {
    pub from: Format,               // DDR | DDR_LEGACY | SM5
    pub to: Format,                 // DDR | SM5
    pub chart_in: PathBuf,
    pub audio_in: PathBuf,
    pub overwrite: bool,
}

// src/job/mod.rs
pub fn run_one(job: &Job) -> Result<(), Error>;              // single-job execution
pub fn run_batch(jobs: &[Job]) -> BatchSummary;              // per-file error recovery
pub struct BatchSummary { pub attempted, succeeded, failed, skipped: usize }
```

---

## Changes to Existing Code

Greenfield project — no existing code to modify. The `.spec/steering/structure.md` already describes the intended layout and matches Decision 2's module split (with the addition of new `xsb/`, `wavm/` modules inferred from the RE findings). After this design is approved, the `structure.md` file will need a minor update to reflect `xsb/` and `wavm/` as top-level modules (they weren't listed in the original structure.md because WAVM and XSB weren't in scope until requirements US-3 and the RE pass).

**Steering file updates required post-approval**:
- `.spec/steering/structure.md`: add `xsb/` and `wavm/` to the top-level layout and module responsibility table.

---

## Deployment Sequence

This is a single binary CLI, not a service. There is no deployment orchestration.

**Release flow** (when applicable — not part of this deliverable's scope):
1. `cargo build --release` produces `target/release/ddr-chart-tools`.
2. User copies the binary to their `$PATH` or runs `cargo install --path .`.
3. No rollback concept — users pin to a version by keeping the old binary.

**CI verification** (suggested, not mandated by requirements):
- `cargo fmt --check`
- `cargo clippy --all-targets -- -D warnings`
- `cargo test` (unit + integration, no external-tool setup — per US-7/US-8)

---

## Risks and Mitigations

| Risk | Impact | Likelihood | Mitigation |
|------|--------|------------|------------|
| MS-ADPCM encoder pure-Rust port produces output DDR rejects | High | Medium | Verify each stage: (1) encoder self-round-trips cleanly, (2) XWB loads in StepMania/vgmstream with correct audio, (3) XWB loads in real DDR World. Fallback: vendored C encoder (Decision 5b). |
| XSB template breaks on a future DDR World update that changes sound bank structure | Medium | Low | Keep template extraction as a documented dev task; if it breaks, regenerate from a newer DDR sample. Only the 4-char / main-first / 326-byte structural shape is supported; other shapes (5-char names in `homs2`-style XSBs) are explicitly out of scope. |
| WAVEBANKMINIWAVEFORMAT bit-packing differs from public docs | Medium | Medium | Round-trip test against every DDR World sample XWB in `~/Desktop/DDR WORLD/.../dance/*.xwb`. If a sample's parse-then-write output differs byte-for-byte from the original, the bit layout is wrong. Caught during first implementation of `xwb/container.rs`. |
| User's output filename has <4 alphanumeric ASCII characters, making 4-char code derivation ambiguous | Low | Low | CLI derives the 4-char code from the first 4 alphanumeric characters of the output basename, padding with `_` if fewer exist. If the result clashes with another song in the same output directory, DDR's behavior is undefined — document the constraint in `--help` and error if the derived code is empty. |
| WAVM file in the wild deviates from vgmstream's fixed-parameter assumption (mono, different rate) | Low | Low | Fail with a descriptive error naming what differed; document the assumption in the module-level comment. Adding variable-parameter support is a follow-up, not a blocker. |
| SSC `#NOTEDATA` with multiple-difficulty-in-one-chart forms not covered by model | Medium | Medium | Stick to the subset StepMania 5 PaX writes and DDR uses (dance-single / dance-double, one chart per `#NOTEDATA`). Reject anything else per US-2 acceptance criteria (Edit skipped with warn, unsupported stepstype rejected with error). |
| MS-ADPCM encoder is too slow (minutes per song) on realistic hardware | Low | Medium | Simple block-by-block encoder without clever predictor search should complete a 4-minute stereo song in <1 second on modern hardware. If measurement shows otherwise, add naive loop-level optimization. |
| `vorbis_rs` doesn't static-link cleanly on Windows | Medium | Medium | Windows is best-effort per steering `tech.md`; if it doesn't build on Windows, document the limitation and ship macOS/Linux only for initial release. |
| Round-trip drift (DDR → SM5 → DDR) accumulates beyond one tick at TPS=1000 per tempo segment | Medium | Low | Requirements US-3 bans drift >1 tick/segment at TPS=1000. Enforce via integration test. Use rational-number tick arithmetic (not floats) in the model's `TickScale` to eliminate accumulation. |
| Basename pairing in batch mode is ambiguous when multiple audio formats present for `DDR_LEGACY` (e.g., `foo.xwb` and `foo.wavm`) | Low (per-file) | Medium | Per requirements US-5 Open Q9: fail that file with a clear error naming the ambiguity; continue with the rest. |

---

## Resolved Design Questions

These were open during design-phase investigation; all are now resolved and captured in the Decisions above for traceability.

1. **Audio byte-copy shortcut for `DDR_LEGACY → DDR`**: resolved in Decision 3. Passthrough allowed only when source is XWB (not WAVM), a matching XSB exists, and both pass the compliance check. Otherwise full decode + re-encode.

2. **Preview clip generation for `SM5 → DDR`**: when the SSC has `#SAMPLESTART` and `#SAMPLELENGTH`, use them to slice the main audio into the `<name>_s` XWB entry. When the SSC lacks these tags, default to `start = 30.0s`, `length = 10.0s`. Captured as implementation detail on the `xwb::write` function.

3. **Preview synthesis for `DDR → SM5`**: discard the DDR `<name>_s` XWB entry entirely (don't try to locate it within the main audio via waveform correlation). Write SM5's `#SAMPLESTART = 30.0`, `#SAMPLELENGTH = 10.0` as defaults. Captured as implementation detail on the `ssc::write` function.

4. **XSB template byte offsets**: resolved in Decision 7 via a design-phase diff of all 12 DDR World sample XSBs. The 4 fixed-width 4-byte name regions (0x4a, 0x8a, 0x13a, 0x13f) and the 8-byte zeroed timestamp region (0x08) are the only per-song patches needed for the 4-char / main-first / 326-byte layout. The template is extracted from `fizz.xsb`.

---
