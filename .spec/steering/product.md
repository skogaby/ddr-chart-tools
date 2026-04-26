# Product Context

## What This Tool Does

`ddr-chart-tools` is a command-line utility for converting and modifying song and chart assets between Dance Dance Revolution (arcade) and StepMania 5 formats. It is a single-binary CLI, designed for offline batch processing of chart files and audio files.

Primary workflows:
1. **Arcade ↔ community roundtrip**: Convert songs between DDR arcade format (SSQ stepfile + XWB audio) and StepMania 5 format (SSC stepfile + OGG audio), in either direction, so arcade songs can be played in SM5 and SM5 songs can be injected into DDR.
2. **Legacy chart modernization**: Take pre-current-generation DDR SSQ files — whether from older arcade mixes or from console titles such as the Xbox Ultramix series — and rewrite them as modern DDR SSQs or as SSC for StepMania 5.

Typical users: a single hobbyist (the maintainer) and eventually other enthusiasts in the DDR/SM5 chart-authoring community who want to move content between the two ecosystems.

## Format Matrix

| `--from-format` | `--to-format`    | Status      | Notes |
|-----------------|------------------|-------------|-------|
| `DDR`           | `SM5`            | Supported   | SSQ + XWB → SSC + OGG |
| `SM5`           | `DDR`            | Supported   | SSC or SM + OGG → SSQ + XWB (+ XSB) |
| `DDR_LEGACY`    | `DDR`            | Supported   | legacy SSQ → modern SSQ; XWB or WAVM → XWB (+ XSB) |
| `DDR_LEGACY`    | `SM5`            | Supported   | legacy SSQ → SSC; XWB or WAVM → OGG |
| anything        | `DDR_LEGACY`     | **Not supported** | Legacy authoring is explicitly out of scope |
| `SM`            | `DDR` / `DDR_LEGACY` | n/a     | SM is accepted as input only when `--from-format SM5` is used |

When outputting StepMania 5 format, the tool always emits **SSC**, never SM. SM is an input-only dialect of SM5.

## Domain Glossary

Terms that appear in code, CLI help text, and documentation:

- **DDR** — Dance Dance Revolution (arcade), specifically the current generation that reads modern SSQs. Authoritative target for arcade output.
- **DDR World** — The specific DDR generation whose SSQ format this tool targets for output. "Modern DDR" and "DDR World" are used interchangeably.
- **DDR_LEGACY** — Any pre-current-generation DDR SSQ, including older arcade mixes and console titles (Ultramix 1-4 on Xbox). Legacy SSQs may use non-`1000` TPS values, carry chunk types the current engine ignores, and encode a non-zero `time_offset[0]` origin-shift. This tool reads them but never writes them.
- **SM5 / StepMania 5** — The open-source rhythm game and its simfile format family.
- **SSQ** — DDR's binary stepfile format. Contains one or more charts (difficulties) for a single song, plus tempo and event data. See `docs/ssq_format.md` for the byte-level spec.
- **SSC** — StepMania 5's modern simfile format. Text-based, one file per song, multiple charts embedded. The tool's only SM5 output format.
- **SM** — StepMania's older simfile format. Text-based, predates SSC. Accepted as input; never written.
- **XWB** — Microsoft XACT Wave Bank. The DDR audio container format.
- **XSB** — Microsoft XACT Sound Bank. Names the cues inside an XWB; DDR needs it to find and play the audio.
- **WAVM** — Headerless XBOX-IMA ADPCM audio (fixed 2 channels, 44.1 kHz). The audio format used by Ultramix on Xbox; extracted from `music_*.sng` archives.
- **Ultramix** — Shorthand for the Xbox-era Dance Dance Revolution Ultramix 1-4 titles. Their archive formats are documented in `docs/ultramix_archive_formats.md`.
- **OGG** — Ogg Vorbis audio. StepMania 5's standard audio format.
- **Chart / Stepfile** — The data describing what steps a player performs. "Stepfile" usually refers to the file on disk (`.ssq`, `.ssc`, `.sm`); "chart" refers to a single difficulty within it.
- **Difficulty / Step chunk** — One playable version of a song (e.g. Single Basic, Double Expert). An SSQ holds multiple step chunks; an SSC holds multiple `#NOTES` blocks.
- **Tempo chunk** — The SSQ chunk that carries BPM changes and the file's tick rate (TPS).
- **TPS (Ticks Per Second)** — The SSQ timing resolution, stored per-file in the tempo chunk's `param2`. `1000` is dominant in newly-authored charts; `150` and `75` also appear in older charts. TPS is not tied to any specific game.
- **Origin-shift** — A non-zero `time_offset[0]` in a legacy tempo chunk, representing an offset between the chart's measure timeline and the audio-sync timeline. Normalized to 0 during modernization (see Business Rule 4).
- **Sync offset / `--sync-offset-ms`** — A user-specified millisecond bias added to the audio-sync offset during modernization to compensate for per-target engine latency (e.g. Ultramix charts on DDR World commonly need ~+53 ms).
- **Freeze / Hold** — A sustained note the player must hold. Represented differently in SSQ (freeze-info block) vs SSC (`H`/`2`/`3` note symbols).
- **Shock / Mine** — Obstacle notes. Semantics differ between formats.
- **Batch mode** — Operation across every eligible pair of files in an input directory.
- **Single mode** — Operation on one explicitly-named chartfile and audiofile pair.

## Business Rules

These rules constrain implementation — violations are bugs, not style issues.

1. **DDR_LEGACY is import-only.** The tool never emits legacy-format SSQs. Any code path that would produce a `--to-format DDR_LEGACY` is an error, detected and rejected at CLI parsing time.
2. **SM5 output is always SSC.** When `--to-format SM5`, the stepfile written is SSC. SM is never an output.
3. **Modern SSQs use TPS=1000.** When the tool writes an SSQ (either via `DDR_LEGACY → DDR` or `SM5 → DDR`), it must author with TPS=1000 and must emit only chunk types 1 (tempo), 2 (events), and 3 (steps). Auxiliary chunks (types 4, 5, 9, 17) are not produced.
4. **Legacy timelines normalize to a zero origin.** When converting `DDR_LEGACY → DDR` or `DDR_LEGACY → SM5`, the tempo, event, and step timelines are shifted so the first tempo entry sits at beat 0. Any legacy-only chunks that reached the parser are discarded. The tool logs what was dropped; it does not attempt to preserve them.
5. **Audio conversion is automatic and paired with the chart.** Whenever a chart is converted, its audio is converted alongside it in the same run. Users never have to invoke audio conversion separately.
6. **Flat input directories only.** In batch mode, only files directly in the input folder are considered. Subdirectories are not scanned.
7. **Default output is under the input tree; `--output-dir` overrides.** Single-file conversions write to `./output` by default; batch writes to `<input-folder>/output`. When `--output-dir` is provided, it is used verbatim in both modes. Output filenames are derived deterministically from input filenames (the `_all` suffix used by Ultramix is stripped).
8. **File pairing.** In batch mode, a chart file and its corresponding audio file are paired by shared basename (e.g. `foo.ssq` ↔ `foo.xwb`, `bar.ssc` ↔ `bar.ogg`). For `DDR_LEGACY` inputs, the chart's `_all` suffix is stripped during pairing so Ultramix charts match their audio (`abs2_all.ssq` ↔ `abs2.wavm`). Unpaired files are skipped with a warning.
9. **Ultramix metadata from `.sif` is ingested opportunistically.** When a sibling `.sif` file exists next to an Ultramix chart, its title / subtitle / artist fields are used to populate `#TITLE` and `#ARTIST` in SSC output. Missing `.sif` files are silently tolerated.
10. **Sync-offset bias is additive, not replacing.** `--sync-offset-ms N` adds N ms to the post-modernize audio-sync offset; it does not overwrite what was in the source file.

## What Success Looks Like

- A user can run one command and get back a directory of converted songs that play correctly in the target platform.
- Round-tripping a song (e.g. DDR → SM5 → DDR) preserves chart timing and note placement within the precision allowed by each format. Exact byte-preservation is not required; gameplay-equivalent output is.
- Legacy SSQs — including Ultramix-era ones with non-zero origins and non-1000 TPS — load without errors on the target platform and play with the correct timing (modulo the documented sync bias).
- Errors on individual files in batch mode don't abort the whole run; the tool reports what failed and continues.

## Out of Scope (For Now)

These are intentionally deferred. Say "no" to scope creep that tries to add them without explicit planning.

- Thumbnails, jackets, banners, video backgrounds (`.sif` title/artist is handled; other per-song art is not).
- Preview clips, keysounds, lyrics.
- Recursive directory scanning in batch mode.
- Emitting legacy-format SSQs.
- Emitting SM (only SSC).
- GUI or interactive prompts.
- Network fetching, dependency downloads, or any non-local file access.
- Chart authoring features (editing, generation, balancing) — this tool only converts what already exists.
- DDR generations other than the current "modern" target.

## Common Mistakes to Avoid

- Treating SM and SSC as the same format in the code. They share a lineage but have different fields and parsing rules. Accepting SM input does not mean the SSC writer can be reused for SM output (and we don't write SM anyway — see rule 2).
- Assuming TPS is always 1000. Legacy SSQs use a mix of `1000`, `150`, and `75`; every tick-valued field is scaled by the file's declared TPS. Rescaling to 1000 happens during modernization, not at parse time.
- Assuming `time_offset[0]` is always 0. In legacy SSQs it may be negative or positive; the modernize step normalizes the whole timeline. Chart-content `time_offset` values are always ≥ 0 regardless of the tempo origin.
- Preserving legacy-only data when modernizing. Rule 4 says drop it. Keeping it "just in case" produces files modern DDR misreads.
- Treating `--sync-offset-ms` as a per-song correction. It's a per-target engine-latency bias; use one value for an entire batch of the same target.
- Using the presence of a chart file alone to trigger conversion. Rule 8 requires a matching audio file; chartfiles without audio are skipped in batch mode.
- Silently swallowing parse errors on legacy-only chunks. The tool should parse them, confirm they're legacy-only, and log their drop — not pretend they didn't exist.
