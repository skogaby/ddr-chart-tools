# Learnings: software-developer

## Learning 1
- **Context**: Working on the ddr-chart-tools project (solo hobby project, no team, user is sole arbiter)
- **Rule**: Chain tasks across a single session. After the user approves a task, do the approval bookkeeping (check task boxes in tasks.md, log events, update state/ui-status) and move straight into the next task using the same @agent-sop:implement flow — don't stop and hand back to EM. User explicitly overrides the default "one task per invocation" SOP rule for this project.
- **Scope**: project
- **Source**: explicit
- **Added**: 2026-04-22T23:39:40-07:00

## Learning 2
- **Context**: Working on the ddr-chart-tools project — CR flow (git commits, CR creation, AutoSDE loop) at the end of each task
- **Rule**: Don't run git operations. User manages all git operations themselves — they do checkpoint commits between tasks and squash before pushing. Skip the CR flow entirely (no `git commit`, no `cr --new-review`, no AutoSDE polling). After task approval, just pause and check in with the user before starting the next task; if they approve, they'll do the checkpoint commit on their side, then you start the next task.
- **Scope**: project
- **Source**: explicit
- **Added**: 2026-04-22T23:40:30-07:00

## Learning 3
- **Context**: Choosing a reference codebase for stepfile conversion logic (BPM calculations, tick↔beat conversions, note-position representation) in ddr-chart-tools
- **Rule**: StepManiPaX's Python implementation (`~/Desktop/Projects/StepmaniPaX/python/stepmanipax/`) is the more solid reference for chart/stepfile conversion logic. ssqparse has known issues and bugs in BPM calculation and conversion — use it only for byte-level SSQ structural reference (chunk framing, XAP template patterns), not for conversion semantics.
- **Scope**: project
- **Source**: explicit
- **Added**: 2026-04-23T00:06:00-07:00

## Learning 4
- **Context**: Designing format abstractions in ddr-chart-tools (traits vs enums, etc.)
- **Rule**: The stepfile formats are fixed: DDR side is always SSQ (variations are chunk presence/TPS, not new file types), StepMania side is always SM (input-only) or SSC. No future stepfile formats expected. Audio formats, by contrast, are open-ended — more legacy formats (FSB, ADX, HCA, AT3, etc.) will likely be added for other-game-to-StepMania conversion. Build the audio module to be extensible (shared `AudioFormatKind` enum + shared `AudioError`), but don't over-abstract stepfiles.
- **Scope**: project
- **Source**: explicit
- **Added**: 2026-04-23T00:23:00-07:00

## Learning 5
- **Context**: Writing tests for ddr-chart-tools (any task)
- **Rule**: Never rely on real Konami assets (DDR World SSQ/XWB/XSB files, real StepMania fixtures, audio samples) being present on disk. Tests stay at a conceptual level using synthetic fixtures built in-code from spec-defined byte layouts. Do not codify integration tests that read `~/Desktop/DDR WORLD/...` or commit real assets to the repo. End-to-end verification happens manually against real files after final implementation; don't turn that into an automated test. Task 20 (originally "End-to-end integration tests") is therefore out of scope as written — skip it or repurpose when we get there.
- **Scope**: project
- **Source**: explicit
- **Added**: 2026-04-23T00:57:00-07:00

## Learning 6
- **Context**: Interpreting "legacy" terminology in ddr-chart-tools
- **Rule**: "Legacy" has two unrelated meanings — don't conflate them. (1) `DDR_LEGACY` at the tool/CLI level means "DDR releases BEFORE DDR World" — a separate class of file, not a subset of DDR World content. This triggers the modernize transform when `--from-format DDR_LEGACY`. (2) Within `docs/ssq_format.md` §1.1, "legacy" describes TPS=150 SSQs that exist INSIDE DDR World alongside TPS=1000 ones — these are normal DDR World assets and must NOT be modernized. The parser should record TPS as a plain field; the source profile (DDR World vs pre-World) comes from the CLI `--from-format` flag, not from tempo-chunk inspection. Modernize runs whenever `from == DDR_LEGACY`, regardless of the input file's TPS.
- **Scope**: project
- **Source**: explicit
- **Added**: 2026-04-23T01:10:00-07:00

## Learning 7
- **Context**: Handling SSQ-format-specific data in ddr-chart-tools (events, aux chunks, or any other concept that doesn't map cleanly to SSC/SM5)
- **Rule**: Do NOT add format-specific fields to the common `Song` in `model/` — that violates steering's "no format-specific types in model/" rule. Instead, parsers return a wrapper struct like `SsqParseResult { song: Song, events: Vec<SsqEvent>, aux_chunks_dropped: Vec<AuxMeta> }` that carries the common `Song` plus format-specific sidecar data. Job layer threads the sidecar data only where it's meaningful (e.g. DDR→DDR writer receives events; DDR→SM5 drops them). Prefer perfect round-trip preservation (option C) over regeneration-from-template (option B) unless round-trip perfection is impossible.
- **Scope**: project
- **Source**: explicit
- **Added**: 2026-04-23T01:30:30-07:00

## Learning 9
- **Context**: SSQ tempo chunks and the span-vs-boundary representation of tempo segments in ddr-chart-tools
- **Rule**: Follow the same Option-C sidecar pattern used for events: raw `(time_offset, tempo_data)` pairs live on `SsqParseResult` (sidecar), while the semantic `tempo_segments: Vec<TempoSegment>` view on `Song` stays clean for cross-format consumers (SSC writer). DDR→DDR writes ride the raw data verbatim (perfect round-trip). SM5→DDR reconstructs from the semantic view + a synthesized trailing entry derived from the maximum chart beat. This mirrors how events are preserved — raw data on the sidecar, semantic view on Song.
- **Scope**: project
- **Source**: explicit
- **Added**: 2026-04-23T02:48:00-07:00

## Learning 10
- **Context**: Creating new source files in ddr-chart-tools
- **Rule**: Before creating any new file, `touch` it first then `git add` it while empty, THEN write its contents. This ensures the new file shows up in `git diff` alongside its content changes between checkpoint commits, giving the user a holistic view of the full changeset. Apply per file (not per task) — when a task creates multiple new files, each gets its own touch+add before content is written.
- **Scope**: project
- **Source**: explicit
- **Added**: 2026-04-23T09:28:20-07:00
