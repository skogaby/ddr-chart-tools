# Product Context

## What This Tool Does

`ddr-chart-tools` is a command-line utility for converting and modifying song and chart assets between Dance Dance Revolution (arcade) and StepMania 5 formats. It is a single-binary CLI, designed for offline batch processing of chart files and audio files.

Primary workflows:
1. **Arcade ↔ community roundtrip**: Convert songs between DDR arcade format (SSQ stepfile + XWB audio) and StepMania 5 format (SSC stepfile + OGG audio), in either direction, so arcade songs can be played in SM5 and SM5 songs can be injected into DDR.
2. **Legacy chart modernization**: Take "legacy" DDR SSQ files (authored by an older pipeline, containing data that modern DDR no longer respects) and rewrite them as "modern" SSQs that current DDR reads correctly.

Typical users: a single hobbyist (the maintainer) and eventually other enthusiasts in the DDR/SM5 chart-authoring community who want to move content between the two ecosystems.

## Format Matrix

| `--from-format` | `--to-format`    | Status      | Notes |
|-----------------|------------------|-------------|-------|
| `DDR`           | `SM5`            | Supported   | SSQ + XWB → SSC + OGG |
| `SM5`           | `DDR`            | Supported   | SSC or SM + OGG → SSQ + XWB |
| `DDR_LEGACY`    | `DDR`            | Supported   | SSQ (legacy) → SSQ (modern) |
| `DDR_LEGACY`    | `SM5`            | Supported   | SSQ (legacy) → SSC + OGG (via modernization) |
| anything        | `DDR_LEGACY`     | **Not supported** | Legacy authoring is explicitly out of scope |
| `SM`            | `DDR` / `DDR_LEGACY` | n/a     | SM is accepted as input only when `--from-format SM5` is used |

When outputting StepMania 5 format, the tool always emits **SSC**, never SM. SM is an input-only dialect of SM5.

## Domain Glossary

Terms that appear in code, CLI help text, and documentation:

- **DDR** — Dance Dance Revolution (arcade), specifically the current generation that reads "modern" SSQs. Authoritative target for arcade output.
- **DDR World** — The specific DDR generation whose SSQ format this tool targets for output. "Modern DDR" and "DDR World" are used interchangeably.
- **DDR_LEGACY** — An older SSQ authoring pipeline. Legacy SSQs contain chunks and field values that modern DDR ignores or misinterprets. This tool can read them but never writes them.
- **SM5 / StepMania 5** — The open-source rhythm game and its simfile format family.
- **SSQ** — DDR's binary stepfile format. Contains one or more charts (difficulties) for a single song, plus tempo and event data. See `docs/ssq_format.md` for the byte-level spec.
- **SSC** — StepMania 5's modern simfile format. Text-based, one file per song, multiple charts embedded. The tool's only SM5 output format.
- **SM** — StepMania's older simfile format. Text-based, predates SSC. Accepted as input; never written.
- **XWB** — Microsoft XACT Wave Bank. The DDR audio container format. Typically contains one ADPCM-encoded track per song.
- **OGG** — Ogg Vorbis audio. StepMania 5's standard audio format.
- **Chart / Stepfile** — The data describing what steps a player performs. "Stepfile" usually refers to the file on disk (`.ssq`, `.ssc`, `.sm`); "chart" refers to a single difficulty within it.
- **Difficulty / Step chunk** — One playable version of a song (e.g. Single Basic, Double Expert). An SSQ holds multiple step chunks; an SSC holds multiple `#NOTES` blocks.
- **Tempo chunk** — The SSQ chunk that carries BPM changes and the file's tick rate (TPS).
- **TPS (Ticks Per Second)** — The SSQ timing resolution, stored per-file. Modern DDR uses 1000, legacy uses 150.
- **Freeze / Hold** — A sustained note the player must hold. Represented differently in SSQ (freeze-info block) vs SSC (`H`/`2`/`3` note symbols).
- **Shock / Mine** — Obstacle notes. Semantics differ between formats.
- **Batch mode** — Operation across every eligible pair of files in an input directory.
- **Single mode** — Operation on one explicitly-named chartfile and audiofile pair.

## Business Rules

These rules constrain implementation — violations are bugs, not style issues.

1. **DDR_LEGACY is import-only.** The tool never emits legacy-format SSQs. Any code path that would produce a `--to-format DDR_LEGACY` is an error, detected and rejected at CLI parsing time.
2. **SM5 output is always SSC.** When `--to-format SM5`, the stepfile written is SSC. SM is never an output.
3. **Modern SSQs use TPS=1000.** When the tool writes an SSQ (either via `DDR_LEGACY → DDR` or `SM5 → DDR`), it must author with TPS=1000 and must emit only chunk types 1 (tempo), 2 (events), and 3 (steps). Auxiliary chunks (types 4, 5, 9, 17) are not produced.
4. **Legacy-only data is dropped during modernization.** When converting `DDR_LEGACY → DDR`, any chunks or fields that modern DDR does not read are discarded. The tool logs what was dropped; it does not attempt to preserve them.
5. **Audio conversion is automatic and paired with the chart.** Whenever a chart is converted, its audio is converted alongside it in the same run. Users never have to invoke audio conversion separately.
6. **Flat input directories only (for now).** In batch mode, only files directly in the input folder are considered. Subdirectories are not scanned. This is an intentional simplification that may change later.
7. **Output is colocated with input.** Converted files are written to the same directory as the source file. Output filenames are derived deterministically from input filenames (collision handling is a design-phase decision).
8. **File pairing.** In batch mode, a chart file and its corresponding audio file are paired by shared basename (e.g. `foo.ssq` ↔ `foo.xwb`, `bar.ssc` ↔ `bar.ogg`). Unpaired files are skipped with a warning, not an error.

## What Success Looks Like

- A user can run one command and get back a directory of converted songs that play correctly in the target platform.
- Round-tripping a song (e.g. DDR → SM5 → DDR) preserves chart timing and note placement within the precision allowed by each format. Exact byte-preservation is not required; gameplay-equivalent output is.
- Legacy SSQs converted to modern SSQs load without errors on current DDR and play with the correct timing.
- Errors on individual files in batch mode don't abort the whole run; the tool reports what failed and continues.

## Out of Scope (For Now)

These are intentionally deferred. Say "no" to scope creep that tries to add them without explicit planning.

- Thumbnails, jackets, banners, video backgrounds.
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
- Assuming TPS is always 1000. Legacy SSQs use TPS=150, and every tick-valued field in the file is scaled by that rate.
- Preserving legacy-only data when modernizing. Rule 4 says drop it. Keeping it "just in case" produces files modern DDR misreads.
- Writing output to a separate directory. Rule 7 says colocate.
- Using the presence of a chart file alone to trigger conversion. Rule 8 requires a matching audio file; chartfiles without audio are skipped in batch mode.
- Silently swallowing parse errors on legacy-only chunks. The tool should parse them, confirm they're legacy-only, and log their drop — not pretend they didn't exist.
