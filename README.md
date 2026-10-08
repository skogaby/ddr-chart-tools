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
| `DDR_LEGACY`    | `DDR`         | ✅        | legacy SSQ → modern SSQ | XWB, WAVM, or WAV → XWB (+ XSB) |
| `DDR_LEGACY`    | `SM5`         | ✅        | legacy SSQ → SSC | XWB, WAVM, or WAV → OGG |
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

Every eligible chart+audio pair in the folder is converted. Files are paired by shared basename (`song.ssq` ↔ `song.xwb`). For `DDR_LEGACY` inputs, the `_all` suffix used by Ultramix is stripped during pairing so `abs2_all.ssq` matches `abs2.wavm`. When a StepMania song ships both `song.ssc` and `song.sm`, the `.ssc` is used. Unpaired files are skipped with a warning. Subdirectories are not scanned.

For `--to-format DDR`, **name each pair after the song's DDR code** (`muka.ssc` + `muka.ogg`) — see "Song codes" below.

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
| `--sync-offset-ms N` | Move the whole chart N milliseconds later relative to the audio, on any conversion (see "Sync Offset" below) |
| `--auto-sync [MODE]` | Measure how far the chart is from its audio and correct it (`apply`, the default), or only log the correction (`report`) — see "Auto-sync" below |
| `--auto-sync-max-ms N` | Largest correction `--auto-sync` may apply, in ms (default 60, max 200) |
| `--song-code CODE` | DDR song code for a single-file `--to-format DDR` conversion; names the output files and the audio bank (see "Song codes" below) |
| `--suffix SUFFIX` | Appended to song codes the tool has to derive (`--to-format DDR`, batch or single-file); valid basenames are left as-is (see "Song codes" below) |
| `-v` / `-vv` | Increase log verbosity (debug / trace) |
| `-q` / `--quiet` | Suppress info-level output (keeps warn and error) |
| `--version` | Print version |

### Legacy modernization

```bash
ddr-chart-tools \
    --from-format DDR_LEGACY --to-format DDR \
    --input-folder path/to/legacy-songs/ \
    --auto-sync
```

Legacy-only chunks are dropped and logged. The output SSQs use the modern authoring conventions (TPS=1000, chunk types 1/2/3 plus — when the input carries them — 20 for mines). The `time_offset[0]` origin-shift used by older charts (e.g. Ultramix) is normalized so the chart timeline begins at beat 0.

### Sync Offset

Converted charts are often played by a different audio engine than the one that produced them. That engine's pipeline latency shows up as a consistent sync bias, which `--sync-offset-ms N` corrects on any conversion. Use 0 (or omit the flag) when you want the raw, unadjusted sync; tune per-target if your platform needs a constant.

A positive value moves the whole chart later relative to the audio (every note lands N ms later in the song); a negative value moves it earlier. In SSQ terms it adds N to every `tempo_data` anchor, so BPMs and stops are unchanged; in SSC terms it subtracts N/1000 from `#OFFSET`. The two formats describe the same quantity with opposite signs, and the tool handles that conversion for you.

**Ultramix → DDR World:** earlier versions recommended `--sync-offset-ms 53`. That figure was tuned when the flag only moved the first tempo anchor, which applied the full correction at the start of a song and progressively less toward the end. Measuring a Dancing Stage Unleashed rip (Ultramix engine, 44 songs) against DDR World's sync with `--auto-sync report` found no common bias: the songs it could measure needed anywhere from −57 to +54 ms (median +2 ms). Use `--auto-sync` for these charts instead of a constant; see "Auto-sync" below.

### Auto-sync

Some charts are simply a few milliseconds off their audio — including a good share of stock DDR World songs, and more so charts from older games. `--auto-sync` measures that error and corrects the chart during conversion, so you no longer need a round trip through a chart editor to nudge the offset by ear.

```bash
# See what it would do to a whole folder first:
ddr-chart-tools --from-format SM5 --to-format DDR --input-folder pack/ --auto-sync report
# Then convert with the corrections applied:
ddr-chart-tools --from-format SM5 --to-format DDR --input-folder pack/ --auto-sync
```

How it works: the tool computes an onset-strength envelope of the audio (a short-window "rising edge" detector) and correlates it with every note of every difficulty, using the chart's own BPMs and stops. The best alignment is the measured offset; the chart is moved so that offset matches the one well-synced DDR World charts produce. Only the chart moves — every tempo anchor in SSQ output, `#OFFSET` in SSC output. The audio is never modified. Against a community-maintained, play-validated per-song offset list for stock DDR World songs, it agrees within 1 ms for about 94% of songs and within 2 ms for about 98%.

- **Cap.** Corrections are at most `--auto-sync-max-ms` (default 60 ms). Much larger errors cannot be measured reliably: most DDR music has strong off-beat hits, so a chart that is half a beat off (150–200 ms at common tempos) looks as in sync as a correct one.
- **Refusals.** When a measurement is not trustworthy, the sync is left as it was and a warning says why: no usable audio, too few notes, the best match at the edge of the search, two alignments fitting about equally well, the song's two halves disagreeing by more than 5 ms (tempo drift or a cut), or a correction beyond the cap. The conversion still succeeds.
- **Log lines.** Each song gets one line ending in stable `key=value` fields (`auto_sync=apply|unchanged|report|refused`, `delta_ms=`, `reason=`, `correction_ms=`, …) so a batch can be grepped.
- **With `--sync-offset-ms`.** Auto-sync runs first; the bias is then added on top as a per-platform engine correction.
- **Legacy inputs.** Works on `DDR_LEGACY` too, including Ultramix-era charts with WAVM audio. When a compliant XWB is passed through byte-for-byte, it is decoded only for the measurement; the bank itself is still copied unchanged. Expect more refusals than on modern charts: on a Dancing Stage Unleashed rip, 16 of 44 songs were measured confidently, and most of the rest were refused because the chart's tempo drifts against the audio over the song, which a single offset cannot fix. Run `--auto-sync report` first to see which songs are affected.

### Song codes (DDR output)

DDR World finds a song's assets by its **song code** — a short ID such as `muka` — and loads `{code}.ssq`, `{code}.xwb` and `{code}.xsb`. It then plays the audio by asking the XACT engine for the cue *named* `{code}` inside the sound bank (and `{code}_s` for the preview). That lookup is a byte-for-byte string compare, case included. If the name baked into the bank does not equal the filename the song is installed under, the chart loads but **plays silently**.

The tool therefore uses one name for both: the output basename **is** the code written into the wave bank and the cues.

- **Batch mode**: the input basename is the code. Name each pair `{code}.ssc` (or `.sm`) + `{code}.ogg` before converting, e.g. `muka.ssc` + `muka.ogg` → `muka.ssq/.xwb/.xsb` with cue `muka`.
- **Single-file mode**: pass `--song-code muka`. The outputs are then named `muka.*` regardless of the input filename, ready to install.

Codes must be 1–16 lowercase ASCII letters, digits or underscores (e.g. `muka`, `bknh2`, `sign_h`). A basename that already follows that rule is used as-is — `sign_h.ssq` + `sign_h.wav` → `sign_h.ssq/.xwb/.xsb` with cue `sign_h`.

If an input basename cannot be a code (capitals, spaces, punctuation, too long — e.g. `A Is For Action.ssc`), the tool **derives** one: the first 4 letters, digits or underscores, lowercased (`aisf`), or `song` if there are none. The derived code names the output files as well as the bank and cues, so the result is installable as written; a warning reports the new name. `--suffix` is appended to derived codes only:

```bash
# "A Is For Action.ssc" -> aisf_h.ssq/.xwb/.xsb; "sign_h.ssc" stays sign_h.*
ddr-chart-tools --from-format SM5 --to-format DDR --input-folder ./pack --suffix _h
```

The suffix must be 1–12 lowercase ASCII letters, digits or underscores, so a derived code never exceeds 16 characters. If two inputs would end up with the same name (e.g. `Sign Here.ssc` and `Sign There.ssc` both derive `sign_h`, or derive the name of an input that is already valid), the run fails before converting anything and lists the clashing inputs; rename them and re-run. The check ignores case, because `Muka` and `muka` are the same file on Windows and macOS.

Renaming the *outputs* after the fact does **not** change the song code: it is inside the `.xwb` and `.xsb`, so re-run the conversion with the right name. For the same reason, a legacy XWB is only byte-copied when its internal bank name already equals the output code; otherwise it is re-encoded under the new name.

### SM5 → DDR audio requirements

The source OGG must decode to **2 channels at 44100 or 48000 Hz**. The wave bank header carries the source rate, and DDR World's audio engine reads it per entry and resamples at playback, so 48 kHz packs convert without a separate resampling step (an earlier version of the tool stamped every bank as 44.1 kHz, which made 48 kHz sources play ~9% slow). Other rates and mono sources are rejected with an error rather than being resampled or remixed silently — convert those before running the tool.

### Ultramix asset extraction

DDR Ultramix (Xbox) packs all of its assets into a pair of archives (`x_data_US.bin` and `music_US.sng`). The `scripts/extract_ultramix_xdata.py` script unpacks those into individual files ready to feed into batch mode:

```bash
python3 scripts/extract_ultramix_xdata.py ultramix_us /path/to/extracted/iso ./extracted
ddr-chart-tools \
    --from-format DDR_LEGACY --to-format DDR \
    --input-folder ./extracted \
    --auto-sync
```

The archive formats are documented in `docs/ultramix_archive_formats.md`.

### Sound-effect bank pairs (`ddr-se-bank`)

A second binary, `ddr-se-bank`, builds the XACT wave-bank + sound-bank pair needed to add a **new
sound effect** to DDR World, rather than to convert a song. The pair is registered at runtime by a
hook DLL through `IXACT2Engine::CreateInMemoryWaveBank` + `CreateSoundBank` and played by cue name;
no game asset is modified.

```bash
ddr-se-bank generate --input clap.ogg --name asti --out-dir ./banks
# -> ./banks/asti.xwb  (in-memory wave bank, MS-ADPCM mono 44.1 kHz, one entry)
# -> ./banks/asti.xsb  (sound bank, one cue named "asti", mix category 6)
```

`--name` is the wave bank's internal name, both of the sound bank's name fields, the entry's name
**and** the cue's name. The engine matches banks by name and resolves cues with a byte-exact
`strcmp`, so its case is significant. It must be 1–16 ASCII letters, digits or underscores.

The input **must already decode to mono 44100 Hz** (Ogg Vorbis). It is deliberately neither
resampled nor downmixed: doing that silently would hide a mistake upstream, and doing it with an
external tool would make the output depend on that tool's version. Convert deliberately, then feed
the result in. Generation is otherwise **deterministic** — the same input and name produce
byte-identical files on any machine, so the outputs can be committed and reproduced.

`dump` prints a wave bank's metadata, including its segment table, so a build script can check a
bank against the engine's container rules without reimplementing a parser. It works on the game's
own banks too:

```bash
ddr-se-bank dump ./banks/asti.xwb
ddr-se-bank dump /path/to/extracted/se_normal.xwb
```

**The dump format is a stable interface**: one `key=value` per line, fixed order, lowercase keys, so
`ddr-se-bank dump bank.xwb | grep '^bank.alignment=' | cut -d= -f2` is safe to depend on. The full
key list is documented in `src/xwb/dump.rs`. The field to check first is `bank.type`
(`buffer` / `streaming`) — the bank-type bit is a hard gate in both directions, so a bank of the
wrong type is rejected outright rather than merely misbehaving.

## Formats

- **SSQ** — DDR's binary stepfile. Holds multiple charts (difficulties) for a single song plus tempo and event data.
- **XWB** — Microsoft XACT Wave Bank. DDR's audio container.
- **XSB** — Microsoft XACT Sound Bank. Names the cues inside an XWB; required by DDR for the audio to be playable.
- **WAVM** — Headerless XBOX-IMA ADPCM audio (2ch, 44.1 kHz). Ultramix-era audio format.
- **WAV** — RIFF WAVE, 16-bit PCM. Accepted as `DDR_LEGACY` audio. Encoded at its source rate with no resampling; 32 kHz (DDR Hottest Party rips) is accepted for DDR output alongside 44.1 and 48 kHz.
- **Hudson SSQ charts** — DDR Hottest Party / Mario Mix (Wii, GameCube) store charts as type 9 chunks with gimmick items. `DDR_LEGACY` input converts them to the game's own gimmicks-off chart: gimmick and hand-marker arrows become normal arrows, "Koopa" arrows gain their one-beat echo arrow, hazards are removed, and freezes are kept. See `docs/hudson_ssq_format.md`.
- **Hottest Party 4 / 5 charts** — stored as type 16 lane-format chunks. Foot charts (Single, and Double on HP5) convert directly; these games keep gimmicks outside the chart file, so nothing is neutralized. The Wii Remote / Balance Board chart variants have no DDR equivalent and are dropped with a warning. See `docs/hudson_lane_ssq_format.md`.
- **SSC** — StepMania 5's simfile format. The tool's only SM5 output.
- **SM** — StepMania's older simfile format. Accepted as input (when `--from-format SM5`); never written.
- **OGG** — Ogg Vorbis audio. StepMania 5's standard audio format. As SM5→DDR input it must be stereo at 44.1 or 48 kHz (see "SM5 → DDR audio requirements").

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
