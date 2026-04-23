# ddr-chart-tools

A command-line utility for converting and modifying song and chart assets between Dance Dance Revolution (arcade) and StepMania 5 formats.

## What It Does

- Converts songs between **DDR arcade format** (SSQ stepfile + XWB audio) and **StepMania 5 format** (SSC stepfile + OGG audio) in either direction.
- Modernizes "legacy" DDR SSQ files (authored by an older pipeline, containing data that modern DDR no longer respects) into modern SSQs that current DDR reads correctly.
- Handles chartfile and audiofile conversion together in a single run. You don't convert audio separately.
- Works on single files or a folder of files (batch mode).

## Format Matrix

| `--from-format` | `--to-format` | Supported | Chart I/O | Audio I/O |
|-----------------|---------------|-----------|-----------|-----------|
| `DDR`           | `SM5`         | ✅        | SSQ → SSC | XWB → OGG |
| `SM5`           | `DDR`         | ✅        | SSC or SM → SSQ | OGG → XWB |
| `DDR_LEGACY`    | `DDR`         | ✅        | legacy SSQ → modern SSQ | (audio pass-through or converted as needed) |
| `DDR_LEGACY`    | `SM5`         | ✅        | legacy SSQ → SSC | XWB → OGG |
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

Output files are written alongside the inputs (e.g. `path/to/song.ssc` and `path/to/song.ogg`).

### Batch conversion

```bash
ddr-chart-tools \
    --from-format DDR --to-format SM5 \
    --input-folder path/to/ddr-songs/
```

Every eligible chart+audio pair in the folder is converted. Files are paired by shared basename (`song.ssq` ↔ `song.xwb`). Unpaired files are skipped with a warning. Subdirectories are not scanned in this release.

### Flags

| Flag | Description |
|------|-------------|
| `--from-format` | Source format: `DDR`, `DDR_LEGACY`, or `SM5` (required) |
| `--to-format` | Target format: `DDR` or `SM5` (required) |
| `--chartfile` | Path to a single chart file (requires `--audiofile`) |
| `--audiofile` | Path to a single audio file (requires `--chartfile`) |
| `--input-folder` | Directory of file pairs to convert in batch |
| `--overwrite` | Silently replace existing output files |
| `-v` / `-vv` | Increase log verbosity (debug / trace) |
| `-q` / `--quiet` | Suppress info-level output (keeps warn and error) |
| `--version` | Print version |

### Legacy modernization

```bash
ddr-chart-tools \
    --from-format DDR_LEGACY --to-format DDR \
    --input-folder path/to/legacy-songs/
```

Legacy-only chunks and fields are dropped and logged. The output SSQs use the modern authoring conventions (TPS=1000, chunk types 1/2/3 only).

## Formats

- **SSQ** — DDR's binary stepfile. Holds multiple charts (difficulties) for a single song plus tempo and event data.
- **XWB** — Microsoft XACT Wave Bank. DDR's audio container.
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

- Thumbnails, jackets, banners, or video backgrounds
- Preview clips, keysounds, or lyrics
- Recursive directory scanning in batch mode
- Emitting legacy-format SSQs
- Emitting SM (only SSC)
- GUI or interactive prompts

These may be added later.
