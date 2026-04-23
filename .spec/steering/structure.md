# Package Structure

This file describes the *intended* Cargo project layout. The layout materializes during the first implementation task — nothing is created at bootstrap time. Treat this document as the map developers and agents follow when they add files.

## Top-Level Layout

```
ddr-chart-tools/
├── Cargo.toml              # package manifest
├── Cargo.lock              # committed (binary crate convention)
├── README.md               # end-user docs: install, usage, examples
├── .cargo/
│   └── config.toml         # cross-compilation linker config (Windows target)
├── .gitignore
├── .spec/
│   ├── workspace-manifest.json
│   ├── steering/           # these files
│   └── workflow/           # per-feature workflows
├── docs/
│   └── ssq_format.md       # byte-level SSQ spec — authoritative reference for the SSQ parser
├── src/
│   ├── main.rs             # binary entry point; thin — calls into cli module
│   ├── lib.rs              # library surface for integration tests
│   ├── error.rs            # top-level Error enum (wraps each module's typed error)
│   ├── cli/                # arg parsing, validation, job planning
│   ├── job/                # per-job orchestration (parse → model → write) + batch runner
│   ├── model/              # format-independent types (Song, Chart, Note, …)
│   ├── ssq/                # SSQ parse + write (modern DDR)
│   ├── ssq_legacy/         # legacy SSQ modernization (tick rescale, aux-chunk drop)
│   ├── ssc/                # SSC parse + write
│   ├── sm/                 # SM parse only (never written)
│   ├── xwb/                # XWB container parse + write, MS-ADPCM codec (adpcm/)
│   ├── xsb/                # XSB template-based writer + template.bin
│   ├── wavm/               # WAVM decode (headerless XBOX-IMA ADPCM)
│   ├── ogg/                # OGG Vorbis decode (lewton) + encode (vorbis_rs)
│   └── util/               # cross-cutting helpers (byte readers, path pairing, logging)
└── tests/
    ├── fixtures/           # small sample files (if licensing allows)
    └── *.rs                # integration tests
```

## Where Things Go

### Adding a new format-to-format conversion
1. Confirm the source format module (`src/{source}/`) exposes a `parse` function returning the model type.
2. Confirm the target format module (`src/{target}/`) exposes a `write` function accepting the model type.
3. Wire it into `src/job/` — add a branch to the job dispatcher for the `(from, to)` pair.
4. Extend the CLI validation in `src/cli/` to accept the new combination.
5. Add an integration test under `tests/` that round-trips a fixture through the new path.

### Adding a new field to the common model
1. Add the field to the struct in `src/model/`.
2. Update every format's `parse` to populate it (or explicitly document that it's unavailable from that format).
3. Update every format's `write` to emit it (or explicitly document that it's dropped on that output).
4. Update the conversion tests to assert the field round-trips where expected.

### Parsing a new SSQ chunk type
1. Add a variant to the SSQ chunk enum in `src/ssq/` (or `src/ssq_legacy/` if legacy-only).
2. Implement the byte-level reader, referencing `docs/ssq_format.md` section numbers in comments.
3. Decide whether the chunk contributes to the common model. If it's legacy-only, the `ssq_legacy` modernization step drops it with a `warn!` log.
4. Add a parser unit test with a known-good byte sequence.

### Fixing a bug reported against a specific file
1. Reduce the failing file to the smallest reproducer possible (ideally a single chart, trimmed chunks).
2. Check it into `tests/fixtures/` if licensing permits; otherwise document how to obtain the file.
3. Write the failing test first.
4. Fix and verify.

## Module Responsibilities

| Module | Owns | Does not own |
|--------|------|--------------|
| `cli/` | arg parsing, validation, translating CLI intent into a list of conversion jobs | file I/O, format parsing |
| `job/` | per-job orchestration (dispatch, output paths, overwrite check), batch runner with per-file error recovery | CLI concerns, binary-level format details |
| `model/` | format-independent types and rules about valid combinations | any I/O, any format-specific encoding |
| `ssq/` | modern SSQ parse + write, chunk types 1/2/3 only | SSC writing, audio |
| `ssq_legacy/` | legacy SSQ modernization (tick rescale, aux-chunk drop) | writing SSQs (defers to `ssq/`) |
| `ssc/` | SSC text parse + write | SM parsing (separate module), audio |
| `sm/` | SM text parse only | any writing |
| `xwb/` | XWB container parse + write, MS-ADPCM decode/encode (`adpcm/` submodule) | OGG concerns |
| `xsb/` | XSB template-based writer (static template + name patching) | XWB, audio codec |
| `wavm/` | WAVM decode (headerless XBOX-IMA ADPCM, fixed 2ch/44100Hz) | container parsing, other codecs |
| `ogg/` | OGG Vorbis decode (lewton) + encode (vorbis_rs) | XWB concerns |
| `util/` | pure helpers: byte readers, basename pairing, logging setup | anything domain-specific |

## Naming Conventions

| Type | Pattern | Example |
|------|---------|---------|
| Module directories | `snake_case`, singular | `ssq/`, not `ssqs/` or `SSQ/` |
| Parse function | `parse` or `parse_{thing}` returning `Result<T, Error>` | `ssq::parse(bytes) -> Result<Ssq, SsqError>` |
| Write function | `write` or `write_{thing}` taking a writer | `ssc::write(&song, &mut writer) -> Result<(), SscError>` |
| Error types | `{Format}Error` in each format module | `SsqError`, `SscError` |
| Model types | domain nouns, no format prefix | `Song`, `Chart`, `Note`, `TempoChange` |
| Format-specific types | prefixed with format name | `SsqChunk`, `SscTag`, `XwbEntry` |
| Integration test files | `{from}_to_{to}.rs` | `tests/ddr_to_sm5.rs`, `tests/ddr_legacy_to_ddr.rs` |

## Things Agents Should Not Do

- **Don't invent a new top-level module** (`src/manager/`, `src/service/`, etc.). The categories above cover every concern this tool has.
- **Don't put format-specific types in `model/`**. If something belongs only to SSQ, it goes in `ssq/`.
- **Don't bypass the model layer**. A direct `src/ssq_to_ssc.rs` is wrong; always `ssq → model → ssc`.
- **Don't add a file at repo root that isn't in the top-level layout above** without updating this document first.
- **Don't edit `docs/ssq_format.md`** as part of implementation work — it's a reference document, not a living design artifact. If the format doc is wrong, that's a separate, explicit task.

## Where to Find Things

| Question | Look here |
|----------|----------|
| "How does the CLI parse args?" | `src/cli/` |
| "What does an SSQ file look like on disk?" | `docs/ssq_format.md` |
| "How is a chart represented in memory?" | `src/model/` |
| "Why does conversion X exist?" | `README.md` (format matrix) and `src/cli/` (validation rules) |
| "Why was crate Y chosen?" | the feature design doc that introduced it |
| "What's the end-user install path?" | `README.md` |
