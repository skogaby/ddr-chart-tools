# ddr-chart-tools

A command-line utility for converting and modifying song and chart assets between Dance Dance Revolution (arcade) and StepMania 5 formats, with first-class support for older DDR releases.

## What It Does

- Converts songs between **DDR arcade format** (SSQ stepfile + XWB audio) and **StepMania 5 format** (SSC stepfile + OGG audio) in either direction.
- Modernizes **legacy DDR SSQ files** (from pre-current-generation DDR releases, including Ultramix-era Xbox titles) into modern DDR SSQs or into SSC for StepMania 5.
- Handles chart and audio conversion together in a single run. You don't convert audio separately.
- Works on single files or a folder of files (batch mode).

## Format Matrix

| `--from-format` | `--to-format` | Supported | Chart I/O | Audio I/O |
|-----------------|---------------|-----------|-----------|-----------|
| `DDR`           | `SM5`         | ✅        | SSQ → SSC | XWB → OGG |
| `SM5`           | `DDR`         | ✅        | SSC or SM → SSQ | OGG → XWB (+ XSB) |
| `DDR_LEGACY`    | `DDR`         | ✅        | legacy SSQ → modern SSQ | XWB or WAVM → XWB (+ XSB) |
| `DDR_LEGACY`    | `SM5`         | ✅        | legacy SSQ → SSC | XWB or WAVM → OGG |
| anything        | `DDR_LEGACY`  | ❌ not supported — legacy authoring is out of scope |

When the output is StepMania 5 format, the tool always produces **SSC**, never SM.

## Usage

### Single-file conversion

```bash
ddr-chart-tools \
    --from-format DDR --to-format SM5 \
    --chartfile path/to/song.ssq \
    --audiofile path/to/song.xwb
```

Output files land in `./output` by default. Use `--output-dir` to place them elsewhere.

### Batch conversion

```bash
ddr-chart-tools \
    --from-format DDR --to-format SM5 \
    --input-folder path/to/ddr-songs/
```

Every eligible chart+audio pair in the folder is converted. Files are paired by shared basename (`song.ssq` ↔ `song.xwb`). For `DDR_LEGACY` inputs, the `_all` suffix used by Ultramix is stripped during pairing so `abs2_all.ssq` matches `abs2.wavm`. Unpaired files are skipped with a warning. Subdirectories are not scanned.

Batch output defaults to `<input-folder>/output/`. Use `--output-dir` to override.

### Flags

| Flag | Description |
|------|-------------|
| `--from-format` | Source format: `DDR`, `DDR_LEGACY`, or `SM5` (required) |
| `--to-format` | Target format: `DDR` or `SM5` (required) |
| `--chartfile` | Path to a single chart file (requires `--audiofile`) |
| `--audiofile` | Path to a single audio file (requires `--chartfile`) |
| `--input-folder` | Directory of file pairs to convert in batch |
| `--output-dir` | Directory to write output into (defaults: `./output` single, `<input>/output` batch) |
| `--overwrite` | Silently replace existing output files |
| `--sync-offset-ms N` | Add N milliseconds to the audio-sync offset (see "Sync Offset" below) |
| `-v` / `-vv` | Increase log verbosity (debug / trace) |
| `-q` / `--quiet` | Suppress info-level output (keeps warn and error) |
| `--version` | Print version |

### Legacy modernization

```bash
ddr-chart-tools \
    --from-format DDR_LEGACY --to-format DDR \
    --input-folder path/to/legacy-songs/ \
    --sync-offset-ms 53
```

Legacy-only chunks are dropped and logged. The output SSQs use the modern authoring conventions (TPS=1000, chunk types 1/2/3 plus — when the input carries them — 20 for mines). The `time_offset[0]` origin-shift used by older charts (e.g. Ultramix) is normalized so the chart timeline begins at beat 0.

### Sync Offset

Converted legacy charts are often played by a different audio engine than the one that produced them. That engine's pipeline latency shows up as a consistent sync bias — in practice, **Ultramix → DDR World** output drifts ~53 ms and benefits from `--sync-offset-ms 53`. Use 0 (or omit the flag) when you want the raw, unadjusted sync; tune per-target if your platform needs a different constant.

### Ultramix asset extraction

DDR Ultramix (Xbox) packs all of its assets into a pair of archives (`x_data_US.bin` and `music_US.sng`). The `scripts/extract_ultramix_xdata.py` script unpacks those into individual files ready to feed into batch mode:

```bash
python3 scripts/extract_ultramix_xdata.py ultramix_us /path/to/extracted/iso ./extracted
ddr-chart-tools \
    --from-format DDR_LEGACY --to-format DDR \
    --input-folder ./extracted \
    --sync-offset-ms 53
```

The archive formats are documented in `docs/ultramix_archive_formats.md`.

## Formats

- **SSQ** — DDR's binary stepfile. Holds multiple charts (difficulties) for a single song plus tempo and event data.
- **XWB** — Microsoft XACT Wave Bank. DDR's audio container.
- **XSB** — Microsoft XACT Sound Bank. Names the cues inside an XWB; required by DDR for the audio to be playable.
- **WAVM** — Headerless XBOX-IMA ADPCM audio (2ch, 44.1 kHz). Ultramix-era audio format.
- **SSC** — StepMania 5's simfile format. The tool's only SM5 output.
- **SM** — StepMania's older simfile format. Accepted as input (when `--from-format SM5`); never written.
- **OGG** — Ogg Vorbis audio. StepMania 5's standard audio format.

## Installation

(Not yet published. Build from source for now.)

### Build from source

Requires a stable Rust toolchain (install via [rustup](https://rustup.rs/)).

```bash
git clone <this repo>
cd ddr-chart-tools
cargo build --release
```

The binary lands at `target/release/ddr-chart-tools`. Copy it somewhere on your `$PATH` (e.g. `~/bin`).

Alternatively:

```bash
cargo install --path .
```

### Cross-compile for Windows (from macOS)

```bash
# One-time setup
rustup target add x86_64-pc-windows-gnu
brew install mingw-w64

# Build
cargo build --release --target x86_64-pc-windows-gnu
```

The Windows binary lands at `target/x86_64-pc-windows-gnu/release/ddr-chart-tools.exe`.

### Cross-compile for Linux (from macOS)

```bash
# One-time setup
rustup target add x86_64-unknown-linux-musl
brew install filosottile/musl-cross/musl-cross

# Build
cargo build --release --target x86_64-unknown-linux-musl
```

The Linux binary lands at `target/x86_64-unknown-linux-musl/release/ddr-chart-tools`. It is statically linked and runs on any x86_64 Linux distribution without additional dependencies.

## Development

```bash
cargo build                       # debug build
cargo run -- --help               # run the CLI
cargo test                        # unit + integration tests
cargo clippy -- -D warnings       # lint (must pass)
cargo fmt                         # format
```

## Out of Scope

For now, this tool does **not** handle:

- Thumbnails, jackets, banners, or video backgrounds (Ultramix's `.sif` → SSC title/artist is handled; other art is not)
- Preview clips, keysounds, or lyrics
- Recursive directory scanning in batch mode
- Emitting legacy-format SSQs
- Emitting SM (only SSC)
- GUI or interactive prompts

These may be added later.
