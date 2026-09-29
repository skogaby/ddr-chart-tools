# Summary: Auto-Sync

PDD pass completed 2026-09-28. Design and plan approved by the maintainer.

## Artifacts

| Path | Contents |
|---|---|
| `rough-idea.md` | The idea as first stated |
| `idea-honing.md` | Decision register D1–D24 (all Accepted, Overridden, or Assumed), questions, outliers note, readiness gate |
| `research/orientation.md` | Codebase blind-spot pass, incl. the `--sync-offset-ms` first-segment bug |
| `research/calibration-corpus.md` | Stock DDR World + community offset CSV: provenance, sign, statistics, TPS split |
| `research/onset-detection.md` | Onset-feature literature and the +9ms or Null? algorithm |
| `research/dependencies.md` | `rustfft` footprint |
| `research/prototype-results.md` | Full prototype results: sign, accuracy, aliasing, cap, gating, timing model, Ghidra verification |
| `prototypes/sync-probe/` | Throwaway Rust prototype and analysis scripts (not to be carried into `src/`) |
| `design/detailed-design.md` | Approved, self-contained design |
| `implementation/plan.md` | Approved six-step plan with checklist |

## Design in brief

`--auto-sync [apply|report]` works on all four conversions. It correlates a
short-window rising-edge onset envelope of the audio against every chart
note, weighted by how many difficulties share it, and moves the chart by the
measured error.

- **Cap:** ±60 ms by default (`--auto-sync-max-ms`), searched to ±70 ms.
- **Resolution:** whole ms.
- **Chart-side only.** Runs after modernization and before `--sync-offset-ms`.
- **Refusals** leave the source sync unchanged with a warning:
  - too few notes, or no audio;
  - peak at the search edge;
  - a rival peak;
  - halves disagreeing by more than 5 ms;
  - a correction over the cap.
- **Calibration:** the target offset is calibrated against 711 TPS 1000 stock
  DDR World songs and a community offset database. An ignored,
  environment-gated test re-derives it.
- **Prototype accuracy:** within 1 ms of the community value for 91% of songs
  (94% on TPS 1000) and within 2 ms for 97%. That is at the resolution of the
  reference and of DDR World's own clock, whose integer-ms tempo anchors were
  verified in Ghidra.
- **Bundled fix:** `--sync-offset-ms` now moves the whole chart on every
  conversion.

## Plan in brief

1. Uniform timeline shift; the bias applies on every conversion.
2. `src/sync/` estimator on synthetic audio.
3. Calibration test against stock DDR World.
4. `--auto-sync` for `SM5 → DDR` and `DDR → SM5`.
5. `--auto-sync` for `DDR_LEGACY` inputs.
6. Cross-build check, manual validation, README guidance.

## Implementation status (2026-09-28)

All six plan steps are implemented and committed on `main`. Nothing is
pushed.

| Step | Commits |
|---|---|
| 1 Uniform shift / bias fix | `65fbee4` |
| 2 `src/sync/` estimator | `64c520d`, `96209ae`, `7eff865` |
| 3 Calibration (T = −2.11 ms; 94.1% ≤ 1 ms, 97.6% ≤ 2 ms on 711 songs) | `749f165` |
| 4 `--auto-sync` for SM5 ↔ DDR | `e0c126b` |
| 5 `--auto-sync` for DDR_LEGACY | `c8ba8d5` |
| 6 Cross-builds, legacy validation, README guidance | `869a854` |

Task files are in `.agents/tasks/2026-09-28-auto-sync/`, and the working
records in `.agents/scratchpad/2026-09-28-auto-sync/`. Legacy findings are
in `research/legacy-validation.md`.

## Next steps (original)

1. Run the code-task-generator sop against
   `.agents/planning/2026-09-28-auto-sync/implementation/plan.md` to produce
   task files.
2. Run the code-assist sop on each task in order.

## Open items and assumptions to watch

- **Calibration parity (plan Step 3).** If the production estimator falls
  materially below the prototype's agreement, stop before wiring the
  feature.
- **The +53 ms Ultramix guidance** is unvalidated. It was tuned against the
  buggy first-segment-only shift and is replaced by a report-mode
  measurement in Step 6.
- **Negative `tempo_data[0]`** beyond about −22 ms needs an in-game check
  (Step 6).
- **SM5 target.** Using the DDR World target for SM5 output assumes DDR
  World "in sync" equals StepMania null; checked by eye in ArrowVortex
  (Step 6).
- **Community CSV.** Parts of the `judgement_offsets.csv` used for
  calibration may be outdated (e.g. `tiho`). A refreshed CSV can be
  substituted in the calibration test at any time.
- **Follow-up outside this feature:** `DDR → SM5` of TPS ≠ 1000 sources uses
  exact `value / TPS` timing rather than the game's integer-ms anchors
  (up to about 0.5 ms per anchor).
- **Pre-existing parser limits found in the corpus:**
  - two stock SSQs (`lich`, `ifly`) rejected for a freeze-end without a
    head;
  - four stock XWBs (`hart`, `itiz`, `mixn`, `souv`) without `WBND` magic.
- **Repository cleanup** of `.agents/` and `CLAUDE.md` / `AGENTS.md`,
  requested by the maintainer, is separate from this project.
